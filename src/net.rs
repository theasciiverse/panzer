//! iroh networking on a background thread, bridged to the synchronous game
//! loop by channels that never block it. Taken from Tetrix
//! (`games/tetrix/src/net.rs`), narrowed to one guest.
//!
//! Host: binds an iroh endpoint, starts the LAN beacon, accepts the guest,
//! and delivers its messages tagged with a client id. Joiner: dials one
//! host and exchanges messages with it. Each connection carries a
//! single bidirectional QUIC stream of length-prefixed `proto::Msg` frames,
//! opened by the joiner (QUIC only reveals a stream once data is written,
//! so the joiner's `Hello` is what makes it appear on the host).

use crate::lan;
use crate::proto::{self, Msg};
use iroh::endpoint::presets;
use iroh::{Endpoint, EndpointAddr, EndpointId};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

const ALPN: &[u8] = b"panzer/1";
const MAX_FRAME: usize = 1 << 20;
const MAX_JOINERS: usize = 1;
/// Both sides ping every 2 s, so a peer silent this long has crashed or
/// lost the network; iroh's default is much longer.
const IDLE_TIMEOUT: Duration = Duration::from_secs(6);

pub type ClientId = u32;

fn endpoint_builder() -> iroh::endpoint::Builder {
    let transport = iroh::endpoint::QuicTransportConfig::builder()
        .max_idle_timeout(Some(IDLE_TIMEOUT.try_into().expect("6 s fits a QUIC idle timeout")))
        .build();
    Endpoint::builder(presets::N0).transport_config(transport)
}

pub enum Cmd {
    /// Host: to every joiner. Joiner: to the host.
    Broadcast(Msg),
    SendTo(ClientId, Msg),
    Kick(ClientId),
    /// Host: refresh what the LAN beacon says.
    Beacon { players: u8, in_game: bool },
}

#[derive(Debug)]
pub enum Evt {
    Hosting { id: EndpointId },
    Connected,
    Failed(String),
    ClientJoined { client: ClientId, name: String },
    ClientMsg { client: ClientId, msg: Msg },
    ClientLeft { client: ClientId },
    HostMsg(Msg),
    Disconnected,
}

pub struct Net {
    cmd: UnboundedSender<Cmd>,
    evt: mpsc::Receiver<Evt>,
}

impl Net {
    pub fn host(name: String) -> Net {
        Self::spawn(move |cmd, evt| async move { host_main(name, cmd, evt).await })
    }

    pub fn join(addr: EndpointAddr, name: String) -> Net {
        Self::spawn(move |cmd, evt| async move { join_main(addr, name, cmd, evt).await })
    }

    fn spawn<F, Fut>(f: F) -> Net
    where
        F: FnOnce(UnboundedReceiver<Cmd>, mpsc::Sender<Evt>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()>,
    {
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (evt_tx, evt_rx) = mpsc::channel();
        let fail = evt_tx.clone();
        let spawned = std::thread::Builder::new().name("panzer-net".into()).spawn(move || {
            match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
                Ok(rt) => rt.block_on(f(cmd_rx, evt_tx)),
                Err(e) => {
                    let _ = evt_tx.send(Evt::Failed(format!("no async runtime: {e}")));
                }
            }
        });
        if let Err(e) = spawned {
            let _ = fail.send(Evt::Failed(format!("no network thread: {e}")));
        }
        Net { cmd: cmd_tx, evt: evt_rx }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.cmd.send(c);
    }

    pub fn poll(&self) -> Option<Evt> {
        self.evt.try_recv().ok()
    }
}

type Clients = Arc<Mutex<HashMap<ClientId, UnboundedSender<Msg>>>>;

async fn host_main(name: String, mut cmd: UnboundedReceiver<Cmd>, evt: mpsc::Sender<Evt>) {
    let ep = match endpoint_builder().alpns(vec![ALPN.to_vec()]).bind().await {
        Ok(ep) => ep,
        Err(e) => {
            let _ = evt.send(Evt::Failed(format!("could not open network endpoint: {e}")));
            return;
        }
    };
    // Deliberately not waiting for `online()`: that needs a relay server,
    // and a LAN party without internet must still work.
    let _ = evt.send(Evt::Hosting { id: ep.id() });
    log::info!("hosting as {} on {:?}", ep.id(), ep.bound_sockets());

    let ports: Vec<u16> = ep.bound_sockets().iter().filter(|s| s.is_ipv4()).map(|s| s.port()).collect();
    let own_ips: Vec<Ipv4Addr> = ep
        .addr()
        .ip_addrs()
        .filter_map(|s| match s.ip() {
            IpAddr::V4(v4) => Some(v4),
            _ => None,
        })
        .collect();
    let beacon = lan::Beacon { id: ep.id(), ports, players: 1, in_game: false, name };
    let broadcaster = match lan::Broadcaster::start(beacon, own_ips, lan::PORT) {
        Ok(b) => Some(b),
        Err(e) => {
            log::warn!("LAN beacon unavailable: {e}");
            None
        }
    };

    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    let accept_ep = ep.clone();
    let (acc_clients, acc_evt) = (clients.clone(), evt.clone());
    tokio::spawn(async move {
        let mut next: ClientId = 1;
        while let Some(incoming) = accept_ep.accept().await {
            let id = next;
            next += 1;
            tokio::spawn(serve_client(incoming, id, acc_clients.clone(), acc_evt.clone()));
        }
    });

    while let Some(c) = cmd.recv().await {
        let map = clients.lock().unwrap();
        match c {
            Cmd::Broadcast(m) => map.values().for_each(|tx| drop(tx.send(m.clone()))),
            Cmd::SendTo(id, m) => {
                if let Some(tx) = map.get(&id) {
                    let _ = tx.send(m);
                }
            }
            Cmd::Kick(id) => {
                drop(map);
                clients.lock().unwrap().remove(&id);
            }
            Cmd::Beacon { players, in_game } => {
                if let Some(b) = &broadcaster {
                    b.update(players, in_game);
                }
            }
        }
    }
    // The game loop dropped us: close politely so joiners notice at once.
    drop(broadcaster);
    ep.close().await;
}

async fn serve_client(incoming: iroh::endpoint::Incoming, id: ClientId, clients: Clients, evt: mpsc::Sender<Evt>) {
    let conn = match incoming.accept() {
        Ok(a) => match a.await {
            Ok(c) => c,
            Err(e) => return log::warn!("client {id} handshake failed: {e}"),
        },
        Err(e) => return log::warn!("client {id} refused: {e}"),
    };
    let Ok((mut send, mut recv)) = conn.accept_bi().await else { return };
    let name = match read_msg(&mut recv).await {
        Some(Msg::Hello { name, version }) if version == proto::VERSION => name,
        Some(Msg::Hello { version, .. }) => {
            let reason = format!("version mismatch: host speaks {}, you speak {version}", proto::VERSION);
            let _ = write_msg(&mut send, &Msg::Reject { reason }).await;
            let _ = send.finish();
            tokio::time::sleep(Duration::from_millis(300)).await;
            return;
        }
        _ => return,
    };
    let (tx, mut rx) = unbounded_channel();
    let full = {
        let mut map = clients.lock().unwrap();
        let full = map.len() >= MAX_JOINERS;
        if !full {
            map.insert(id, tx);
        }
        full
    };
    if full {
        let _ = write_msg(&mut send, &Msg::Reject { reason: "the game already has two players".into() }).await;
        let _ = send.finish();
        tokio::time::sleep(Duration::from_millis(300)).await;
        return;
    }
    log::info!("client {id} joined as {name:?} from {}", conn.remote_id());
    let _ = evt.send(Evt::ClientJoined { client: id, name });

    let reader = async {
        while let Some(msg) = read_msg(&mut recv).await {
            if evt.send(Evt::ClientMsg { client: id, msg }).is_err() {
                return;
            }
        }
    };
    let writer = async {
        while let Some(m) = rx.recv().await {
            if write_msg(&mut send, &m).await.is_err() {
                return;
            }
        }
        let _ = send.finish();
    };
    tokio::select! {
        _ = reader => {}
        _ = writer => {}
    }
    clients.lock().unwrap().remove(&id);
    log::info!("client {id} left");
    let _ = evt.send(Evt::ClientLeft { client: id });
}

async fn join_main(addr: EndpointAddr, name: String, mut cmd: UnboundedReceiver<Cmd>, evt: mpsc::Sender<Evt>) {
    let ep = match endpoint_builder().bind().await {
        Ok(ep) => ep,
        Err(e) => {
            let _ = evt.send(Evt::Failed(format!("could not open network endpoint: {e}")));
            return;
        }
    };
    log::info!("joining {addr:?}");
    let conn = match tokio::time::timeout(Duration::from_secs(15), ep.connect(addr, ALPN)).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            let _ = evt.send(Evt::Failed(format!("could not connect: {e}")));
            return;
        }
        Err(_) => {
            let _ = evt.send(Evt::Failed("timed out reaching the host".into()));
            return;
        }
    };
    let (mut send, mut recv) = match conn.open_bi().await {
        Ok(s) => s,
        Err(e) => {
            let _ = evt.send(Evt::Failed(format!("could not open stream: {e}")));
            return;
        }
    };
    if write_msg(&mut send, &Msg::Hello { name, version: proto::VERSION }).await.is_err() {
        let _ = evt.send(Evt::Failed("host closed the connection".into()));
        return;
    }
    let _ = evt.send(Evt::Connected);

    let reader = async {
        while let Some(msg) = read_msg(&mut recv).await {
            if evt.send(Evt::HostMsg(msg)).is_err() {
                return;
            }
        }
    };
    let writer = async {
        while let Some(c) = cmd.recv().await {
            if let Cmd::Broadcast(m) | Cmd::SendTo(_, m) = c {
                if write_msg(&mut send, &m).await.is_err() {
                    return;
                }
            }
        }
        let _ = send.finish();
    };
    tokio::select! {
        _ = reader => {}
        _ = writer => {}
    }
    let _ = evt.send(Evt::Disconnected);
    conn.close(0u32.into(), b"bye");
    ep.close().await;
}

async fn write_msg<W: tokio::io::AsyncWrite + Unpin>(w: &mut W, m: &Msg) -> std::io::Result<()> {
    let bytes = proto::encode(m);
    w.write_all(&(bytes.len() as u32).to_le_bytes()).await?;
    w.write_all(&bytes).await
}

/// `None` when the stream ends or a frame is malformed.
async fn read_msg<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> Option<Msg> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).await.ok()?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return None;
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await.ok()?;
    proto::decode(&buf)
}
