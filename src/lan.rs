//! LAN discovery: hosts shout "a Panzer game is here" over UDP broadcast,
//! joiners listen and list them. No typing addresses, no internet needed.
//!
//! Taken from Tetrix (`games/tetrix/src/lan.rs`), which proved it: iroh only
//! connects to an `EndpointId` you already know and has no way to browse,
//! so this beacon sits in front of it. It carries the host's iroh UDP
//! ports, and the joiner pairs them with the packet's source IP, so the
//! connection goes direct over the LAN even with no internet at all.

use iroh::{EndpointAddr, EndpointId};
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Next to Tetrix's 31457, so a firewall rule for one is easy to widen.
pub const PORT: u16 = 31458;
const MAGIC: &str = "PANZER-LAN-1";
const INTERVAL: Duration = Duration::from_millis(800);
const STALE_AFTER: Duration = Duration::from_secs(4);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Beacon {
    pub id: EndpointId,
    pub ports: Vec<u16>,
    pub players: u8,
    pub in_game: bool,
    pub name: String,
}

fn encode(b: &Beacon) -> String {
    let ports: Vec<String> = b.ports.iter().map(|p| p.to_string()).collect();
    format!("{MAGIC}|{}|{}|{}|{}|{}", b.id, ports.join(","), b.players, b.in_game as u8, b.name)
}

fn decode(line: &str) -> Option<Beacon> {
    let mut parts = line.splitn(6, '|');
    if parts.next()? != MAGIC {
        return None;
    }
    let id = parts.next()?.parse().ok()?;
    let ports = parts.next()?.split(',').filter_map(|p| p.parse().ok()).collect();
    let players = parts.next()?.parse().ok()?;
    let in_game = parts.next()? == "1";
    let name = parts.next()?.to_string();
    Some(Beacon { id, ports, players, in_game, name })
}

/// Broadcasts the beacon until dropped. `update` changes what is announced.
pub struct Broadcaster {
    beacon: Arc<Mutex<Beacon>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Broadcaster {
    /// Sends to the limited broadcast address plus the /24 broadcast of each
    /// of the host's own IPv4 addresses, since 255.255.255.255 only leaves by
    /// the default-route interface on many systems.
    pub fn start(beacon: Beacon, own_ips: Vec<Ipv4Addr>, port: u16) -> io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", 0))?;
        socket.set_broadcast(true)?;
        let mut targets = vec![Ipv4Addr::BROADCAST];
        for ip in own_ips {
            if ip.is_private() {
                let o = ip.octets();
                targets.push(Ipv4Addr::new(o[0], o[1], o[2], 255));
            }
        }
        targets.dedup();
        let beacon = Arc::new(Mutex::new(beacon));
        let stop = Arc::new(AtomicBool::new(false));
        let (b2, s2) = (beacon.clone(), stop.clone());
        let handle = thread::Builder::new().name("panzer-beacon".into()).spawn(move || {
            while !s2.load(Ordering::Relaxed) {
                let payload = encode(&b2.lock().unwrap());
                for t in &targets {
                    let _ = socket.send_to(payload.as_bytes(), (*t, port));
                }
                thread::sleep(INTERVAL);
            }
        })?;
        Ok(Broadcaster { beacon, stop, handle: Some(handle) })
    }

    pub fn update(&self, players: u8, in_game: bool) {
        let mut b = self.beacon.lock().unwrap();
        b.players = players;
        b.in_game = in_game;
    }
}

impl Drop for Broadcaster {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[derive(Clone, Debug)]
pub struct Found {
    pub beacon: Beacon,
    pub from: IpAddr,
}

impl Found {
    /// Direct LAN addresses for iroh to dial, no lookup service required.
    pub fn addr(&self) -> EndpointAddr {
        let mut a = EndpointAddr::new(self.beacon.id);
        for &p in &self.beacon.ports {
            a = a.with_ip_addr(SocketAddr::new(self.from, p));
        }
        a
    }
}

/// Collects beacons on a background thread; `hosts()` is a live snapshot.
pub struct Listener {
    known: Arc<Mutex<HashMap<EndpointId, (Found, Instant)>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Listener {
    pub fn start(port: u16) -> io::Result<Self> {
        // Address/port reuse so several Panzer instances on one machine
        // (and anything else listening) can all hear the broadcasts.
        let s = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::DGRAM, Some(socket2::Protocol::UDP))?;
        s.set_reuse_address(true)?;
        #[cfg(unix)]
        s.set_reuse_port(true)?;
        s.bind(&SocketAddr::from(([0, 0, 0, 0], port)).into())?;
        let socket = UdpSocket::from(s);
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;

        let known = Arc::new(Mutex::new(HashMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (k2, s2) = (known.clone(), stop.clone());
        let handle = thread::Builder::new().name("panzer-lan-listen".into()).spawn(move || {
            let mut buf = [0u8; 1024];
            while !s2.load(Ordering::Relaxed) {
                if let Ok((n, from)) = socket.recv_from(&mut buf) {
                    if let Some(beacon) = std::str::from_utf8(&buf[..n]).ok().and_then(decode) {
                        let id = beacon.id;
                        k2.lock().unwrap().insert(id, (Found { beacon, from: from.ip() }, Instant::now()));
                    }
                }
            }
        })?;
        Ok(Listener { known, stop, handle: Some(handle) })
    }

    pub fn hosts(&self) -> Vec<Found> {
        let mut k = self.known.lock().unwrap();
        k.retain(|_, (_, seen)| seen.elapsed() <= STALE_AFTER);
        let mut v: Vec<Found> = k.values().map(|(f, _)| f.clone()).collect();
        v.sort_by(|a, b| a.beacon.name.cmp(&b.beacon.name).then(a.beacon.id.cmp(&b.beacon.id)));
        v
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> EndpointId {
        iroh::SecretKey::generate().public()
    }

    #[test]
    fn beacon_round_trip() {
        let b = Beacon { id: id(), ports: vec![1234, 5678], players: 3, in_game: true, name: "dåvid's | game".into() };
        assert_eq!(decode(&encode(&b)), Some(b));
        assert_eq!(decode("MATO-ANNOUNCE-1|x|y"), None);
    }

    #[test]
    fn broadcast_is_heard_locally() {
        const P: u16 = 31_991;
        let l = Listener::start(P).expect("listen");
        let b = Beacon { id: id(), ports: vec![9], players: 1, in_game: false, name: "t".into() };
        let _bc = Broadcaster::start(b.clone(), vec![], P).expect("broadcast");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if l.hosts().iter().any(|f| f.beacon == b) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("beacon not heard on this machine within 5 s");
    }
}
