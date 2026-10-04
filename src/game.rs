//! The campaign around the battles: title, LAN lobby, mission cards,
//! lives, the results, and the HUD and cards drawn over the arena.
//!
//! Three roles share one state machine. Solo and Host run the simulation;
//! a Host also takes the guest's controls and sends a frame every tick. A
//! Guest runs no simulation at all: it mirrors the host's phase and battle
//! and only sends its controls and menu presses.

use crate::ai::SPECS;
use crate::arena::{parse_missions, Mission, MISSIONS_TXT};
use crate::audio::{Audio, Engine, Music, Sfx, SLOTS};
use crate::input::{Cmd, Input};
use crate::lan;
use crate::net::{self, ClientId, Evt, Net};
use crate::proto::{GameState, Msg, Press};
use crate::render::{self, Layout};
use crate::world::{Controls, Event, World, P2_KIND};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::Frame;
use serde::{Deserialize, Serialize};

pub const DT: f32 = 1.0 / 60.0;
const START_LIVES: u32 = 3;
/// A spare tank after clearing every this many missions.
const BONUS_EVERY: usize = 5;
const TOAST_FOR: f32 = 4.0;

const AMBER: Color = Color::Rgb(255, 180, 40);
const GREEN: Color = Color::Rgb(90, 220, 120);
const DIM: Color = Color::Rgb(120, 130, 120);
const INK: Color = Color::Rgb(16, 18, 16);
const PAPER: Color = Color::Rgb(232, 226, 210);

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Phase {
    Title,
    /// Hosting: waiting for a second player, picking the mission.
    Lobby,
    /// Looking for games on the LAN.
    Browse,
    Intro,
    Play,
    Paused,
    Won,
    Lost,
    Over,
}

const MENU: [&str; 4] = ["Play", "Host a LAN game", "Join a LAN game", "Quit"];

/// How the program was started.
pub enum Start {
    Title,
    Mission(usize),
    Host,
    Browse,
    Join(iroh::EndpointAddr),
}

struct Host {
    net: Net,
    guest: Option<(ClientId, String)>,
    /// The guest's latest controls; fire and mine are kept until used.
    held: Controls,
    fire: bool,
    mine: bool,
    presses: Vec<Press>,
    /// Events for the next frame to the guest.
    out: Vec<Event>,
}

struct Guest {
    net: Net,
    host_name: String,
    connected: bool,
    ping_t: f32,
}

enum Role {
    Solo,
    Host(Box<Host>),
    Guest(Box<Guest>),
}

pub struct Game {
    pub missions: Vec<Mission>,
    pub phase: Phase,
    pub phase_t: f32,
    pub mission: usize,
    pub lives: u32,
    /// Enemy spawns already destroyed in this mission (they stay gone when you
    /// lose a tank, as in the original).
    gone: Vec<usize>,
    /// The same count as the host reports it, when this is the guest.
    guest_gone: usize,
    /// Tanks destroyed in this campaign, by roster kind.
    pub tally: [u32; 10],
    pub world: World,
    pub pick: usize,
    menu: usize,
    pub help: bool,
    pub victory: bool,
    pub bonus: bool,
    pub time: f32,
    seed: u64,
    pub exit: bool,
    role: Role,
    /// Players in the current campaign, and which tank is ours.
    players: usize,
    pub me: usize,
    attempt: u32,
    name: String,
    browser: Option<lan::Listener>,
    browse_pick: usize,
    toast: Option<(String, f32)>,
}

fn player_name() -> String {
    let n = std::env::var("USER").or_else(|_| std::env::var("USERNAME")).unwrap_or_else(|_| "Player".into());
    n.chars().take(16).collect()
}

impl Game {
    pub fn new(start: Start) -> Game {
        let missions = parse_missions(MISSIONS_TXT).expect("bundled missions parse");
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        let world = World::new(&missions[0], &[], seed, 1);
        let mut g = Game {
            missions,
            phase: Phase::Title,
            phase_t: 0.0,
            mission: 0,
            lives: START_LIVES,
            gone: Vec::new(),
            guest_gone: 0,
            tally: [0; 10],
            world,
            pick: 0,
            menu: 0,
            help: false,
            victory: false,
            bonus: false,
            time: 0.0,
            seed,
            exit: false,
            role: Role::Solo,
            players: 1,
            me: 0,
            attempt: 0,
            name: player_name(),
            browser: None,
            browse_pick: 0,
            toast: None,
        };
        match start {
            Start::Title => {}
            Start::Mission(m) => {
                g.pick = m.min(g.missions.len() - 1);
                g.start_campaign();
            }
            Start::Host => g.host(),
            Start::Browse => g.browse(),
            Start::Join(addr) => {
                g.browse();
                g.join(addr, "a direct link".into());
            }
        }
        g
    }

    pub fn is_guest(&self) -> bool {
        matches!(self.role, Role::Guest(_))
    }

    fn say(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::info!("toast: {msg}");
        self.toast = Some((msg, self.time));
    }

    fn set_phase(&mut self, p: Phase) {
        log::info!("phase {:?} -> {:?} (mission {}, lives {})", self.phase, p, self.mission + 1, self.lives);
        self.phase = p;
        self.phase_t = 0.0;
    }

    fn host(&mut self) {
        self.role = Role::Host(Box::new(Host {
            net: Net::host(self.name.clone()),
            guest: None,
            held: Controls::default(),
            fire: false,
            mine: false,
            presses: Vec::new(),
            out: Vec::new(),
        }));
        self.me = 0;
        self.set_phase(Phase::Lobby);
    }

    fn browse(&mut self) {
        match lan::Listener::start(lan::PORT) {
            Ok(l) => self.browser = Some(l),
            Err(e) => self.say(format!("Cannot listen for LAN games: {e}")),
        }
        self.browse_pick = 0;
        self.role = Role::Solo;
        self.set_phase(Phase::Browse);
    }

    fn join(&mut self, addr: iroh::EndpointAddr, host_name: String) {
        log::info!("joining {host_name}");
        self.role = Role::Guest(Box::new(Guest { net: Net::join(addr, self.name.clone()), host_name, connected: false, ping_t: 0.0 }));
        self.me = 1;
    }

    /// Leave any LAN game and go back to the title.
    fn leave(&mut self, why: Option<&str>) {
        self.role = Role::Solo;
        self.browser = None;
        self.me = 0;
        if let Some(w) = why {
            self.say(w);
        }
        self.set_phase(Phase::Title);
    }

    fn start_campaign(&mut self) {
        self.mission = self.pick;
        self.lives = START_LIVES;
        self.tally = [0; 10];
        self.gone.clear();
        self.victory = false;
        self.players = match &self.role {
            Role::Host(h) if h.guest.is_some() => 2,
            _ => 1,
        };
        self.begin_mission();
    }

    fn begin_mission(&mut self) {
        self.seed = self.seed.wrapping_add(0x9e37_79b9);
        self.attempt += 1;
        self.world = World::new(&self.missions[self.mission], &self.gone, self.seed, self.players);
        self.set_phase(Phase::Intro);
    }

    fn gone_count(&self) -> usize {
        if self.is_guest() {
            self.guest_gone
        } else {
            self.gone.len()
        }
    }

    /// Handle commands, advance one tick (`DT`), and play the sounds that go with it.
    pub fn tick(&mut self, input: &mut Input, controls: impl FnOnce(&mut Input) -> Controls, audio: &mut Audio) {
        self.time += DT;
        self.phase_t += DT;
        let (prev, prev_t) = (self.phase, self.phase_t - DT);
        let mut cmds: Vec<Cmd> = input.cmds.drain(..).collect();
        for c in &cmds {
            match c {
                Cmd::Exit => self.exit = true,
                Cmd::Mute => audio.toggle_mute(),
                Cmd::Music => audio.toggle_music(),
                Cmd::Help if self.phase != Phase::Play => self.help = !self.help,
                Cmd::Help if !self.is_guest() => {
                    self.help = true;
                    self.set_phase(Phase::Paused);
                }
                _ => {}
            }
        }
        if self.help && cmds.iter().any(|c| matches!(c, Cmd::Back | Cmd::Confirm)) {
            self.help = false;
            input.clear();
            cmds.clear();
        }
        self.poll_net();
        if self.is_guest() {
            self.guest_tick(&cmds, input, controls, audio);
        } else {
            // the guest's menu presses count as ours
            if let Role::Host(h) = &mut self.role {
                for p in h.presses.drain(..) {
                    match p {
                        Press::Confirm if self.phase != Phase::Lobby => cmds.push(Cmd::Confirm),
                        Press::Pause => cmds.push(if self.phase == Phase::Paused { Cmd::Confirm } else { Cmd::Pause }),
                        _ => {}
                    }
                }
            }
            self.local_tick(&cmds, input, controls, audio);
            self.send_frame();
        }
        self.cue(prev, prev_t, audio);
        self.update_band(audio);
    }

    fn local_tick(&mut self, cmds: &[Cmd], input: &mut Input, controls: impl FnOnce(&mut Input) -> Controls, audio: &mut Audio) {
        let confirm = cmds.contains(&Cmd::Confirm);
        let back = cmds.contains(&Cmd::Back) && !self.help;
        let n = self.missions.len();
        let step_pick = |pick: &mut usize| {
            let mut moved = false;
            for c in cmds {
                match c {
                    Cmd::Left => *pick = (*pick + n - 1) % n,
                    Cmd::Right => *pick = (*pick + 1) % n,
                    _ => continue,
                }
                moved = true;
            }
            moved
        };
        match self.phase {
            Phase::Title => {
                let mut moved = step_pick(&mut self.pick);
                for c in cmds {
                    match c {
                        Cmd::Up => self.menu = (self.menu + MENU.len() - 1) % MENU.len(),
                        Cmd::Down => self.menu = (self.menu + 1) % MENU.len(),
                        _ => continue,
                    }
                    moved = true;
                }
                if moved {
                    audio.play(Sfx::Select, 0.6);
                }
                if back && !self.help {
                    self.exit = true;
                } else if confirm && !self.help {
                    input.clear();
                    match self.menu {
                        0 => self.start_campaign(),
                        1 => self.host(),
                        2 => self.browse(),
                        _ => self.exit = true,
                    }
                }
            }
            Phase::Lobby => {
                if step_pick(&mut self.pick) {
                    audio.play(Sfx::Select, 0.6);
                }
                if back {
                    self.leave(None);
                } else if confirm {
                    input.clear();
                    self.start_campaign();
                }
            }
            Phase::Browse => {
                let found = self.browser.as_ref().map_or(0, |b| b.hosts().len());
                for c in cmds {
                    match c {
                        Cmd::Up if found > 0 => self.browse_pick = (self.browse_pick + found - 1) % found,
                        Cmd::Down if found > 0 => self.browse_pick = (self.browse_pick + 1) % found,
                        _ => {}
                    }
                }
                if back {
                    self.leave(None);
                } else if confirm && found > 0 {
                    input.clear();
                    let f = self.browser.as_ref().map(|b| b.hosts()).unwrap_or_default();
                    let pick = &f[self.browse_pick.min(f.len() - 1)];
                    if pick.beacon.in_game {
                        self.say(format!("{}'s game has already started", pick.beacon.name));
                    } else {
                        self.join(pick.addr(), pick.beacon.name.clone());
                    }
                }
            }
            Phase::Intro => {
                if back {
                    self.to_menu();
                } else if self.phase_t > 3.2 || (confirm && self.phase_t > 0.4) {
                    input.clear();
                    self.set_phase(Phase::Play);
                }
            }
            Phase::Play => {
                if back || cmds.contains(&Cmd::Pause) {
                    self.set_phase(Phase::Paused);
                    return;
                }
                let mut cs = vec![controls(input)];
                if let Role::Host(h) = &mut self.role {
                    let mut c = h.held;
                    c.fire = std::mem::take(&mut h.fire);
                    c.mine = std::mem::take(&mut h.mine);
                    if h.guest.is_none() {
                        c = Controls::default();
                    }
                    cs.push(c);
                }
                self.world.step(DT, &cs);
                self.sounds(audio);
                match self.world.outcome() {
                    Some(true) => {
                        self.bonus = (self.mission + 1) % BONUS_EVERY == 0 && self.mission + 1 < self.missions.len();
                        if self.bonus {
                            self.lives += 1;
                        }
                        self.set_phase(Phase::Won);
                    }
                    Some(false) => {
                        self.gone.extend(self.world.kills.iter().copied());
                        self.lives = self.lives.saturating_sub(1);
                        self.set_phase(Phase::Lost);
                    }
                    None => {}
                }
            }
            Phase::Paused => {
                if (cmds.contains(&Cmd::Pause) || confirm) && !self.help {
                    input.clear();
                    self.set_phase(Phase::Play);
                } else if back && !self.help {
                    self.to_menu();
                }
            }
            Phase::Won => {
                // keep the smoke drifting while the card is up
                self.world.step(DT, &[]);
                self.sounds(audio);
                if self.phase_t > 3.5 || (confirm && self.phase_t > 0.8) {
                    input.clear();
                    if self.mission + 1 >= self.missions.len() {
                        self.victory = true;
                        self.set_phase(Phase::Over);
                    } else {
                        self.mission += 1;
                        self.gone.clear();
                        self.begin_mission();
                    }
                }
            }
            Phase::Lost => {
                self.world.step(DT, &[]);
                self.sounds(audio);
                if self.phase_t > 3.0 || (confirm && self.phase_t > 0.8) {
                    input.clear();
                    if self.lives == 0 {
                        self.set_phase(Phase::Over);
                    } else {
                        self.begin_mission();
                    }
                }
            }
            Phase::Over => {
                if (confirm || back) && self.phase_t > 1.0 {
                    input.clear();
                    self.pick = 0;
                    self.to_menu();
                }
            }
        }
    }

    /// Out of a campaign: hosts go back to their lobby, everyone else to the title.
    fn to_menu(&mut self) {
        if matches!(self.role, Role::Host(_)) {
            self.set_phase(Phase::Lobby);
        } else {
            self.set_phase(Phase::Title);
        }
    }

    fn guest_tick(&mut self, cmds: &[Cmd], input: &mut Input, controls: impl FnOnce(&mut Input) -> Controls, audio: &mut Audio) {
        let Role::Guest(g) = &mut self.role else { return };
        let back = cmds.contains(&Cmd::Back) && !self.help;
        let in_battle = matches!(self.phase, Phase::Play);
        if cmds.contains(&Cmd::Confirm) && !self.help {
            g.net.send(net::Cmd::Broadcast(Msg::Press(Press::Confirm)));
        }
        if cmds.contains(&Cmd::Pause) || (back && in_battle) {
            g.net.send(net::Cmd::Broadcast(Msg::Press(Press::Pause)));
        }
        if in_battle {
            let c = controls(input);
            g.net.send(net::Cmd::Broadcast(Msg::Input(c)));
        }
        g.ping_t += DT;
        if g.ping_t > 2.0 {
            g.ping_t = 0.0;
            g.net.send(net::Cmd::Broadcast(Msg::Ping));
        }
        if back && !in_battle {
            if self.phase == Phase::Browse {
                // still connecting: give up on this host, keep browsing
                self.role = Role::Solo;
                self.me = 0;
                self.say("Cancelled");
            } else {
                self.leave(Some("You left the game"));
            }
        }
        self.sounds(audio);
    }

    fn poll_net(&mut self) {
        let mut evts = Vec::new();
        match &self.role {
            Role::Host(h) => {
                while let Some(e) = h.net.poll() {
                    evts.push(e);
                }
            }
            Role::Guest(g) => {
                while let Some(e) = g.net.poll() {
                    evts.push(e);
                }
            }
            Role::Solo => return,
        }
        for e in evts {
            if !matches!(e, Evt::ClientMsg { msg: Msg::Input(_) | Msg::Ping, .. } | Evt::HostMsg(Msg::Frame { .. })) {
                log::debug!("net {e:?}");
            }
            match e {
                Evt::Hosting { id } => log::info!("hosting as {id}"),
                Evt::ClientJoined { client, name } => {
                    let lobby = self.phase == Phase::Lobby;
                    let Role::Host(h) = &mut self.role else { continue };
                    if h.guest.is_some() || !lobby {
                        let reason = if lobby { "the game already has two players" } else { "the game has already started" };
                        h.net.send(net::Cmd::SendTo(client, Msg::Reject { reason: reason.into() }));
                        h.net.send(net::Cmd::Kick(client));
                    } else {
                        h.guest = Some((client, name.clone()));
                        self.say(format!("{name} joined"));
                    }
                }
                Evt::ClientMsg { client, msg } => {
                    let Role::Host(h) = &mut self.role else { continue };
                    if h.guest.as_ref().map(|g| g.0) != Some(client) {
                        continue;
                    }
                    match msg {
                        Msg::Input(c) => {
                            h.held = c;
                            h.fire |= c.fire;
                            h.mine |= c.mine;
                        }
                        Msg::Press(p) => h.presses.push(p),
                        _ => {}
                    }
                }
                Evt::ClientLeft { client } => {
                    let Role::Host(h) = &mut self.role else { continue };
                    if let Some((id, name)) = h.guest.clone() {
                        if id == client {
                            h.guest = None;
                            h.held = Controls::default();
                            self.say(format!("{name} left"));
                        }
                    }
                }
                Evt::Connected => {
                    if let Role::Guest(g) = &mut self.role {
                        g.connected = true;
                        let host = g.host_name.clone();
                        self.say(format!("Connected to {host}"));
                    }
                }
                Evt::HostMsg(Msg::Frame { game, snap }) => self.apply_frame(game, snap),
                Evt::HostMsg(Msg::Reject { reason }) => {
                    self.role = Role::Solo;
                    self.me = 0;
                    self.say(format!("Turned away: {reason}"));
                    self.set_phase(Phase::Browse);
                }
                Evt::HostMsg(_) => {}
                Evt::Failed(why) => {
                    let hosting = matches!(self.role, Role::Host(_));
                    self.role = Role::Solo;
                    self.me = 0;
                    self.say(why);
                    if hosting {
                        self.set_phase(Phase::Title);
                    }
                }
                Evt::Disconnected => {
                    if self.phase != Phase::Browse {
                        self.leave(Some("The host left the game"));
                    } else {
                        self.role = Role::Solo;
                        self.me = 0;
                    }
                }
            }
        }
    }

    fn game_state(&self) -> GameState {
        let guest_name = match &self.role {
            Role::Host(h) => h.guest.as_ref().map(|g| g.1.clone()),
            _ => None,
        };
        GameState {
            phase: self.phase,
            phase_t: self.phase_t,
            mission: if self.phase == Phase::Lobby { self.pick } else { self.mission } as u16,
            attempt: self.attempt,
            players: self.players as u8,
            lives: self.lives,
            bonus: self.bonus,
            victory: self.victory,
            tally: self.tally,
            gone: self.gone.len() as u16,
            host_name: self.name.clone(),
            guest_name,
        }
    }

    /// Host: one frame to the guest, every tick.
    fn send_frame(&mut self) {
        let battle = !matches!(self.phase, Phase::Title | Phase::Lobby | Phase::Browse);
        let state = self.game_state();
        let Role::Host(h) = &mut self.role else { return };
        h.net.send(net::Cmd::Beacon { players: 1 + h.guest.is_some() as u8, in_game: self.phase != Phase::Lobby });
        if h.guest.is_none() {
            h.out.clear();
            return;
        }
        let events = std::mem::take(&mut h.out);
        let snap = battle.then(|| self.world.snapshot(events));
        h.net.send(net::Cmd::Broadcast(Msg::Frame { game: state, snap }));
    }

    /// Guest: become what the host says.
    fn apply_frame(&mut self, game: GameState, snap: Option<crate::world::Snapshot>) {
        let mission = (game.mission as usize).min(self.missions.len() - 1);
        if game.attempt != self.attempt || game.players as usize != self.world.players || mission != self.mission {
            self.world = World::new(&self.missions[mission], &[], 0, game.players.max(1) as usize);
            self.attempt = game.attempt;
        }
        if game.phase != self.phase {
            log::info!("host phase {:?} -> {:?}", self.phase, game.phase);
        }
        self.phase = game.phase;
        self.phase_t = game.phase_t;
        self.mission = mission;
        self.pick = mission;
        self.players = game.players as usize;
        self.lives = game.lives;
        self.bonus = game.bonus;
        self.victory = game.victory;
        self.tally = game.tally;
        self.guest_gone = game.gone as usize;
        if let Role::Guest(g) = &mut self.role {
            g.host_name = game.host_name;
            g.connected = true;
        }
        if let Some(s) = snap {
            self.world.apply(&s);
        }
    }

    /// The sounds of what just happened in the battle (host: and pass the
    /// events on to the guest).
    fn sounds(&mut self, audio: &mut Audio) {
        let events: Vec<Event> = self.world.events.drain(..).collect();
        for &e in &events {
            match e {
                Event::Shot { rocket, .. } => audio.play(if rocket { Sfx::Rocket } else { Sfx::Shot }, 0.7),
                Event::Bounce { .. } => audio.play(Sfx::Bounce, 0.6),
                Event::Fizzle { .. } => audio.play(Sfx::Fizzle, 0.5),
                Event::Cancel { .. } => audio.play(Sfx::Cancel, 0.7),
                Event::Empty { who } => {
                    if who as usize == self.me {
                        audio.play(Sfx::Click, 0.6)
                    }
                }
                Event::TankDown { kind, .. } => {
                    let player = kind == 0 || kind == P2_KIND;
                    audio.play(Sfx::Boom, if player { 1.0 } else { 0.85 });
                    if !player && !self.is_guest() {
                        self.tally[kind] += 1;
                    }
                }
                Event::MineSet => audio.play(Sfx::MineSet, 0.7),
                Event::MineBeep => audio.play(Sfx::MineBeep, 0.5),
                Event::MineBoom { .. } => audio.play(Sfx::MineBoom, 1.0),
                Event::CrateBroken { .. } => audio.play(Sfx::Crate, 0.7),
            }
        }
        if let Role::Host(h) = &mut self.role {
            h.out.extend(events);
        }
    }

    /// Jingles that go with entering a phase, on host and guest alike.
    fn cue(&self, prev: Phase, prev_t: f32, audio: &mut Audio) {
        if self.phase != prev {
            match self.phase {
                Phase::Intro => audio.play(Sfx::Start, 0.8),
                Phase::Won => audio.play(Sfx::Clear, 0.9),
                Phase::Lost => audio.play(Sfx::Down, 0.9),
                _ => {}
            }
        } else if self.phase == Phase::Won && self.bonus && prev_t < 1.0 && self.phase_t >= 1.0 {
            audio.play(Sfx::Life, 0.8);
        }
    }

    /// Engines and music follow the battle.
    fn update_band(&self, audio: &mut Audio) {
        let mut engines = [Engine::default(); SLOTS];
        if self.phase == Phase::Play {
            for (slot, t) in engines.iter_mut().zip(self.world.tanks.iter()) {
                if t.alive {
                    let voice = if t.kind == P2_KIND { 1 } else { t.kind as u8 + 1 };
                    *slot = Engine { kind: voice, speed: t.speed };
                }
            }
        }
        audio.set_engines(&engines);
        match self.phase {
            Phase::Title | Phase::Over | Phase::Lobby | Phase::Browse => audio.set_music(Music::Title, 0),
            Phase::Play => {
                let mut layers = 0u16;
                for t in self.world.enemies().iter().filter(|t| t.alive) {
                    layers |= 1 << t.kind;
                }
                audio.set_music(Music::Battle, layers);
            }
            _ => audio.set_music(Music::Off, 0),
        }
    }

    // ------------------------------------------------------------- drawing

    pub fn draw(&self, frame: &mut Frame, input: &Input, layout: &Layout) {
        let full = frame.area();
        frame.render_widget(Block::new().style(Style::new().bg(INK)), full);
        if full.width < 48 || full.height < 18 {
            frame.render_widget(Paragraph::new("Panzer needs a terminal of at least 48x18.\nEnlarge the window or shrink the font.").style(Style::new().fg(AMBER)), full);
            return;
        }
        if matches!(self.phase, Phase::Title | Phase::Lobby | Phase::Browse) {
            render::draw_title(frame.buffer_mut(), full, self.time);
            match self.phase {
                Phase::Title => self.draw_title_text(frame, full),
                Phase::Lobby => self.lobby_card(frame, full),
                _ => self.browse_card(frame, full),
            }
            if self.help {
                help_card(frame, full);
            }
            self.draw_toast(frame, full);
            return;
        }
        let mouse_aim = input.aiming_with_mouse().then(|| input.mouse.and_then(|(c, r)| layout.cell_to_world(c, r))).flatten();
        let dim = match self.phase {
            Phase::Play => None,
            Phase::Intro => Some(0.55),
            Phase::Over => Some(0.4),
            Phase::Paused => Some(0.6),
            _ => Some(0.8),
        };
        render::draw_arena(frame.buffer_mut(), layout, &self.world, self.me, mouse_aim, self.phase == Phase::Play, dim);
        if self.world.players > 1 && dim.is_none() {
            self.player_labels(frame, layout);
        }
        self.draw_hud(frame, full);
        match self.phase {
            Phase::Intro => self.intro_card(frame, full),
            Phase::Paused if !self.help => {
                let esc = match self.role {
                    Role::Solo => "Esc: back to the title",
                    Role::Host(_) => "Esc: back to the lobby",
                    Role::Guest(_) => "Esc: leave the game",
                };
                card(frame, full, "PAUSED", vec![Line::from(""), Line::from("P or Space: carry on"), Line::from(esc), Line::from("H: help")], AMBER)
            }
            Phase::Won => {
                let mut lines = vec![Line::from(""), Line::from(format!("Mission {} cleared", self.mission + 1))];
                if self.bonus && self.phase_t > 1.0 {
                    lines.push(Line::from(""));
                    lines.push(Line::styled("+1 spare tank!", Style::new().fg(GREEN).add_modifier(Modifier::BOLD)));
                }
                card(frame, full, "MISSION CLEARED!", lines, GREEN);
            }
            Phase::Lost => {
                let left = match self.lives {
                    0 => "No tanks left.".to_string(),
                    1 => "1 tank left.".to_string(),
                    n => format!("{n} tanks left."),
                };
                card(frame, full, "TANK DESTROYED", vec![Line::from(""), Line::from(left), Line::from(""), Line::styled("Destroyed enemies stay destroyed.", Style::new().fg(DIM))], Color::Rgb(240, 90, 70));
            }
            Phase::Over => self.results_card(frame, full),
            _ => {}
        }
        if self.help {
            help_card(frame, full);
        }
        self.draw_toast(frame, full);
    }

    fn draw_toast(&self, frame: &mut Frame, full: Rect) {
        let Some((msg, at)) = &self.toast else { return };
        if self.time - at > TOAST_FOR {
            return;
        }
        let w = (msg.chars().count() as u16 + 4).min(full.width);
        let area = Rect { x: full.x + (full.width - w) / 2, y: full.y + full.height.saturating_sub(4), width: w, height: 1 };
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(Line::styled(format!("  {msg}  "), Style::new().fg(INK).bg(AMBER).add_modifier(Modifier::BOLD))), area);
    }

    /// "P1" and "P2" over the tanks in a two-player game.
    fn player_labels(&self, frame: &mut Frame, layout: &Layout) {
        for i in 0..self.world.players {
            let t = &self.world.tanks[i];
            if !t.alive {
                continue;
            }
            let c = t.spec().turret;
            let col = layout.area.x as f32 + (t.x - layout.ox) * layout.s - 1.0;
            let row = layout.area.y as f32 + ((t.y - 1.05 - layout.oy) * layout.s) / 2.0;
            if col < layout.area.x as f32 || row < layout.area.y as f32 {
                continue;
            }
            let label = if i == self.me { format!("P{} you", i + 1) } else { format!("P{}", i + 1) };
            let area = Rect { x: col as u16, y: row as u16, width: label.len() as u16, height: 1 }.intersection(layout.area);
            frame.render_widget(Paragraph::new(Line::styled(label, Style::new().fg(Color::Rgb(c.0, c.1, c.2)).bg(INK).add_modifier(Modifier::BOLD))), area);
        }
    }

    fn lobby_card(&self, frame: &mut Frame, full: Rect) {
        let m = &self.missions[self.pick];
        let mut lines = vec![Line::from("")];
        match &self.role {
            Role::Host(h) => {
                lines.push(Line::from(format!("Hosting as {} on the local network.", self.name)));
                lines.push(Line::from(""));
                match &h.guest {
                    Some((_, n)) => lines.push(Line::styled(format!("{n} has joined: two tanks, one campaign."), Style::new().fg(GREEN).add_modifier(Modifier::BOLD))),
                    None => lines.push(Line::styled("Waiting for a second player (they pick Join a LAN game)...", Style::new().fg(DIM))),
                }
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("◀  ", Style::new().fg(AMBER)),
                    Span::styled(format!("Mission {:>2}: {}", self.pick + 1, m.name), Style::new().fg(PAPER).add_modifier(Modifier::BOLD)),
                    Span::styled("  ▶", Style::new().fg(AMBER)),
                ]));
                lines.push(Line::from(""));
                let start = if h.guest.is_some() { "Space: start together" } else { "Space: start alone" };
                lines.push(Line::styled(format!("{start}    Left/Right: mission    Esc: stop hosting"), Style::new().fg(DIM)));
            }
            Role::Guest(g) => {
                lines.push(Line::styled(format!("You are in {}'s game.", g.host_name), Style::new().fg(GREEN).add_modifier(Modifier::BOLD)));
                lines.push(Line::from(""));
                lines.push(Line::from(format!("Mission {:>2}: {}", self.pick + 1, m.name)));
                lines.push(Line::from(""));
                lines.push(Line::styled("Waiting for the host to start.    Esc: leave", Style::new().fg(DIM)));
            }
            Role::Solo => {}
        }
        card(frame, full, "LAN GAME", lines, AMBER);
    }

    fn browse_card(&self, frame: &mut Frame, full: Rect) {
        let mut lines = vec![Line::from("")];
        if let Role::Guest(g) = &self.role {
            lines.push(Line::from(format!("Connecting to {}...", g.host_name)));
            lines.push(Line::from(""));
            lines.push(Line::styled("Esc: cancel", Style::new().fg(DIM)));
            card(frame, full, "JOIN A LAN GAME", lines, AMBER);
            return;
        }
        let hosts = self.browser.as_ref().map(|b| b.hosts()).unwrap_or_default();
        if hosts.is_empty() {
            let dots = ".".repeat(1 + (self.time * 2.0) as usize % 3);
            lines.push(Line::styled(format!("Looking for games on the local network{dots}"), Style::new().fg(PAPER)));
            lines.push(Line::from(""));
            lines.push(Line::styled("On the other machine: Host a LAN game.", Style::new().fg(DIM)));
        } else {
            for (k, h) in hosts.iter().enumerate() {
                let sel = k == self.browse_pick.min(hosts.len() - 1);
                let state = if h.beacon.in_game { "playing" } else if h.beacon.players >= 2 { "full" } else { "waiting for you" };
                let style = if sel { Style::new().fg(INK).bg(AMBER).add_modifier(Modifier::BOLD) } else { Style::new().fg(PAPER) };
                lines.push(Line::styled(format!(" {}'s game  ({state}) ", h.beacon.name), style));
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::styled("Up/Down: choose    Space: join    Esc: back", Style::new().fg(DIM)));
        card(frame, full, "JOIN A LAN GAME", lines, AMBER);
    }

    fn draw_hud(&self, frame: &mut Frame, full: Rect) {
        let w = &self.world;
        let me = self.me.min(w.tanks.len() - 1);
        let p = &w.tanks[me];
        let spec = p.spec();
        let shells = spec.max_shells - w.shells_of(me).min(spec.max_shells);
        let mines = spec.max_mines - w.mines_of(me).min(spec.max_mines);
        let total = w.enemies().len() + self.gone_count();
        let left = w.enemies_left();
        let m = &self.missions[self.mission];
        let mut spans = vec![
            Span::styled(" PANZER ", Style::new().fg(INK).bg(AMBER).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  Mission {}: {}  ", self.mission + 1, m.name), Style::new().fg(PAPER)),
            Span::styled("Tanks ", Style::new().fg(DIM)),
            Span::styled("■".repeat(self.lives as usize), Style::new().fg(Color::Rgb(84, 134, 236))),
            Span::styled("   Enemies ", Style::new().fg(DIM)),
            Span::styled(format!("{left}/{total}"), Style::new().fg(PAPER)),
            Span::styled("   Shells ", Style::new().fg(DIM)),
            Span::styled("●".repeat(shells), Style::new().fg(PAPER)),
            Span::styled("○".repeat(spec.max_shells - shells), Style::new().fg(DIM)),
            Span::styled("   Mines ", Style::new().fg(DIM)),
            Span::styled("◆".repeat(mines), Style::new().fg(Color::Rgb(236, 194, 44))),
            Span::styled("◇".repeat(spec.max_mines - mines), Style::new().fg(DIM)),
        ];
        if self.phase == Phase::Play && w.time < 1.5 {
            spans.push(Span::styled("   GO!", Style::new().fg(GREEN).add_modifier(Modifier::BOLD)));
        }
        frame.render_widget(Paragraph::new(Line::from(spans)).style(Style::new().bg(INK)), Rect { height: 1, ..full });
        let hint = "WASD move   mouse/IJKL aim   click/Space fire   right-click/F mine   P pause   H help   M music   F8 sound";
        frame.render_widget(Paragraph::new(Line::styled(hint, Style::new().fg(DIM))).style(Style::new().bg(INK)), Rect { y: full.y + full.height - 1, height: 1, ..full });
    }

    fn intro_card(&self, frame: &mut Frame, full: Rect) {
        let m = &self.missions[self.mission];
        let mut counts = [0usize; 10];
        for t in self.world.enemies() {
            counts[t.kind.min(9)] += 1;
        }
        let seen_before: Vec<bool> = (0..10).map(|k| self.missions[..self.mission].iter().any(|m| m.enemies.iter().any(|e| e.0 == k))).collect();
        let mut lines = vec![
            Line::styled(m.name.clone(), Style::new().fg(PAPER).add_modifier(Modifier::BOLD)),
            Line::from(""),
            Line::styled(m.brief.clone(), Style::new().fg(PAPER)),
            Line::from(""),
            Line::from(format!("Enemy tanks: {}", self.world.enemies().len())),
            Line::from(""),
        ];
        for k in 1..10 {
            if counts[k] == 0 {
                continue;
            }
            let s = &SPECS[k];
            let mut row = vec![
                Span::styled("██ ", Style::new().fg(Color::Rgb(s.body.0, s.body.1, s.body.2))),
                Span::styled(format!("{:<8}x{} ", s.name, counts[k]), Style::new().fg(PAPER).add_modifier(Modifier::BOLD)),
                Span::styled(s.blurb, Style::new().fg(DIM)),
            ];
            if !seen_before[k] {
                row.push(Span::styled("  NEW", Style::new().fg(AMBER).add_modifier(Modifier::BOLD)));
            }
            lines.push(Line::from(row));
        }
        lines.push(Line::from(""));
        if self.world.players > 1 {
            lines.push(Line::styled("Two players: you share the spare tanks; the mission fails when both tanks are down.", Style::new().fg(GREEN)));
        }
        lines.push(Line::from(vec![Span::styled("Tanks left: ", Style::new().fg(DIM)), Span::styled("■".repeat(self.lives as usize), Style::new().fg(Color::Rgb(84, 134, 236)))]));
        lines.push(Line::styled("Space to start", Style::new().fg(DIM)));
        card(frame, full, &format!("MISSION {}", self.mission + 1), lines, AMBER);
    }

    fn results_card(&self, frame: &mut Frame, full: Rect) {
        let title = if self.victory { "CAMPAIGN COMPLETE" } else { "GAME OVER" };
        let mut lines = vec![Line::from("")];
        if self.victory {
            lines.push(Line::styled("All twenty missions cleared. The toy box is yours.", Style::new().fg(GREEN)));
        } else {
            lines.push(Line::from(format!("You reached mission {}: {}", self.mission + 1, self.missions[self.mission].name)));
        }
        lines.push(Line::from(""));
        lines.push(Line::styled("Tanks destroyed", Style::new().fg(PAPER).add_modifier(Modifier::BOLD)));
        let mut total = 0;
        for k in 1..10 {
            if self.tally[k] == 0 {
                continue;
            }
            total += self.tally[k];
            let s = &SPECS[k];
            lines.push(Line::from(vec![
                Span::styled("██ ", Style::new().fg(Color::Rgb(s.body.0, s.body.1, s.body.2))),
                Span::styled(format!("{:<8} {:>3}", s.name, self.tally[k]), Style::new().fg(PAPER)),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::styled(format!("Total {total}"), Style::new().fg(AMBER).add_modifier(Modifier::BOLD)));
        lines.push(Line::from(""));
        lines.push(Line::styled("Space: back to the title", Style::new().fg(DIM)));
        card(frame, full, title, lines, if self.victory { GREEN } else { AMBER });
    }

    fn draw_title_text(&self, frame: &mut Frame, full: Rect) {
        let m = &self.missions[self.pick];
        let mut lines = vec![Line::styled("Toy tanks. Real ricochets.", Style::new().fg(PAPER).add_modifier(Modifier::BOLD)), Line::from("")];
        for (k, item) in MENU.iter().enumerate() {
            let sel = k == self.menu;
            let text = if k == 0 { format!("{item}: mission {}, {}", self.pick + 1, m.name) } else { item.to_string() };
            let style = if sel { Style::new().fg(INK).bg(AMBER).add_modifier(Modifier::BOLD) } else { Style::new().fg(PAPER) };
            let arrows = sel && k <= 1;
            lines.push(Line::from(vec![
                Span::styled(if arrows { "◀ " } else { "  " }, Style::new().fg(AMBER)),
                Span::styled(format!(" {text} "), style),
                Span::styled(if arrows { " ▶" } else { "  " }, Style::new().fg(AMBER)),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::styled("Up/Down: choose    Left/Right: mission    Space: go    H: help    Esc: quit", Style::new().fg(DIM)));
        lines.push(Line::styled("An Asciiverse game, after Wii Play Tanks (Nintendo, 2006)", Style::new().fg(DIM)));
        let h = lines.len() as u16 + 2;
        let w = 84.min(full.width);
        let area = Rect { x: full.x + (full.width - w) / 2, y: full.y + full.height * 46 / 100, width: w, height: h.min(full.height) };
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(lines).centered().block(Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(AMBER))).style(Style::new().bg(INK)),
            area,
        );
    }
}

fn card(frame: &mut Frame, full: Rect, title: &str, lines: Vec<Line>, accent: Color) {
    let w = 78.min(full.width.saturating_sub(4));
    let inner_w = w.saturating_sub(4).max(1);
    // count wrapped lines roughly so the card fits its text
    let h: u16 = lines.iter().map(|l| (l.width() as u16).div_ceil(inner_w).max(1)).sum::<u16>() + 2;
    let h = h.min(full.height.saturating_sub(2));
    let area = Rect { x: full.x + (full.width - w) / 2, y: full.y + (full.height.saturating_sub(h)) / 2, width: w, height: h };
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(accent))
        .title(Line::styled(format!(" {title} "), Style::new().fg(accent).add_modifier(Modifier::BOLD)).centered())
        .padding(ratatui::widgets::Padding::horizontal(1));
    frame.render_widget(Paragraph::new(lines).centered().wrap(Wrap { trim: true }).block(block).style(Style::new().bg(INK).fg(PAPER)), area);
}

fn help_card(frame: &mut Frame, full: Rect) {
    let k = |key: &str, what: &str| Line::from(vec![Span::styled(format!("{key:>22}  "), Style::new().fg(AMBER)), Span::styled(what.to_string(), Style::new().fg(PAPER))]);
    let lines = vec![
        Line::from(""),
        k("WASD / arrows", "drive (left stick)"),
        k("mouse / IJKL", "aim the turret (right stick)"),
        k("Q / E", "turn the turret slowly"),
        k("click / Space", "fire (right trigger); five shells at most"),
        k("right-click / F", "drop a mine (left trigger); two at most"),
        k("P / Esc", "pause"),
        k("M / F8", "music on/off, all sound on/off"),
        Line::from(""),
        Line::styled("Shells bounce off walls once. Shoot incoming shells to cancel them.", Style::new().fg(DIM)),
        Line::styled("Mines go off after ten seconds, when a tank drives close, or when shot.", Style::new().fg(DIM)),
        Line::styled("They break crates. Pits stop tanks but not shells.", Style::new().fg(DIM)),
        Line::styled("A spare tank every five missions.", Style::new().fg(DIM)),
        Line::styled("Two players: one picks Host a LAN game, the other Join a LAN game.", Style::new().fg(DIM)),
        Line::from(""),
        Line::styled("Esc or Space to close", Style::new().fg(DIM)),
    ];
    card(frame, full, "HOW TO PLAY", lines, GREEN);
}
