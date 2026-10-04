//! Keyboard, mouse and gamepad.
//!
//! - Move: WASD or the arrow keys (eight ways), or the left stick.
//! - Aim: the mouse pointer, or the right stick, or IJKL (eight ways), or
//!   Q/E to turn the turret slowly.
//! - Fire: left click, Space, or the right trigger/bumper.
//! - Mine: right click, F, or the left trigger/bumper or A.
//!
//! With the kitty keyboard protocol presses and releases are exact. Elsewhere
//! a key counts as held while press/repeat events keep arriving (as in
//! Calvary): a tap holds for a moment, then repeats keep it alive.
//!
//! Gamepads: only pads in SDL's controller database count (gilrs also
//! reports 3D mice and tablet key pads as gamepads); `PANZER_ANY_PAD=1`
//! accepts everything.

use crate::world::Controls;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::time::{Duration, Instant};

const PRE_REPEAT: Duration = Duration::from_millis(220);
const REPEAT_GAP: Duration = Duration::from_millis(110);
#[cfg(feature = "gamepad")]
const STICK_DEADZONE: f32 = 0.2;
const AIM_THRESHOLD: f32 = 0.45;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Key {
    Up,
    Down,
    Left,
    Right,
    AimUp,
    AimDown,
    AimLeft,
    AimRight,
    TurnLeft,
    TurnRight,
}
const KEYS: usize = 10;

#[cfg(feature = "gamepad")]
fn usable_pad(p: &gilrs::Gamepad) -> bool {
    p.mapping_source() == gilrs::MappingSource::SdlMappings || std::env::var("PANZER_ANY_PAD").is_ok_and(|v| v == "1")
}

#[derive(Clone, Copy, Default)]
struct Hold {
    down: bool,
    last: Option<Instant>,
    repeats: u32,
}

impl Hold {
    fn held(&self, exact: bool, now: Instant) -> bool {
        if exact {
            return self.down;
        }
        let Some(last) = self.last else { return false };
        let window = if self.repeats == 0 { PRE_REPEAT } else { REPEAT_GAP };
        now.duration_since(last) <= window
    }
}

/// Menu and system commands, as edges.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cmd {
    Confirm,
    Back,
    Pause,
    Left,
    Right,
    Up,
    Down,
    Help,
    Mute,
    Music,
    Exit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AimBy {
    Mouse,
    Keys,
    Pad,
}

pub struct Input {
    exact: bool,
    holds: [Hold; KEYS],
    fire_edge: bool,
    mine_edge: bool,
    pub cmds: Vec<Cmd>,
    aim_by: AimBy,
    /// Mouse pointer cell.
    pub mouse: Option<(u16, u16)>,
    /// Last aim direction from keys or pad, kept when they are released.
    aim_dir: Option<f32>,
    #[cfg(feature = "gamepad")]
    gilrs: Option<gilrs::Gilrs>,
    pub pad_name: Option<String>,
    pad_move: (f32, f32),
    pad_aim: (f32, f32),
    /// The stick moved last (else the keys).
    move_by_pad: bool,
}

impl Input {
    pub fn new(exact: bool) -> Input {
        Input {
            exact,
            holds: [Hold::default(); KEYS],
            fire_edge: false,
            mine_edge: false,
            cmds: Vec::new(),
            aim_by: AimBy::Mouse,
            mouse: None,
            aim_dir: None,
            #[cfg(feature = "gamepad")]
            gilrs: None,
            pad_name: None,
            pad_move: (0.0, 0.0),
            pad_aim: (0.0, 0.0),
            move_by_pad: false,
        }
    }

    pub fn aiming_with_mouse(&self) -> bool {
        self.aim_by == AimBy::Mouse && self.mouse.is_some()
    }

    pub fn on_key(&mut self, k: KeyEvent, now: Instant) {
        let press = k.kind != KeyEventKind::Release;
        let fresh = k.kind == KeyEventKind::Press;
        let held = match k.code {
            KeyCode::Char('w' | 'W') | KeyCode::Up => Some(Key::Up),
            KeyCode::Char('s' | 'S') | KeyCode::Down => Some(Key::Down),
            KeyCode::Char('a' | 'A') | KeyCode::Left => Some(Key::Left),
            KeyCode::Char('d' | 'D') | KeyCode::Right => Some(Key::Right),
            KeyCode::Char('i' | 'I') => Some(Key::AimUp),
            KeyCode::Char('k' | 'K') => Some(Key::AimDown),
            KeyCode::Char('j' | 'J') => Some(Key::AimLeft),
            KeyCode::Char('l' | 'L') => Some(Key::AimRight),
            KeyCode::Char('q' | 'Q') => Some(Key::TurnLeft),
            KeyCode::Char('e' | 'E') => Some(Key::TurnRight),
            _ => None,
        };
        if let Some(key) = held {
            let h = &mut self.holds[key as usize];
            if self.exact {
                h.down = press;
            } else {
                // without release events every repeat arrives as a press
                if h.held(false, now) {
                    h.repeats += 1;
                } else {
                    h.repeats = 0;
                }
                h.last = Some(now);
            }
            if key as usize >= Key::AimUp as usize && press {
                self.aim_by = AimBy::Keys;
            }
            if key as usize <= Key::Right as usize {
                self.move_by_pad = false;
            }
            if fresh {
                match key {
                    Key::Up => self.cmds.push(Cmd::Up),
                    Key::Down => self.cmds.push(Cmd::Down),
                    Key::Left => self.cmds.push(Cmd::Left),
                    Key::Right => self.cmds.push(Cmd::Right),
                    _ => {}
                }
            }
            return;
        }
        if !fresh {
            return;
        }
        match k.code {
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => self.cmds.push(Cmd::Exit),
            KeyCode::Char(' ') => {
                self.fire_edge = true;
                self.cmds.push(Cmd::Confirm);
            }
            KeyCode::Char('f' | 'F' | 'x' | 'X') => self.mine_edge = true,
            KeyCode::Enter => self.cmds.push(Cmd::Confirm),
            KeyCode::Esc => self.cmds.push(Cmd::Back),
            KeyCode::Char('p' | 'P') => self.cmds.push(Cmd::Pause),
            KeyCode::F(1) | KeyCode::Char('h' | 'H' | '?') => self.cmds.push(Cmd::Help),
            KeyCode::F(8) => self.cmds.push(Cmd::Mute),
            KeyCode::Char('m' | 'M') => self.cmds.push(Cmd::Music),
            _ => {}
        }
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        match m.kind {
            MouseEventKind::Moved | MouseEventKind::Drag(_) => {
                self.mouse = Some((m.column, m.row));
                self.aim_by = AimBy::Mouse;
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.mouse = Some((m.column, m.row));
                self.aim_by = AimBy::Mouse;
                self.fire_edge = true;
            }
            MouseEventKind::Down(MouseButton::Right) => self.mine_edge = true,
            _ => {}
        }
    }

    pub fn open_pad(&mut self) {
        #[cfg(feature = "gamepad")]
        match gilrs::Gilrs::new() {
            Ok(g) => {
                let all: Vec<_> = g.gamepads().map(|(_, p)| (p.name().to_string(), usable_pad(&p))).collect();
                log::info!("gamepads (name, used): {all:?}");
                self.pad_name = all.into_iter().find(|(_, used)| *used).map(|(n, _)| n);
                self.gilrs = Some(g);
            }
            Err(e) => log::warn!("gamepad support unavailable: {e}"),
        }
    }

    /// Drain gamepad events; call once per frame.
    pub fn poll_pad(&mut self) {
        #[cfg(feature = "gamepad")]
        {
            use gilrs::{Axis, Button, EventType};
            let Some(g) = &mut self.gilrs else { return };
            let mut events = Vec::new();
            while let Some(ev) = g.next_event() {
                events.push(ev);
            }
            for ev in events {
                let g = self.gilrs.as_ref().unwrap();
                let pad = g.gamepad(ev.id);
                if !usable_pad(&pad) {
                    continue;
                }
                match ev.event {
                    EventType::Connected => {
                        self.pad_name = Some(pad.name().to_string());
                        log::info!("gamepad connected: {:?}", self.pad_name);
                    }
                    EventType::Disconnected => {
                        log::info!("gamepad disconnected");
                        self.pad_name = None;
                    }
                    EventType::ButtonPressed(b, _) => {
                        log::debug!("pad button {b:?}");
                        match b {
                            Button::RightTrigger2 | Button::RightTrigger => self.fire_edge = true,
                            Button::LeftTrigger2 | Button::LeftTrigger | Button::West => self.mine_edge = true,
                            Button::South => {
                                self.mine_edge = true;
                                self.cmds.push(Cmd::Confirm);
                            }
                            Button::East => self.cmds.push(Cmd::Back),
                            Button::Start => self.cmds.push(Cmd::Pause),
                            Button::Select => self.cmds.push(Cmd::Help),
                            Button::DPadLeft => self.cmds.push(Cmd::Left),
                            Button::DPadRight => self.cmds.push(Cmd::Right),
                            Button::DPadUp => self.cmds.push(Cmd::Up),
                            Button::DPadDown => self.cmds.push(Cmd::Down),
                            _ => {}
                        }
                    }
                    _ => {}
                }
                let axis = |a: Axis| pad.axis_data(a).map_or(0.0, |d| d.value());
                let dead = |(x, y): (f32, f32)| if x.hypot(y) < STICK_DEADZONE { (0.0, 0.0) } else { (x, y) };
                let mv = dead((axis(Axis::LeftStickX), axis(Axis::LeftStickY)));
                if mv != (0.0, 0.0) {
                    self.move_by_pad = true;
                }
                self.pad_move = mv;
                self.pad_aim = dead((axis(Axis::RightStickX), axis(Axis::RightStickY)));
                if self.pad_aim.0.hypot(self.pad_aim.1) >= AIM_THRESHOLD {
                    self.aim_by = AimBy::Pad;
                }
            }
        }
    }

    /// Drop held keys and edges, e.g. between screens.
    pub fn clear(&mut self) {
        self.fire_edge = false;
        self.mine_edge = false;
        self.cmds.clear();
    }

    /// Controls for one step; `mouse_world` is the pointer on the map, if over it.
    pub fn controls(&mut self, now: Instant, mouse_world: Option<(f32, f32)>) -> Controls {
        let held = |k: Key| self.holds[k as usize].held(self.exact, now);
        let axis = |neg: Key, pos: Key| held(pos) as i8 as f32 - held(neg) as i8 as f32;
        let keys_mv = (axis(Key::Left, Key::Right), axis(Key::Up, Key::Down));
        let mv = if keys_mv != (0.0, 0.0) || !self.move_by_pad {
            let l = keys_mv.0.hypot(keys_mv.1).max(1.0);
            (keys_mv.0 / l, keys_mv.1 / l)
        } else {
            // stick up is positive, map up is negative y
            (self.pad_move.0, -self.pad_move.1)
        };
        let keys_aim = (axis(Key::AimLeft, Key::AimRight), axis(Key::AimUp, Key::AimDown));
        if keys_aim != (0.0, 0.0) {
            self.aim_dir = Some(keys_aim.1.atan2(keys_aim.0));
        }
        let (ax, ay) = self.pad_aim;
        if self.aim_by == AimBy::Pad && ax.hypot(ay) >= AIM_THRESHOLD {
            self.aim_dir = Some((-ay).atan2(ax));
        }
        let turret_turn = axis(Key::TurnLeft, Key::TurnRight);
        if turret_turn != 0.0 {
            self.aim_by = AimBy::Keys;
            self.aim_dir = None;
        }
        let (aim_at, aim_dir) = match self.aim_by {
            AimBy::Mouse => (mouse_world, None),
            _ => (None, self.aim_dir),
        };
        Controls { mv, aim_at, aim_dir, turret_turn, fire: std::mem::take(&mut self.fire_edge), mine: std::mem::take(&mut self.mine_edge) }
    }
}
