//! The battle simulation: tanks, shells that ricochet, mines, and the marks
//! they leave on the floor. One map unit is one tile; y grows downwards.

use crate::ai::{self, Brain, Spec};
use crate::arena::{Arena, Mission, Tile, GH, GW};
use serde::{Deserialize, Serialize};
use std::f32::consts::{PI, TAU};

pub const TANK_R: f32 = 0.4;
pub const SHELL_R: f32 = 0.09;
pub const MINE_FUSE: f32 = 10.0;
pub const MINE_RADIUS: f32 = 1.6;
/// Floor stain resolution, cells per tile.
pub const STAIN: usize = 8;
/// Roster kind of the second player's tank.
pub const P2_KIND: usize = 10;
/// Shells cannot hit the tank that fired them this soon.
const OWNER_GRACE: f32 = 0.22;
const FIELD_EVERY: f32 = 0.25;

pub type Rgb = (u8, u8, u8);

#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1)
    }
    pub fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
    pub fn f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f32()
    }
}

pub fn wrap(a: f32) -> f32 {
    (a + PI).rem_euclid(TAU) - PI
}

#[derive(Clone)]
pub struct Tank {
    /// Index into `ai::SPECS`: 0 is the player, 1-9 the enemy roster.
    pub kind: usize,
    pub x: f32,
    pub y: f32,
    pub heading: f32,
    pub turret: f32,
    pub alive: bool,
    /// Current speed as a fraction of top speed (for sound).
    pub speed: f32,
    pub vx: f32,
    pub vy: f32,
    pub odometer: f32,
    stamp: f32,
    pub cooldown: f32,
    pub muzzle: f32,
    /// Seconds a ghost stays visible after firing.
    pub reveal: f32,
    pub burst: u32,
    burst_t: f32,
    /// Index of this tank's spawn in the mission's enemy list.
    pub spawn: usize,
}

impl Tank {
    pub fn spec(&self) -> &'static Spec {
        &ai::SPECS[self.kind]
    }
}

#[derive(Clone, Copy)]
pub struct Shell {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub bounces: i32,
    pub owner: usize,
    pub rocket: bool,
    pub age: f32,
    pub alive: bool,
    puff: f32,
}

#[derive(Clone, Copy)]
pub struct Mine {
    pub x: f32,
    pub y: f32,
    pub owner: usize,
    pub fuse: f32,
    pub armed: bool,
    pub alive: bool,
    beep: f32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PKind {
    Smoke,
    Fire,
    Spark,
    Debris(Rgb),
}

#[derive(Clone, Copy)]
pub struct Particle {
    pub x: f32,
    pub y: f32,
    /// Height above the floor, in tiles.
    pub z: f32,
    pub vx: f32,
    pub vy: f32,
    pub vz: f32,
    pub life: f32,
    pub max: f32,
    pub size: f32,
    pub kind: PKind,
}

#[derive(Clone, Copy)]
pub struct Blast {
    pub x: f32,
    pub y: f32,
    pub age: f32,
    pub radius: f32,
}

/// Something that happened, for sounds and for the effects that go with it.
/// A guest machine receives these from the host and replays the effects.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Shot { rocket: bool, x: f32, y: f32, angle: f32 },
    Bounce { x: f32, y: f32 },
    Fizzle { x: f32, y: f32 },
    Cancel { x: f32, y: f32 },
    /// Out of shells; only the player who pressed fire hears the click.
    Empty { who: u8 },
    TankDown { kind: usize, x: f32, y: f32 },
    MineSet,
    MineBeep,
    MineBoom { x: f32, y: f32 },
    CrateBroken { tx: i32, ty: i32 },
}

/// What a guest needs each tick to draw the battle the host is running.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub time: f32,
    pub tanks: Vec<TankState>,
    pub shells: Vec<ShellState>,
    pub mines: Vec<MineState>,
    pub events: Vec<Event>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct TankState {
    pub kind: u8,
    pub alive: bool,
    pub x: f32,
    pub y: f32,
    pub heading: f32,
    pub turret: f32,
    pub speed: f32,
    pub odometer: f32,
    pub muzzle: f32,
    pub reveal: f32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct ShellState {
    pub id: u32,
    pub owner: u8,
    pub rocket: bool,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct MineState {
    pub owner: u8,
    pub x: f32,
    pub y: f32,
    pub fuse: f32,
}

#[derive(Clone, Copy, Default, Debug, Serialize, Deserialize)]
pub struct Controls {
    /// Desired direction of travel (x right, y down), length up to 1.
    pub mv: (f32, f32),
    /// Turret target: a map point (mouse) or a direction (stick, keys).
    pub aim_at: Option<(f32, f32)>,
    pub aim_dir: Option<f32>,
    /// Turret rotation for the fine-aim keys, -1..1.
    pub turret_turn: f32,
    pub fire: bool,
    pub mine: bool,
}

pub struct World {
    pub arena: Arena,
    /// The first `players` tanks are people, the rest the enemy.
    pub players: usize,
    pub tanks: Vec<Tank>,
    brains: Vec<Brain>,
    pub shells: Vec<Shell>,
    pub mines: Vec<Mine>,
    pub particles: Vec<Particle>,
    pub blasts: Vec<Blast>,
    /// Tread marks (fade) and scorch marks (stay), darkness 0-1 per stain cell.
    pub tracks: Vec<f32>,
    pub scorch: Vec<f32>,
    pub events: Vec<Event>,
    pub time: f32,
    pub rng: Rng,
    /// Steps to the nearest player from every tile (u16::MAX: unreachable).
    pub player_field: Vec<u16>,
    field_t: f32,
    fade_t: f32,
    next_id: u32,
    pub won_t: Option<f32>,
    pub lost_t: Option<f32>,
    /// Spawn indexes of enemies destroyed in this attempt.
    pub kills: Vec<usize>,
    pending_mines: Vec<usize>,
}

impl World {
    /// A battle on `mission` for `players` (1 or 2), without the enemies whose
    /// spawn index is in `gone`.
    pub fn new(mission: &Mission, gone: &[usize], seed: u64, players: usize) -> World {
        let mut tanks = vec![Tank {
            kind: 0,
            x: mission.player.0,
            y: mission.player.1,
            heading: 0.0,
            turret: 0.0,
            alive: true,
            speed: 0.0,
            vx: 0.0,
            vy: 0.0,
            odometer: 0.0,
            stamp: 0.0,
            cooldown: 0.0,
            muzzle: 0.0,
            reveal: 0.0,
            burst: 0,
            burst_t: 0.0,
            spawn: usize::MAX,
        }];
        if players > 1 {
            let (x, y) = second_spawn(mission);
            tanks.push(Tank { kind: P2_KIND, x, y, ..tanks[0].clone() });
        }
        let mut rng = Rng::new(seed);
        for (i, &(kind, x, y)) in mission.enemies.iter().enumerate() {
            if gone.contains(&i) {
                continue;
            }
            let face = (mission.player.1 - y).atan2(mission.player.0 - x);
            // face the player, snapped to the hull's four sides
            let heading = (face / (PI / 2.0)).round() * (PI / 2.0);
            tanks.push(Tank { kind, x, y, heading, turret: face, spawn: i, cooldown: rng.range(1.0, 2.0), ..tanks[0].clone() });
        }
        for t in &mut tanks[..players] {
            t.spawn = usize::MAX;
        }
        let brains = (0..tanks.len()).map(|i| Brain::new(seed.wrapping_add(i as u64 * 7919))).collect();
        let n = GW * STAIN * GH * STAIN;
        let mut w = World {
            arena: mission.arena.clone(),
            players: players.max(1),
            tanks,
            brains,
            shells: Vec::new(),
            mines: Vec::new(),
            particles: Vec::new(),
            blasts: Vec::new(),
            tracks: vec![0.0; n],
            scorch: vec![0.0; n],
            events: Vec::new(),
            time: 0.0,
            rng,
            player_field: Vec::new(),
            field_t: 0.0,
            fade_t: 0.0,
            next_id: 1,
            won_t: None,
            lost_t: None,
            kills: Vec::new(),
            pending_mines: Vec::new(),
        };
        w.player_field = w.player_field();
        w
    }

    pub fn is_player(&self, i: usize) -> bool {
        i < self.players
    }

    pub fn enemies(&self) -> &[Tank] {
        &self.tanks[self.players..]
    }

    pub fn enemies_left(&self) -> usize {
        self.enemies().iter().filter(|t| t.alive).count()
    }

    pub fn players_alive(&self) -> usize {
        self.tanks[..self.players].iter().filter(|t| t.alive).count()
    }

    fn player_field(&self) -> Vec<u16> {
        let goals: Vec<(i32, i32)> = self.tanks[..self.players].iter().filter(|t| t.alive).map(|t| (t.x as i32, t.y as i32)).collect();
        self.field_from_many(&goals)
    }

    pub fn shells_of(&self, owner: usize) -> usize {
        self.shells.iter().filter(|s| s.alive && s.owner == owner).count()
    }

    pub fn mines_of(&self, owner: usize) -> usize {
        self.mines.iter().filter(|m| m.alive && m.owner == owner).count()
    }

    /// Finished: Some(true) won, Some(false) lost, once the dust settles.
    pub fn outcome(&self) -> Option<bool> {
        if self.lost_t.is_some_and(|t| t > 2.4) {
            Some(false)
        } else if self.won_t.is_some_and(|t| t > 2.0) {
            Some(true)
        } else {
            None
        }
    }

    // ---------------------------------------------------------------- paths

    /// Breadth-first steps to `goal` from every tile, 8-way without cutting corners.
    pub fn field_from(&self, goal: (i32, i32)) -> Vec<u16> {
        self.field_from_many(&[goal])
    }

    pub fn field_from_many(&self, goals: &[(i32, i32)]) -> Vec<u16> {
        let mut f = vec![u16::MAX; GW * GH];
        let mut q = std::collections::VecDeque::new();
        for &goal in goals {
            if self.arena.walkable(goal.0, goal.1) {
                f[goal.1 as usize * GW + goal.0 as usize] = 0;
                q.push_back(goal);
            }
        }
        while let Some((x, y)) = q.pop_front() {
            let d = f[y as usize * GW + x as usize];
            for (dx, dy) in NEIGHBOURS {
                if !self.step_ok(x, y, dx, dy) {
                    continue;
                }
                let (nx, ny) = (x + dx, y + dy);
                let i = ny as usize * GW + nx as usize;
                if f[i] == u16::MAX {
                    f[i] = d + 1;
                    q.push_back((nx, ny));
                }
            }
        }
        f
    }

    fn step_ok(&self, x: i32, y: i32, dx: i32, dy: i32) -> bool {
        let a = &self.arena;
        a.walkable(x + dx, y + dy) && (dx == 0 || dy == 0 || (a.walkable(x + dx, y) && a.walkable(x, y + dy)))
    }

    /// Direction from (x, y) one step down (or up, `ascend`) the field.
    pub fn flow(&self, field: &[u16], x: f32, y: f32, ascend: bool) -> Option<(f32, f32)> {
        let (tx, ty) = (x.floor() as i32, y.floor() as i32);
        let here = *field.get(ty.max(0) as usize * GW + tx.max(0) as usize)?;
        if here == u16::MAX {
            return None;
        }
        let mut best = here;
        let mut pick = None;
        for (dx, dy) in NEIGHBOURS {
            if !self.step_ok(tx, ty, dx, dy) {
                continue;
            }
            let v = field[(ty + dy) as usize * GW + (tx + dx) as usize];
            if v == u16::MAX {
                continue;
            }
            if (ascend && v > best) || (!ascend && v < best) {
                best = v;
                pick = Some((tx + dx, ty + dy));
            }
        }
        let (gx, gy) = match pick {
            Some((nx, ny)) => (nx as f32 + 0.5, ny as f32 + 0.5),
            None => (tx as f32 + 0.5, ty as f32 + 0.5),
        };
        let (dx, dy) = (gx - x, gy - y);
        let d = dx.hypot(dy);
        if d < 0.12 {
            return None;
        }
        Some((dx / d, dy / d))
    }

    /// Follow a shell's path from (x, y) at `angle`, bouncing up to `bounces`
    /// times: the first tank it would hit, if any.
    pub fn trace(&self, x: f32, y: f32, angle: f32, bounces: i32, shooter: usize, max_len: f32) -> Option<usize> {
        let (mut x, mut y) = (x, y);
        let (mut vx, mut vy) = (angle.cos(), angle.sin());
        let step = 0.1;
        let mut left = bounces;
        let mut len = 0.0;
        let reach = (TANK_R + SHELL_R + 0.02).powi(2);
        while len < max_len {
            let mut hit = false;
            if self.arena.at(x + vx * step, y).blocks_shells() {
                vx = -vx;
                hit = true;
            } else {
                x += vx * step;
            }
            if self.arena.at(x, y + vy * step).blocks_shells() {
                vy = -vy;
                hit = true;
            } else {
                y += vy * step;
            }
            if hit {
                left -= 1;
                if left < 0 {
                    return None;
                }
            }
            len += step;
            for (j, t) in self.tanks.iter().enumerate() {
                if !t.alive || (j == shooter && len < 0.8) {
                    continue;
                }
                if (t.x - x).powi(2) + (t.y - y).powi(2) < reach {
                    return Some(j);
                }
            }
        }
        None
    }

    /// Where a straight line from (x, y) at `angle` first meets a wall.
    pub fn ray_to_wall(&self, x: f32, y: f32, angle: f32, max_len: f32) -> (f32, f32) {
        let (dx, dy) = (angle.cos() * 0.05, angle.sin() * 0.05);
        let (mut x, mut y) = (x, y);
        let mut len = 0.0;
        while len < max_len && !self.arena.at(x + dx, y + dy).blocks_shells() {
            x += dx;
            y += dy;
            len += 0.05;
        }
        (x, y)
    }

    // ----------------------------------------------------------------- step

    /// One tick; `controls[i]` drives player `i`.
    pub fn step(&mut self, dt: f32, controls: &[Controls]) {
        self.time += dt;
        if let Some(t) = &mut self.won_t {
            *t += dt;
        }
        if let Some(t) = &mut self.lost_t {
            *t += dt;
        }
        // the players
        for i in 0..self.players {
            let c = controls.get(i).copied().unwrap_or_default();
            if !self.tanks[i].alive {
                continue;
            }
            self.drive(i, c.mv, dt);
            let p = &self.tanks[i];
            let want = match (c.aim_at, c.aim_dir) {
                (_, Some(a)) => Some(a),
                (Some((ax, ay)), None) => Some((ay - p.y).atan2(ax - p.x)),
                _ => None,
            };
            let p = &mut self.tanks[i];
            if let Some(a) = want {
                let d = wrap(a - p.turret);
                let step = 14.0 * dt;
                p.turret = wrap(p.turret + d.clamp(-step, step));
            }
            p.turret = wrap(p.turret + c.turret_turn * 2.2 * dt);
            if c.fire {
                self.try_fire(i);
            }
            if c.mine {
                self.try_mine(i);
            }
        }
        // the enemies
        self.field_t -= dt;
        if self.field_t <= 0.0 {
            self.field_t = FIELD_EVERY;
            self.player_field = self.player_field();
        }
        let mut brains = std::mem::take(&mut self.brains);
        for i in self.players..self.tanks.len() {
            if !self.tanks[i].alive {
                continue;
            }
            let d = ai::think(self, &mut brains[i], i, dt);
            self.drive(i, d.mv, dt);
            let t = &mut self.tanks[i];
            let step = t.spec().turret_rate * dt;
            t.turret = wrap(t.turret + wrap(d.aim - t.turret).clamp(-step, step));
            if d.fire {
                self.try_fire(i);
            }
            if d.mine {
                self.try_mine(i);
            }
        }
        self.brains = brains;
        // bursts, timers
        for i in 0..self.tanks.len() {
            let t = &mut self.tanks[i];
            t.cooldown = (t.cooldown - dt).max(0.0);
            t.muzzle = (t.muzzle - dt).max(0.0);
            t.reveal = (t.reveal - dt).max(0.0);
            if t.alive && t.burst > 0 {
                t.burst_t -= dt;
                if t.burst_t <= 0.0 {
                    t.burst -= 1;
                    t.burst_t = 0.13;
                    self.fire(i);
                }
            }
        }
        self.separate_tanks();
        self.update_shells(dt);
        self.trails(dt);
        self.update_mines(dt);
        self.update_effects(dt);
        // outcome
        if self.players_alive() == 0 && self.lost_t.is_none() {
            self.lost_t = Some(0.0);
            log::info!("all players destroyed at {:.1}s", self.time);
        }
        if self.players_alive() > 0 && self.won_t.is_none() && self.enemies_left() == 0 {
            self.won_t = Some(0.0);
            log::info!("mission cleared at {:.1}s", self.time);
        }
    }

    /// Steer tank `i` toward direction `mv`. Tanks turn in place before they
    /// roll, and either end of the hull will do, as with real treads.
    fn drive(&mut self, i: usize, mv: (f32, f32), dt: f32) {
        let spec = self.tanks[i].spec();
        let mag = mv.0.hypot(mv.1).min(1.0);
        let t = &mut self.tanks[i];
        if mag < 0.1 || spec.speed <= 0.0 {
            t.speed = 0.0;
            t.vx = 0.0;
            t.vy = 0.0;
            return;
        }
        let want = mv.1.atan2(mv.0);
        let fwd = wrap(want - t.heading);
        let back = wrap(want + PI - t.heading);
        let (diff, dir) = if fwd.abs() <= back.abs() + 0.05 { (fwd, 1.0) } else { (back, -1.0) };
        let step = spec.turn * dt;
        t.heading = wrap(t.heading + diff.clamp(-step, step));
        let rest = (diff - diff.clamp(-step, step)).abs();
        let f = if rest < 0.45 { 1.0 } else { (1.0 - (rest - 0.45) * 1.4).max(0.0) };
        let v = spec.speed * mag * f * dir;
        let (ox, oy) = (t.x, t.y);
        t.x += t.heading.cos() * v * dt;
        t.y += t.heading.sin() * v * dt;
        self.resolve_walls(i);
        let t = &mut self.tanks[i];
        let moved = (t.x - ox).hypot(t.y - oy);
        t.vx = (t.x - ox) / dt;
        t.vy = (t.y - oy) / dt;
        t.speed = (moved / dt / spec.speed).min(1.0);
        t.odometer += moved * dir;
        self.stamp_tracks(i);
    }

    /// Tread marks every short stretch of driving.
    fn stamp_tracks(&mut self, i: usize) {
        let t = &mut self.tanks[i];
        if (t.odometer - t.stamp).abs() > 0.09 {
            t.stamp = t.odometer;
            let (x, y, h) = (t.x, t.y, t.heading);
            let (nx, ny) = (-h.sin(), h.cos());
            for side in [-0.25f32, 0.25] {
                self.stain_track(x + nx * side, y + ny * side);
            }
        }
    }

    fn resolve_walls(&mut self, i: usize) {
        for _ in 0..2 {
            let t = &self.tanks[i];
            let (mut x, mut y) = (t.x, t.y);
            for ty in (y - TANK_R).floor() as i32..=(y + TANK_R).floor() as i32 {
                for tx in (x - TANK_R).floor() as i32..=(x + TANK_R).floor() as i32 {
                    if !self.arena.tile(tx, ty).blocks_tanks() {
                        continue;
                    }
                    let cx = x.clamp(tx as f32, tx as f32 + 1.0);
                    let cy = y.clamp(ty as f32, ty as f32 + 1.0);
                    let (dx, dy) = (x - cx, y - cy);
                    let d = dx.hypot(dy);
                    if d >= TANK_R {
                        continue;
                    }
                    if d > 1e-4 {
                        x += dx / d * (TANK_R - d);
                        y += dy / d * (TANK_R - d);
                    } else {
                        // centre inside the tile: leave by the nearest side
                        let (l, r) = (x - tx as f32, tx as f32 + 1.0 - x);
                        let (u, b) = (y - ty as f32, ty as f32 + 1.0 - y);
                        let m = l.min(r).min(u).min(b);
                        if m == l {
                            x = tx as f32 - TANK_R;
                        } else if m == r {
                            x = tx as f32 + 1.0 + TANK_R;
                        } else if m == u {
                            y = ty as f32 - TANK_R;
                        } else {
                            y = ty as f32 + 1.0 + TANK_R;
                        }
                    }
                }
            }
            let t = &mut self.tanks[i];
            t.x = x;
            t.y = y;
        }
    }

    fn separate_tanks(&mut self) {
        let n = self.tanks.len();
        for i in 0..n {
            for j in i + 1..n {
                let (a, b) = (&self.tanks[i], &self.tanks[j]);
                if !a.alive || !b.alive {
                    continue;
                }
                let (dx, dy) = (b.x - a.x, b.y - a.y);
                let d = dx.hypot(dy).max(1e-3);
                let overlap = 2.0 * TANK_R - d;
                if overlap <= 0.0 {
                    continue;
                }
                let (ai, bj) = (a.spec().speed <= 0.0, b.spec().speed <= 0.0);
                let (ka, kb) = match (ai, bj) {
                    (true, true) => (0.0, 0.0),
                    (true, false) => (0.0, 1.0),
                    (false, true) => (1.0, 0.0),
                    _ => (0.5, 0.5),
                };
                let (ux, uy) = (dx / d, dy / d);
                self.tanks[i].x -= ux * overlap * ka;
                self.tanks[i].y -= uy * overlap * ka;
                self.tanks[j].x += ux * overlap * kb;
                self.tanks[j].y += uy * overlap * kb;
                self.resolve_walls(i);
                self.resolve_walls(j);
            }
        }
    }

    // --------------------------------------------------------------- firing

    fn try_fire(&mut self, i: usize) {
        let t = &self.tanks[i];
        if !t.alive || t.cooldown > 0.0 || t.burst > 0 {
            return;
        }
        let spec = t.spec();
        if self.shells_of(i) >= spec.max_shells {
            if self.is_player(i) {
                self.events.push(Event::Empty { who: i as u8 });
            }
            return;
        }
        self.tanks[i].cooldown = self.rng.range(spec.reload.0, spec.reload.1);
        if spec.burst > 1 {
            self.tanks[i].burst = spec.burst - 1;
            self.tanks[i].burst_t = 0.13;
        }
        self.fire(i);
    }

    fn fire(&mut self, i: usize) {
        let spec = self.tanks[i].spec();
        if self.shells_of(i) >= spec.max_shells + spec.burst as usize - 1 {
            return;
        }
        let t = &mut self.tanks[i];
        t.muzzle = 0.12;
        t.reveal = 0.6;
        let (ca, sa) = (t.turret.cos(), t.turret.sin());
        let id = self.next_id;
        self.next_id += 1;
        // start inside the tank so a muzzle against a wall still bounces off it
        self.shells.push(Shell {
            id,
            x: t.x + ca * 0.2,
            y: t.y + sa * 0.2,
            vx: ca * spec.shell_speed,
            vy: sa * spec.shell_speed,
            bounces: spec.bounces,
            owner: i,
            rocket: spec.rocket,
            age: 0.0,
            alive: true,
            puff: 0.0,
        });
        let (x, y, angle) = (t.x, t.y, t.turret);
        self.emit(Event::Shot { rocket: spec.rocket, x, y, angle });
    }

    fn try_mine(&mut self, i: usize) {
        let t = &self.tanks[i];
        let spec = t.spec();
        if !t.alive || self.mines_of(i) >= spec.max_mines {
            return;
        }
        let (x, y) = (t.x, t.y);
        if self.mines.iter().any(|m| m.alive && (m.x - x).hypot(m.y - y) < 0.8) {
            return;
        }
        self.mines.push(Mine { x, y, owner: i, fuse: MINE_FUSE, armed: false, alive: true, beep: 0.0 });
        self.events.push(Event::MineSet);
    }

    // --------------------------------------------------------------- shells

    fn update_shells(&mut self, dt: f32) {
        for k in 0..self.shells.len() {
            let mut s = self.shells[k];
            if !s.alive {
                continue;
            }
            let speed = s.vx.hypot(s.vy);
            let subs = ((speed * dt) / 0.08).ceil().max(1.0) as usize;
            let h = dt / subs as f32;
            for _ in 0..subs {
                let mut bounced = false;
                if self.arena.at(s.x + s.vx * h, s.y).blocks_shells() {
                    s.vx = -s.vx;
                    bounced = true;
                } else {
                    s.x += s.vx * h;
                }
                if self.arena.at(s.x, s.y + s.vy * h).blocks_shells() {
                    s.vy = -s.vy;
                    bounced = true;
                } else {
                    s.y += s.vy * h;
                }
                if bounced {
                    s.bounces -= 1;
                    if s.bounces < 0 {
                        s.alive = false;
                        self.emit(Event::Fizzle { x: s.x, y: s.y });
                        break;
                    }
                    self.emit(Event::Bounce { x: s.x, y: s.y });
                }
                s.age += h;
                // tanks
                let reach = (TANK_R + SHELL_R).powi(2);
                let mut hit = None;
                for (j, t) in self.tanks.iter().enumerate() {
                    if !t.alive || (j == s.owner && s.age < OWNER_GRACE) {
                        continue;
                    }
                    if (t.x - s.x).powi(2) + (t.y - s.y).powi(2) < reach {
                        hit = Some(j);
                        break;
                    }
                }
                if let Some(j) = hit {
                    s.alive = false;
                    self.destroy_tank(j);
                    break;
                }
                // mines
                if let Some(m) = self.mines.iter().position(|m| m.alive && (m.x - s.x).hypot(m.y - s.y) < 0.3) {
                    s.alive = false;
                    self.pending_mines.push(m);
                    break;
                }
            }
            self.shells[k] = s;
        }
        // shells meeting in the air cancel each other
        let n = self.shells.len();
        for a in 0..n {
            for b in a + 1..n {
                let (p, q) = (self.shells[a], self.shells[b]);
                if p.alive && q.alive && (p.x - q.x).hypot(p.y - q.y) < SHELL_R * 2.5 {
                    self.shells[a].alive = false;
                    self.shells[b].alive = false;
                    self.emit(Event::Cancel { x: (p.x + q.x) / 2.0, y: (p.y + q.y) / 2.0 });
                }
            }
        }
        self.shells.retain(|s| s.alive);
    }

    /// Smoke (or rocket fire) puffed out behind every shell.
    fn trails(&mut self, dt: f32) {
        for k in 0..self.shells.len() {
            let s = &mut self.shells[k];
            s.puff += s.vx.hypot(s.vy) * dt;
            if s.puff <= if s.rocket { 0.12 } else { 0.22 } {
                continue;
            }
            s.puff = 0.0;
            let life = if s.rocket { 0.45 } else { 0.6 };
            let p = Particle {
                x: s.x,
                y: s.y,
                z: 0.25,
                vx: 0.0,
                vy: 0.0,
                vz: 0.15,
                life,
                max: life,
                size: if s.rocket { 0.11 } else { 0.08 },
                kind: if s.rocket { PKind::Fire } else { PKind::Smoke },
            };
            self.particles.push(p);
        }
    }

    fn destroy_tank(&mut self, j: usize) {
        // once the last enemy falls the player cannot lose any more
        if self.is_player(j) && self.won_t.is_some() {
            return;
        }
        let t = &mut self.tanks[j];
        if !t.alive {
            return;
        }
        t.alive = false;
        let (x, y, kind, spawn) = (t.x, t.y, t.kind, t.spawn);
        if !self.is_player(j) {
            self.kills.push(spawn);
        }
        self.emit(Event::TankDown { kind, x, y });
        log::info!("tank {j} ({}) destroyed", ai::SPECS[kind].name);
    }

    // ---------------------------------------------------------------- mines

    fn update_mines(&mut self, dt: f32) {
        for k in 0..self.mines.len() {
            let mut m = self.mines[k];
            if !m.alive {
                continue;
            }
            m.fuse -= dt;
            let owner = &self.tanks[m.owner];
            if !m.armed && (!owner.alive || (owner.x - m.x).hypot(owner.y - m.y) > TANK_R + 0.5 || m.fuse < MINE_FUSE - 4.0) {
                m.armed = true;
            }
            if m.armed && self.tanks.iter().any(|t| t.alive && (t.x - m.x).hypot(t.y - m.y) < 0.7) {
                self.pending_mines.push(k);
            }
            if m.fuse < 2.5 {
                m.beep -= dt;
                if m.beep <= 0.0 {
                    m.beep = 0.12 + 0.3 * (m.fuse / 2.5);
                    self.events.push(Event::MineBeep);
                }
            }
            if m.fuse <= 0.0 {
                self.pending_mines.push(k);
            }
            self.mines[k] = m;
        }
        while let Some(k) = self.pending_mines.pop() {
            self.explode_mine(k);
        }
        self.mines.retain(|m| m.alive);
    }

    fn explode_mine(&mut self, k: usize) {
        if !self.mines[k].alive {
            return;
        }
        self.mines[k].alive = false;
        let (x, y) = (self.mines[k].x, self.mines[k].y);
        self.emit(Event::MineBoom { x, y });
        for j in 0..self.tanks.len() {
            let t = &self.tanks[j];
            if t.alive && (t.x - x).hypot(t.y - y) < MINE_RADIUS + TANK_R * 0.4 {
                self.destroy_tank(j);
            }
        }
        for s in &mut self.shells {
            if (s.x - x).hypot(s.y - y) < MINE_RADIUS {
                s.alive = false;
            }
        }
        for ty in (y - MINE_RADIUS).floor() as i32..=(y + MINE_RADIUS).floor() as i32 {
            for tx in (x - MINE_RADIUS).floor() as i32..=(x + MINE_RADIUS).floor() as i32 {
                let (cx, cy) = (x.clamp(tx as f32, tx as f32 + 1.0), y.clamp(ty as f32, ty as f32 + 1.0));
                if self.arena.tile(tx, ty) == Tile::Crate && (cx - x).hypot(cy - y) < MINE_RADIUS * 0.8 {
                    self.emit(Event::CrateBroken { tx, ty });
                    // the paths change
                    self.field_t = 0.0;
                }
            }
        }
        for (m, other) in self.mines.iter().enumerate() {
            if other.alive && (other.x - x).hypot(other.y - y) < MINE_RADIUS {
                self.pending_mines.push(m);
            }
        }
    }

    // -------------------------------------------------------------- effects

    fn emit(&mut self, e: Event) {
        self.fx(e);
        self.events.push(e);
    }

    /// The looks of an event: smoke, sparks, blasts, scorch, debris, and the
    /// broken crate. Run by whoever draws the battle, host or guest.
    pub fn fx(&mut self, e: Event) {
        match e {
            Event::Shot { x, y, angle, .. } => {
                let (mx, my) = (x + angle.cos() * 0.55, y + angle.sin() * 0.55);
                for _ in 0..4 {
                    let a = angle + self.rng.range(-0.6, 0.6);
                    let v = self.rng.range(0.3, 1.2);
                    let life = self.rng.range(0.3, 0.6);
                    self.particles.push(Particle { x: mx, y: my, z: 0.3, vx: a.cos() * v, vy: a.sin() * v, vz: 0.2, life, max: life, size: 0.12, kind: PKind::Smoke });
                }
            }
            Event::Bounce { x, y } => self.puff(x, y, 3, PKind::Spark),
            Event::Fizzle { x, y } => self.puff(x, y, 5, PKind::Smoke),
            Event::Cancel { x, y } => self.puff(x, y, 6, PKind::Spark),
            Event::TankDown { kind, x, y } => {
                let body = ai::SPECS[kind.min(ai::SPECS.len() - 1)].body;
                self.blasts.push(Blast { x, y, age: 0.0, radius: 0.75 });
                self.scorch_cross(x, y);
                for _ in 0..10 {
                    let a = self.rng.range(0.0, TAU);
                    let v = self.rng.range(1.0, 3.5);
                    let life = self.rng.range(0.5, 1.0);
                    self.particles.push(Particle { x, y, z: 0.3, vx: a.cos() * v, vy: a.sin() * v, vz: self.rng.range(1.0, 3.0), life, max: life, size: 0.1, kind: PKind::Debris(body) });
                }
                self.puff(x, y, 10, PKind::Smoke);
                self.puff(x, y, 6, PKind::Fire);
            }
            Event::MineBoom { x, y } => {
                self.blasts.push(Blast { x, y, age: 0.0, radius: MINE_RADIUS });
                self.stain_disc(x, y, MINE_RADIUS * 0.7);
                self.puff(x, y, 18, PKind::Smoke);
                self.puff(x, y, 10, PKind::Fire);
            }
            Event::CrateBroken { tx, ty } => {
                self.arena.set(tx, ty, Tile::Floor);
                for _ in 0..8 {
                    let a = self.rng.range(0.0, TAU);
                    let v = self.rng.range(0.5, 2.5);
                    let life = self.rng.range(0.5, 1.1);
                    let (px, py) = (tx as f32 + self.rng.f32(), ty as f32 + self.rng.f32());
                    self.particles.push(Particle { x: px, y: py, z: 0.5, vx: a.cos() * v, vy: a.sin() * v, vz: self.rng.range(1.0, 3.0), life, max: life, size: 0.1, kind: PKind::Debris((214, 176, 120)) });
                }
            }
            Event::Empty { .. } | Event::MineSet | Event::MineBeep => {}
        }
    }

    /// The battle as a guest needs it, with the events since the last one.
    pub fn snapshot(&self, events: Vec<Event>) -> Snapshot {
        Snapshot {
            time: self.time,
            tanks: self
                .tanks
                .iter()
                .map(|t| TankState {
                    kind: t.kind as u8,
                    alive: t.alive,
                    x: t.x,
                    y: t.y,
                    heading: t.heading,
                    turret: t.turret,
                    speed: t.speed,
                    odometer: t.odometer,
                    muzzle: t.muzzle,
                    reveal: t.reveal,
                })
                .collect(),
            shells: self.shells.iter().map(|s| ShellState { id: s.id, owner: s.owner as u8, rocket: s.rocket, x: s.x, y: s.y, vx: s.vx, vy: s.vy }).collect(),
            mines: self.mines.iter().map(|m| MineState { owner: m.owner as u8, x: m.x, y: m.y, fuse: m.fuse }).collect(),
            events,
        }
    }

    /// Guest side: take the host's battle as it is now, and replay the looks
    /// of what happened since the last snapshot.
    pub fn apply(&mut self, s: &Snapshot) {
        let dt = (s.time - self.time).clamp(0.0, 0.1);
        self.time = s.time;
        for (k, st) in s.tanks.iter().enumerate() {
            if k >= self.tanks.len() {
                let mut t = self.tanks[0].clone();
                t.stamp = st.odometer;
                self.tanks.push(t);
            }
            let t = &mut self.tanks[k];
            t.kind = st.kind as usize;
            t.alive = st.alive;
            t.x = st.x;
            t.y = st.y;
            t.heading = st.heading;
            t.turret = st.turret;
            t.speed = st.speed;
            t.odometer = st.odometer;
            t.muzzle = st.muzzle;
            t.reveal = st.reveal;
            self.stamp_tracks(k);
        }
        self.tanks.truncate(s.tanks.len());
        let old = std::mem::take(&mut self.shells);
        self.shells = s
            .shells
            .iter()
            .map(|n| Shell {
                id: n.id,
                x: n.x,
                y: n.y,
                vx: n.vx,
                vy: n.vy,
                bounces: 0,
                owner: n.owner as usize,
                rocket: n.rocket,
                age: 1.0,
                alive: true,
                // keep each shell's trail spacing across snapshots
                puff: old.iter().find(|o| o.id == n.id).map_or(0.0, |o| o.puff),
            })
            .collect();
        self.mines = s.mines.iter().map(|m| Mine { x: m.x, y: m.y, owner: m.owner as usize, fuse: m.fuse, armed: true, alive: true, beep: 0.0 }).collect();
        for &e in &s.events {
            self.emit(e);
        }
        self.trails(dt);
        self.update_effects(dt);
    }

    fn puff(&mut self, x: f32, y: f32, n: usize, kind: PKind) {
        for _ in 0..n {
            let a = self.rng.range(0.0, TAU);
            let v = match kind {
                PKind::Spark => self.rng.range(1.5, 4.0),
                PKind::Fire => self.rng.range(0.5, 2.0),
                _ => self.rng.range(0.2, 1.4),
            };
            let life = match kind {
                PKind::Spark => self.rng.range(0.1, 0.25),
                _ => self.rng.range(0.4, 0.9),
            };
            let size = match kind {
                PKind::Spark => 0.05,
                PKind::Fire => self.rng.range(0.12, 0.22),
                _ => self.rng.range(0.1, 0.22),
            };
            self.particles.push(Particle { x, y, z: 0.3, vx: a.cos() * v, vy: a.sin() * v, vz: 0.4, life, max: life, size, kind });
        }
    }

    fn update_effects(&mut self, dt: f32) {
        for p in &mut self.particles {
            p.life -= dt;
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            p.z += p.vz * dt;
            let drag = if matches!(p.kind, PKind::Debris(_)) { 0.6 } else { 2.5 };
            p.vx *= 1.0 - (drag * dt).min(1.0);
            p.vy *= 1.0 - (drag * dt).min(1.0);
            if let PKind::Debris(_) = p.kind {
                p.vz -= 9.0 * dt;
                if p.z < 0.0 {
                    p.z = 0.0;
                    p.vz = -p.vz * 0.35;
                    p.vx *= 0.6;
                    p.vy *= 0.6;
                }
            }
        }
        self.particles.retain(|p| p.life > 0.0);
        for b in &mut self.blasts {
            b.age += dt;
        }
        self.blasts.retain(|b| b.age < 0.7);
        self.fade_t += dt;
        if self.fade_t > 0.5 {
            // tread marks fade over about half a minute
            let k = 1.0 - self.fade_t / 30.0;
            self.fade_t = 0.0;
            for v in &mut self.tracks {
                *v *= k;
            }
        }
    }

    fn stain_index(x: f32, y: f32) -> Option<usize> {
        let (sx, sy) = ((x * STAIN as f32).floor(), (y * STAIN as f32).floor());
        if sx < 0.0 || sy < 0.0 || sx >= (GW * STAIN) as f32 || sy >= (GH * STAIN) as f32 {
            return None;
        }
        Some(sy as usize * GW * STAIN + sx as usize)
    }

    fn stain_track(&mut self, x: f32, y: f32) {
        if let Some(i) = Self::stain_index(x, y) {
            self.tracks[i] = self.tracks[i].max(0.3);
        }
    }

    fn scorch_cross(&mut self, x: f32, y: f32) {
        let n = 16;
        for k in 0..=n {
            let f = k as f32 / n as f32 - 0.5;
            for (dx, dy) in [(f, f), (f, -f)] {
                for (ox, oy) in [(0.0, 0.0), (0.06, 0.0)] {
                    if let Some(i) = Self::stain_index(x + dx * 0.9 + ox, y + dy * 0.9 + oy) {
                        self.scorch[i] = self.scorch[i].max(0.75);
                    }
                }
            }
        }
    }

    fn stain_disc(&mut self, x: f32, y: f32, r: f32) {
        let st = 1.0 / STAIN as f32;
        let mut sy = y - r;
        while sy <= y + r {
            let mut sx = x - r;
            while sx <= x + r {
                let d = (sx - x).hypot(sy - y) / r;
                if d < 1.0 {
                    if let Some(i) = Self::stain_index(sx, sy) {
                        let v = 0.6 * (1.0 - d * d);
                        self.scorch[i] = self.scorch[i].max(v);
                    }
                }
                sx += st;
            }
            sy += st;
        }
    }
}

/// Where the second player starts: a free floor tile near the first
/// player's spawn, two steps above or below when there is room.
fn second_spawn(m: &Mission) -> (f32, f32) {
    let (px, py) = (m.player.0 as i32, m.player.1 as i32);
    let free = |x: i32, y: i32| m.arena.walkable(x, y) && !m.enemies.iter().any(|e| e.1 as i32 == x && e.2 as i32 == y) && (x, y) != (px, py);
    let preferred = [(0, 2), (0, -2), (1, 2), (1, -2), (2, 0), (0, 3), (0, -3), (2, 2), (2, -2), (0, 1), (0, -1), (1, 0)];
    let ring = (1..8).flat_map(|r| (-r..=r).flat_map(move |dy| (-r..=r).map(move |dx| (dx, dy))));
    for (dx, dy) in preferred.into_iter().chain(ring) {
        if free(px + dx, py + dy) {
            return ((px + dx) as f32 + 0.5, (py + dy) as f32 + 0.5);
        }
    }
    (m.player.0, m.player.1)
}

const NEIGHBOURS: [(i32, i32); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{parse_missions, MISSIONS_TXT};

    #[test]
    fn shell_bounces_then_fizzles() {
        let missions = parse_missions(MISSIONS_TXT).unwrap();
        let mut w = World::new(&missions[0], &[], 1, 1);
        // remove the enemies so nothing interferes
        for t in &mut w.tanks[1..] {
            t.alive = false;
        }
        w.won_t = Some(0.0);
        w.tanks[0].turret = PI / 2.0;
        w.tanks[0].x = 3.5;
        w.tanks[0].y = 4.5;
        w.step(1.0 / 60.0, &[Controls { fire: true, ..Default::default() }]);
        assert_eq!(w.shells.len(), 1);
        let mut bounced = 0;
        for _ in 0..600 {
            w.step(1.0 / 60.0, &[]);
            bounced += w.events.drain(..).filter(|e| matches!(e, Event::Bounce { .. })).count();
        }
        assert_eq!(bounced, 1, "a standard shell bounces once");
        assert!(w.shells.is_empty(), "and then it is gone");
    }

    #[test]
    fn second_player_spawns_on_free_floor_everywhere() {
        for m in parse_missions(MISSIONS_TXT).unwrap() {
            let w = World::new(&m, &[], 1, 2);
            let (a, b) = (&w.tanks[0], &w.tanks[1]);
            assert_eq!(b.kind, P2_KIND);
            assert!(w.arena.walkable(b.x as i32, b.y as i32), "{}", m.name);
            assert!((a.x - b.x).hypot(a.y - b.y) >= 1.0, "{}: players overlap", m.name);
            for e in w.enemies() {
                assert!((e.x - b.x).hypot(e.y - b.y) >= 1.0, "{}: P2 on an enemy", m.name);
            }
        }
    }

    #[test]
    fn guest_mirrors_the_host() {
        let missions = parse_missions(MISSIONS_TXT).unwrap();
        let m = &missions[13];
        let mut host = World::new(m, &[], 5, 2);
        let mut guest = World::new(m, &[], 99, 2);
        let fire = Controls { fire: true, mine: true, aim_dir: Some(0.3), mv: (1.0, 0.0), ..Default::default() };
        for k in 0..900 {
            let c = if k % 30 == 0 { fire } else { Controls { mv: (0.0, 1.0), ..Default::default() } };
            host.step(1.0 / 60.0, &[c, c]);
            let events = std::mem::take(&mut host.events);
            guest.apply(&host.snapshot(events));
            guest.events.clear();
            assert_eq!(guest.tanks.len(), host.tanks.len());
            for (g, h) in guest.tanks.iter().zip(&host.tanks) {
                assert_eq!((g.alive, g.x, g.y, g.turret), (h.alive, h.x, h.y, h.turret));
            }
            assert_eq!(guest.shells.len(), host.shells.len());
            assert_eq!(guest.mines.len(), host.mines.len());
        }
        // broken crates and stains follow too
        for ty in 0..GH as i32 {
            for tx in 0..GW as i32 {
                assert_eq!(guest.arena.tile(tx, ty), host.arena.tile(tx, ty));
            }
        }
    }

    #[test]
    fn every_enemy_can_reach_or_is_stationary() {
        for (n, m) in parse_missions(MISSIONS_TXT).unwrap().iter().enumerate() {
            let w = World::new(m, &[], 1, 2);
            for t in w.enemies() {
                if t.spec().speed > 0.0 {
                    let f = &w.player_field;
                    let v = f[t.y as usize * GW + t.x as usize];
                    assert!(v != u16::MAX, "mission {} ({}): a {} cannot reach the player", n + 1, m.name, t.spec().name);
                }
            }
        }
    }
}

/// Headless balance check: a simple bot plays every mission a few times.
/// `cargo test --release balance -- --ignored --nocapture`
#[cfg(test)]
mod balance {
    use super::*;
    use crate::arena::{parse_missions, MISSIONS_TXT};

    fn bot(w: &World) -> Controls {
        let p = &w.tanks[0];
        let mut c = Controls::default();
        // aim at whatever the shell would hit, else the nearest enemy
        let target = w.tanks.iter().enumerate().skip(w.players).filter(|(_, t)| t.alive && !(t.spec().invisible && t.reveal <= 0.0)).min_by(|a, b| {
            let da = (a.1.x - p.x).hypot(a.1.y - p.y);
            let db = (b.1.x - p.x).hypot(b.1.y - p.y);
            da.total_cmp(&db)
        });
        if let Some((_, t)) = target {
            c.aim_dir = Some((t.y - p.y).atan2(t.x - p.x));
        }
        if let Some(j) = w.trace(p.x, p.y, p.turret, 1, 0, 30.0) {
            c.fire = j != 0;
        }
        // no clear line: go and find one
        if let Some((_, t)) = target {
            if w.trace(p.x, p.y, (t.y - p.y).atan2(t.x - p.x), 0, 0, 30.0).is_none_or(|j| j == 0) {
                let f = w.field_from((t.x as i32, t.y as i32));
                c.mv = w.flow(&f, p.x, p.y, false).unwrap_or_default();
            }
        }
        // dodge
        for s in &w.shells {
            let (rx, ry) = (p.x - s.x, p.y - s.y);
            let vv = s.vx * s.vx + s.vy * s.vy;
            let t = (rx * s.vx + ry * s.vy) / vv;
            if (0.0..1.0).contains(&t) {
                let (cx, cy) = (rx - s.vx * t, ry - s.vy * t);
                let m = cx.hypot(cy).max(0.01);
                if m < 0.9 {
                    c.mv = (cx / m, cy / m);
                }
            }
        }
        c
    }

    #[test]
    #[ignore]
    fn balance() {
        let missions = parse_missions(MISSIONS_TXT).unwrap();
        for (n, m) in missions.iter().enumerate() {
            let (mut wins, mut deaths, mut time, mut shots) = (0, 0, 0.0, 0);
            let mut causes = std::collections::BTreeMap::new();
            for seed in 0..6 {
                let mut gone: Vec<usize> = Vec::new();
                let mut lives = 3;
                loop {
                    let mut w = World::new(m, &gone, seed * 31 + 7, 1);
                    let mut t = 0.0;
                    while w.outcome().is_none() && t < 120.0 {
                        let c = bot(&w);
                        let before: Vec<(usize, usize)> = w.shells.iter().map(|s| (s.id as usize, s.owner)).collect();
                        let alive = w.tanks[0].alive;
                        w.step(1.0 / 60.0, &[c]);
                        if alive && !w.tanks[0].alive {
                            // who did it: the shell that vanished, or a mine
                            let gone_now: Vec<usize> = before.iter().filter(|(id, _)| !w.shells.iter().any(|s| s.id as usize == *id)).map(|(_, o)| w.tanks[*o].spec().name.len()).collect();
                            let cause = before.iter().find(|(id, _)| !w.shells.iter().any(|s| s.id as usize == *id)).map_or("mine", |(_, o)| w.tanks[*o].spec().name);
                            let _ = gone_now;
                            *causes.entry(cause).or_insert(0) += 1;
                        }
                        shots += w.events.iter().filter(|e| matches!(e, Event::Shot { .. })).count();
                        w.events.clear();
                        t += 1.0 / 60.0;
                    }
                    time += t;
                    match w.outcome() {
                        Some(true) => {
                            wins += 1;
                            break;
                        }
                        Some(false) => {
                            deaths += 1;
                            gone.extend(w.kills.iter().copied());
                            lives -= 1;
                            if lives == 0 {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
            println!("{:>2} {:<12} wins {}/6 deaths {:>2} avg time {:>5.1}s shots {:>4} killed by {:?}", n + 1, m.name, wins, deaths, time / 6.0, shots, causes);
        }
    }
}
