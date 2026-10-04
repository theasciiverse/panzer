//! The tank roster and the enemy brains.
//!
//! Every enemy should be readable from how it moves (and sounds) before you
//! see its colour: the Tinpot never moves, the Skitter never stops, the
//! Hornet keeps its distance, the Rascal comes straight at you.
//!
//! Aim works like the turret is the brain: each frame the AI traces where a
//! shell fired right now would go, bounces included, and fires when that path
//! ends in the player and not in a friend.

use crate::world::{wrap, Rgb, Rng, World, TANK_R};
use std::f32::consts::{PI, TAU};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Move {
    Still,
    /// Pick a nearby spot, drive there, pick another.
    Wander,
    /// Close in to this many tiles, then circle.
    Chase(f32),
    /// Hold this distance, backing off when crowded.
    Keep(f32),
    /// Short nervous dashes, away from the player when close.
    Skitter,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Aim {
    /// Turret wanders around the player's general direction; fires when a
    /// sweep happens to line up, often off a wall.
    Sweep,
    /// Points at the player, leading a moving target.
    Direct,
    /// Searches every angle for a path that ricochets into the player.
    Bank,
}

pub struct Spec {
    pub name: &'static str,
    pub blurb: &'static str,
    pub body: Rgb,
    pub turret: Rgb,
    /// Tiles per second; 0 is a fixed emplacement.
    pub speed: f32,
    /// Hull turn rate, radians per second.
    pub turn: f32,
    pub turret_rate: f32,
    /// Seconds between shots (random in range).
    pub reload: (f32, f32),
    pub max_shells: usize,
    pub shell_speed: f32,
    pub bounces: i32,
    pub rocket: bool,
    /// Shells per trigger pull.
    pub burst: u32,
    /// Chance per second of laying a mine.
    pub mines: f32,
    pub max_mines: usize,
    pub moves: Move,
    pub aim: Aim,
    /// Chance of noticing an incoming shell and swerving.
    pub dodge: f32,
    pub invisible: bool,
    /// Aim error, radians.
    pub noise: f32,
}

const SHELL: f32 = 3.8;
const ROCKET: f32 = 7.2;

/// 0 is the player; 1-9 are the enemy roster in order of menace, and the
/// digits in the mission maps; 10 is the second player.
pub static SPECS: [Spec; 11] = [
    Spec {
        name: "You",
        blurb: "",
        body: (52, 98, 206),
        turret: (84, 134, 236),
        speed: 1.95,
        turn: 9.0,
        turret_rate: 14.0,
        reload: (0.12, 0.12),
        max_shells: 5,
        shell_speed: SHELL,
        bounces: 1,
        rocket: false,
        burst: 1,
        mines: 0.0,
        max_mines: 2,
        moves: Move::Still,
        aim: Aim::Direct,
        dodge: 0.0,
        invisible: false,
        noise: 0.0,
    },
    Spec {
        name: "Tinpot",
        blurb: "Bolted to the floor. Sweeps its turret and hopes.",
        body: (150, 92, 44),
        turret: (184, 120, 64),
        speed: 0.0,
        turn: 0.0,
        turret_rate: 1.1,
        reload: (2.4, 4.0),
        max_shells: 1,
        shell_speed: 3.3,
        bounces: 1,
        rocket: false,
        burst: 1,
        mines: 0.0,
        max_mines: 0,
        moves: Move::Still,
        aim: Aim::Sweep,
        dodge: 0.0,
        invisible: false,
        noise: 0.0,
    },
    Spec {
        name: "Plodder",
        blurb: "Slow, steady, wanders about. Shoots when it remembers to.",
        body: (128, 132, 140),
        turret: (160, 164, 172),
        speed: 0.85,
        turn: 3.0,
        turret_rate: 1.6,
        reload: (2.0, 3.2),
        max_shells: 1,
        shell_speed: SHELL,
        bounces: 1,
        rocket: false,
        burst: 1,
        mines: 0.0,
        max_mines: 0,
        moves: Move::Wander,
        aim: Aim::Sweep,
        dodge: 0.2,
        invisible: false,
        noise: 0.0,
    },
    Spec {
        name: "Hornet",
        blurb: "Keeps its distance and fires rockets: fast, but no ricochet.",
        body: (34, 146, 150),
        turret: (70, 190, 190),
        speed: 1.0,
        turn: 3.5,
        turret_rate: 2.4,
        reload: (2.4, 3.4),
        max_shells: 1,
        shell_speed: ROCKET,
        bounces: 0,
        rocket: true,
        burst: 1,
        mines: 0.0,
        max_mines: 0,
        moves: Move::Keep(7.0),
        aim: Aim::Direct,
        dodge: 0.4,
        invisible: false,
        noise: 0.08,
    },
    Spec {
        name: "Skitter",
        blurb: "Nervous and quick. Leaves mines behind when it runs.",
        body: (226, 192, 40),
        turret: (250, 222, 90),
        speed: 2.3,
        turn: 8.0,
        turret_rate: 2.2,
        reload: (1.6, 2.6),
        max_shells: 1,
        shell_speed: SHELL,
        bounces: 1,
        rocket: false,
        burst: 1,
        mines: 0.35,
        max_mines: 2,
        moves: Move::Skitter,
        aim: Aim::Sweep,
        dodge: 0.8,
        invisible: false,
        noise: 0.0,
    },
    Spec {
        name: "Rascal",
        blurb: "Comes straight for you and fires in threes.",
        body: (222, 108, 160),
        turret: (246, 152, 196),
        speed: 1.5,
        turn: 5.0,
        turret_rate: 2.6,
        reload: (1.3, 2.0),
        max_shells: 3,
        shell_speed: SHELL,
        bounces: 1,
        rocket: false,
        burst: 3,
        mines: 0.0,
        max_mines: 0,
        moves: Move::Chase(4.0),
        aim: Aim::Direct,
        dodge: 0.45,
        invisible: false,
        noise: 0.1,
    },
    Spec {
        name: "Sniper",
        blurb: "Never moves. Finds the angle off two walls that you forgot.",
        body: (58, 140, 62),
        turret: (96, 184, 92),
        speed: 0.0,
        turn: 0.0,
        turret_rate: 1.5,
        reload: (2.6, 3.6),
        max_shells: 1,
        shell_speed: 6.0,
        bounces: 2,
        rocket: true,
        burst: 1,
        mines: 0.0,
        max_mines: 0,
        moves: Move::Still,
        aim: Aim::Bank,
        dodge: 0.0,
        invisible: false,
        noise: 0.0,
    },
    Spec {
        name: "Viper",
        blurb: "Fast, slippery, banks shots and lays the odd mine.",
        body: (132, 74, 186),
        turret: (170, 116, 220),
        speed: 2.1,
        turn: 7.0,
        turret_rate: 3.4,
        reload: (0.6, 1.1),
        max_shells: 4,
        shell_speed: 4.2,
        bounces: 1,
        rocket: false,
        burst: 1,
        mines: 0.06,
        max_mines: 1,
        moves: Move::Chase(5.0),
        aim: Aim::Bank,
        dodge: 0.9,
        invisible: false,
        noise: 0.0,
    },
    Spec {
        name: "Ghost",
        blurb: "Invisible. Watch the tread marks; listen for the hum.",
        body: (226, 228, 236),
        turret: (250, 250, 255),
        speed: 1.3,
        turn: 4.0,
        turret_rate: 2.2,
        reload: (1.6, 2.4),
        max_shells: 2,
        shell_speed: SHELL,
        bounces: 1,
        rocket: false,
        burst: 1,
        mines: 0.03,
        max_mines: 1,
        moves: Move::Wander,
        aim: Aim::Direct,
        dodge: 0.6,
        invisible: true,
        noise: 0.12,
    },
    Spec {
        name: "Onyx",
        blurb: "The big black one. Charges, dodges, fires rockets.",
        body: (40, 40, 48),
        turret: (78, 78, 92),
        speed: 2.5,
        turn: 7.5,
        turret_rate: 4.0,
        reload: (0.8, 1.2),
        max_shells: 2,
        shell_speed: 7.6,
        bounces: 0,
        rocket: true,
        burst: 1,
        mines: 0.04,
        max_mines: 1,
        moves: Move::Chase(2.5),
        aim: Aim::Direct,
        dodge: 1.0,
        invisible: false,
        noise: 0.03,
    },
    Spec {
        name: "Player 2",
        blurb: "",
        body: (204, 52, 46),
        turret: (236, 92, 80),
        speed: 1.95,
        turn: 9.0,
        turret_rate: 14.0,
        reload: (0.12, 0.12),
        max_shells: 5,
        shell_speed: SHELL,
        bounces: 1,
        rocket: false,
        burst: 1,
        mines: 0.0,
        max_mines: 2,
        moves: Move::Still,
        aim: Aim::Direct,
        dodge: 0.0,
        invisible: false,
        noise: 0.0,
    },
];

pub struct Brain {
    rng: Rng,
    goal: Option<(i32, i32)>,
    field: Vec<u16>,
    goal_t: f32,
    sweep: f32,
    sweep_t: f32,
    bank: Option<f32>,
    bank_t: f32,
    noise: f32,
    noise_t: f32,
    strafe: f32,
    strafe_t: f32,
    hesitate: f32,
}

impl Brain {
    pub fn new(seed: u64) -> Brain {
        let mut rng = Rng::new(seed);
        let strafe = if rng.f32() < 0.5 { 1.0 } else { -1.0 };
        Brain {
            rng,
            goal: None,
            field: Vec::new(),
            goal_t: 0.0,
            sweep: 0.0,
            sweep_t: 0.0,
            bank: None,
            bank_t: 0.0,
            noise: 0.0,
            noise_t: 0.0,
            strafe,
            strafe_t: 2.0,
            hesitate: 0.0,
        }
    }
}

pub struct Decision {
    pub mv: (f32, f32),
    pub aim: f32,
    pub fire: bool,
    pub mine: bool,
}

pub fn think(w: &World, b: &mut Brain, i: usize, dt: f32) -> Decision {
    let me = &w.tanks[i];
    let spec = me.spec();
    // go for the nearest player still in the fight
    let target = (0..w.players)
        .filter(|&j| w.tanks[j].alive)
        .min_by(|&a, &b| {
            let d = |j: usize| (w.tanks[j].x - me.x).hypot(w.tanks[j].y - me.y);
            d(a).total_cmp(&d(b))
        })
        .unwrap_or(0);
    let p = &w.tanks[target];
    let hunting = p.alive && w.won_t.is_none();
    let at_player = |hit: Option<usize>| hit.is_some_and(|j| w.is_player(j));
    let (dx, dy) = (p.x - me.x, p.y - me.y);
    let to_p = dy.atan2(dx);
    let dist = dx.hypot(dy);

    // ---- where the turret wants to be
    b.noise_t -= dt;
    if b.noise_t <= 0.0 {
        b.noise_t = b.rng.range(0.6, 1.4);
        b.noise = b.rng.range(-1.0, 1.0) * spec.noise;
    }
    let direct = {
        // lead the target: where will it be when the shell gets there?
        let t = dist / spec.shell_speed;
        let (lx, ly) = (p.x + p.vx * t * 0.8, p.y + p.vy * t * 0.8);
        (ly - me.y).atan2(lx - me.x) + b.noise
    };
    let aim = match spec.aim {
        Aim::Sweep => {
            b.sweep_t -= dt;
            if b.sweep_t <= 0.0 {
                b.sweep_t = b.rng.range(1.0, 2.4);
                b.sweep = to_p + b.rng.range(-1.4, 1.4);
            }
            b.sweep
        }
        Aim::Direct => direct,
        Aim::Bank => {
            b.bank_t -= dt;
            if b.bank_t <= 0.0 {
                b.bank_t = b.rng.range(0.3, 0.5);
                b.bank = bank_shot(w, i, me.turret);
            }
            b.bank.unwrap_or(direct)
        }
    };

    // ---- whether to pull the trigger
    b.hesitate = (b.hesitate - dt).max(0.0);
    let mut fire = false;
    if hunting && me.cooldown <= 0.0 && me.burst == 0 && w.shells_of(i) < spec.max_shells && b.hesitate <= 0.0 {
        let (mx, my) = (me.x, me.y);
        if at_player(w.trace(mx, my, me.turret, spec.bounces, i, 30.0)) {
            fire = true;
        } else if spec.aim == Aim::Direct && wrap(me.turret - aim).abs() < 0.08 && dist < 9.0 && w.trace(mx, my, me.turret, spec.bounces, i, 30.0).is_none() {
            // a speculative shot down a clear line (leading a moving target)
            fire = b.rng.f32() < dt * 1.5;
        }
        if fire && b.rng.f32() < 0.15 {
            // even good gunners blink sometimes
            fire = false;
            b.hesitate = 0.3;
        }
    }

    // ---- where to drive
    let mut mv = (0.0f32, 0.0f32);
    let pf = &w.player_field;
    let my_tile = (me.x as i32, me.y as i32);
    let steps = pf[my_tile.1 as usize * crate::arena::GW + my_tile.0 as usize];
    let steps = if steps == u16::MAX { 99.0 } else { steps as f32 };
    b.strafe_t -= dt;
    if b.strafe_t <= 0.0 {
        b.strafe_t = b.rng.range(1.2, 3.0);
        b.strafe = -b.strafe;
    }
    let strafe = ((-to_p.sin()) * b.strafe * 0.7, to_p.cos() * b.strafe * 0.7);
    match spec.moves {
        Move::Still => {}
        Move::Wander => {
            mv = wander(w, b, i, dt, 2, 7, (3.0, 7.0)).map_or((0.0, 0.0), |(x, y)| (x * 0.85, y * 0.85));
        }
        Move::Chase(stop) => {
            if !hunting {
                mv = wander(w, b, i, dt, 2, 6, (2.0, 4.0)).unwrap_or_default();
            } else if steps > stop || !at_player(w.trace(me.x, me.y, to_p, 0, i, 30.0)) {
                mv = w.flow(pf, me.x, me.y, false).unwrap_or_default();
            } else {
                mv = strafe;
            }
        }
        Move::Keep(range) => {
            if hunting && steps < range - 1.5 {
                mv = w.flow(pf, me.x, me.y, true).unwrap_or(strafe);
            } else if hunting && steps > range + 2.0 {
                mv = w.flow(pf, me.x, me.y, false).unwrap_or_default();
            } else {
                mv = wander(w, b, i, dt, 1, 3, (1.5, 3.0)).map_or(strafe, |(x, y)| (x * 0.6, y * 0.6));
            }
        }
        Move::Skitter => {
            if hunting && steps < 4.0 {
                mv = w.flow(pf, me.x, me.y, true).unwrap_or_default();
            } else {
                mv = wander(w, b, i, dt, 1, 4, (0.4, 1.1)).unwrap_or_default();
            }
            // the nervous twitch
            let j = (w.time * 9.0 + i as f32 * 2.1).sin() * 0.35;
            mv = (mv.0 - mv.1 * j, mv.1 + mv.0 * j);
        }
    }

    if spec.speed > 0.0 {
        // swerve out of the way of incoming shells
        for s in &w.shells {
            let (rx, ry) = (me.x - s.x, me.y - s.y);
            let vv = s.vx * s.vx + s.vy * s.vy;
            let t = (rx * s.vx + ry * s.vy) / vv;
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            let (cx, cy) = (rx - s.vx * t, ry - s.vy * t);
            let miss = cx.hypot(cy);
            if miss > 1.0 {
                continue;
            }
            // decided once per shell and tank, so the swerve does not flicker
            let roll = (s.id.wrapping_mul(2_654_435_761) ^ (i as u32).wrapping_mul(40_503)) % 1000;
            if (roll as f32) >= spec.dodge * 1000.0 {
                continue;
            }
            let (ux, uy) = if miss > 1e-3 { (cx / miss, cy / miss) } else { (-s.vy / vv.sqrt(), s.vx / vv.sqrt()) };
            let k = (1.1 - miss) * 3.0;
            mv.0 += ux * k;
            mv.1 += uy * k;
        }
        // stay clear of mines (including your own once it is down)
        for m in &w.mines {
            let (rx, ry) = (me.x - m.x, me.y - m.y);
            let d = rx.hypot(ry).max(0.05);
            if d < 2.3 {
                let k = (2.3 - d) * 1.6;
                mv.0 += rx / d * k;
                mv.1 += ry / d * k;
            }
        }
        // and of each other
        for (j, o) in w.tanks.iter().enumerate().skip(w.players) {
            if j == i || !o.alive {
                continue;
            }
            let (rx, ry) = (me.x - o.x, me.y - o.y);
            let d = rx.hypot(ry).max(0.05);
            if d < TANK_R * 3.2 {
                mv.0 += rx / d * 0.6;
                mv.1 += ry / d * 0.6;
            }
        }
    }
    let len = mv.0.hypot(mv.1);
    if len > 1.0 {
        mv = (mv.0 / len, mv.1 / len);
    }

    // ---- mines
    let mut mine = false;
    if spec.max_mines > 0 && hunting && w.mines_of(i) < spec.max_mines && b.rng.f32() < spec.mines * dt {
        let near_mine = w.mines.iter().any(|m| (m.x - me.x).hypot(m.y - me.y) < 3.0);
        let near_friend = w.tanks.iter().enumerate().skip(w.players).any(|(j, o)| j != i && o.alive && (o.x - me.x).hypot(o.y - me.y) < 2.5);
        mine = !near_mine && !near_friend && dist < 9.0 && dist > 2.0;
    }

    Decision { mv, aim, fire, mine }
}

/// Head for a random nearby tile `min..=max` steps away; a new one when there,
/// or after `patience` seconds.
fn wander(w: &World, b: &mut Brain, i: usize, dt: f32, min: u16, max: u16, patience: (f32, f32)) -> Option<(f32, f32)> {
    let me = &w.tanks[i];
    let here = (me.x as i32, me.y as i32);
    b.goal_t -= dt;
    if b.goal.is_none() || b.goal == Some(here) || b.goal_t <= 0.0 || b.field.is_empty() {
        let from_me = w.field_from(here);
        let mut options = Vec::new();
        for (k, &v) in from_me.iter().enumerate() {
            if v >= min && v <= max && v != u16::MAX {
                options.push(((k % crate::arena::GW) as i32, (k / crate::arena::GW) as i32));
            }
        }
        if options.is_empty() {
            return None;
        }
        let g = options[(b.rng.next_u32() as usize) % options.len()];
        b.goal = Some(g);
        b.field = w.field_from(g);
        b.goal_t = b.rng.range(patience.0, patience.1);
    }
    w.flow(&b.field, me.x, me.y, false)
}

/// The angle nearest the turret's current one whose shell path, bounces
/// included, ends in the player.
fn bank_shot(w: &World, i: usize, current: f32) -> Option<f32> {
    let me = &w.tanks[i];
    let spec = me.spec();
    const N: usize = 240;
    let mut best: Option<(f32, f32)> = None;
    for k in 0..N {
        let a = k as f32 * TAU / N as f32 - PI;
        if w.trace(me.x, me.y, a, spec.bounces, i, 34.0).is_some_and(|j| w.is_player(j)) {
            let d = wrap(a - current).abs();
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((a, d));
            }
        }
    }
    best.map(|(a, _)| a)
}
