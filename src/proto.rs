//! Wire messages between a Panzer host and its guest.
//!
//! The host runs the only simulation. The guest sends its controls every
//! tick; the host sends back a `Frame` every tick: where the campaign is
//! (`GameState`) and the battle as it stands (`world::Snapshot`), with the
//! events since the last frame so the guest can play the same sounds and
//! draw the same smoke, sparks and blasts.
//!
//! Encoding is postcard (serde) inside u32-length-prefixed frames, as in Tetrix.

use crate::game::Phase;
use crate::world::{Controls, Snapshot};
use serde::{Deserialize, Serialize};

/// Bumped on any incompatible change; the host rejects mismatched guests.
pub const VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GameState {
    pub phase: Phase,
    pub phase_t: f32,
    pub mission: u16,
    /// Bumped every time a battle (re)starts, so the guest rebuilds its world.
    pub attempt: u32,
    pub players: u8,
    pub lives: u32,
    pub bonus: bool,
    pub victory: bool,
    pub tally: [u32; 10],
    /// Enemies already destroyed in this mission before the last retry.
    pub gone: u16,
    pub host_name: String,
    pub guest_name: Option<String>,
}

/// A guest's menu press, handled by the host as if it were its own.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Confirm,
    Pause,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Msg {
    /// Guest's first message.
    Hello { name: String, version: u32 },
    Reject { reason: String },
    Frame { game: GameState, snap: Option<Snapshot> },
    Input(Controls),
    Press(Press),
    /// Keeps a quiet connection from timing out.
    Ping,
}

pub fn encode(m: &Msg) -> Vec<u8> {
    postcard::to_allocvec(m).expect("postcard encoding of an owned message cannot fail")
}

pub fn decode(b: &[u8]) -> Option<Msg> {
    postcard::from_bytes(b).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{parse_missions, MISSIONS_TXT};
    use crate::world::{Event, World};

    #[test]
    fn round_trip_and_size() {
        let missions = parse_missions(MISSIONS_TXT).unwrap();
        let mut w = World::new(&missions[19], &[], 1, 2);
        for _ in 0..120 {
            w.step(1.0 / 60.0, &[Controls { fire: true, ..Default::default() }; 2]);
        }
        let game = GameState {
            phase: Phase::Play,
            phase_t: 1.5,
            mission: 19,
            attempt: 3,
            players: 2,
            lives: 2,
            bonus: false,
            victory: false,
            tally: [1; 10],
            gone: 2,
            host_name: "dåvid".into(),
            guest_name: Some("guest".into()),
        };
        let snap = w.snapshot(vec![Event::TankDown { kind: 3, x: 1.0, y: 2.0 }]);
        let bytes = encode(&Msg::Frame { game: game.clone(), snap: Some(snap.clone()) });
        // sent 60 times a second: keep it well under a few kB
        assert!(bytes.len() < 2500, "frame is {} bytes", bytes.len());
        match decode(&bytes) {
            Some(Msg::Frame { game: g, snap: Some(s) }) => {
                assert_eq!(g, game);
                assert_eq!(s.tanks.len(), snap.tanks.len());
                assert_eq!(s.events, snap.events);
            }
            other => panic!("decoded {other:?}"),
        }
        assert!(decode(&[0xff, 0xff]).is_none());
    }
}
