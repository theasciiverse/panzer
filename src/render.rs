//! Drawing, all of it in half-block pixels (`▀`: two freely coloured pixels
//! per terminal cell, so pixels come out about square).
//!
//! The camera looks down at the toy box from slightly south, the way the
//! original's did: a block's top face is shifted up the screen by `LIFT` and
//! its south face shows below it. Everything with height is drawn the same
//! way (screen y = map y - height * LIFT), and a depth buffer keyed on map y
//! lets blocks hide what stands behind them.

use crate::ai::SPECS;
use crate::arena::{Tile, GH, GW};
use crate::world::{wrap, PKind, Rgb, World, STAIN};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

/// On-screen height of one tile of height, in tiles.
pub const LIFT: f32 = 0.45;
const FLOOR_D: f32 = -1e9;
const OVERLAY_D: f32 = 1e9;

/// Where the arena sits on the screen.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub area: Rect,
    pub ox: f32,
    pub oy: f32,
    /// Pixels per tile.
    pub s: f32,
}

impl Layout {
    /// The arena, as large as fits between the HUD row and the hint row.
    pub fn arena(full: Rect) -> Layout {
        let avail = Rect { y: full.y + 1, height: full.height.saturating_sub(2), ..full };
        let (ww, wh) = (GW as f32, GH as f32 + LIFT);
        let s = (avail.width as f32 / ww).min(avail.height as f32 * 2.0 / wh).max(0.5);
        let w = ((ww * s).ceil() as u16).min(avail.width);
        let h = ((wh * s / 2.0).ceil() as u16).min(avail.height);
        let area = Rect { x: avail.x + (avail.width - w) / 2, y: avail.y + (avail.height - h) / 2, width: w, height: h };
        Layout { area, ox: 0.0, oy: -LIFT, s }
    }

    /// The map point under a terminal cell (at shell height).
    pub fn cell_to_world(&self, col: u16, row: u16) -> Option<(f32, f32)> {
        let a = self.area;
        if col < a.x || row < a.y || col >= a.x + a.width || row >= a.y + a.height {
            return None;
        }
        let px = (col - a.x) as f32 + 0.5;
        let py = ((row - a.y) * 2) as f32 + 1.0;
        Some((self.ox + px / self.s, self.oy + py / self.s + 0.45 * LIFT))
    }
}

pub struct Canvas {
    w: usize,
    h: usize,
    px: Vec<Rgb>,
    depth: Vec<f32>,
    ox: f32,
    oy: f32,
    s: f32,
}

type Paint = Option<(Rgb, bool)>;

impl Canvas {
    pub fn new(l: &Layout) -> Canvas {
        let (w, h) = (l.area.width as usize, l.area.height as usize * 2);
        Canvas { w, h, px: vec![(0, 0, 0); w * h], depth: vec![FLOOR_D; w * h], ox: l.ox, oy: l.oy, s: l.s }
    }

    fn ps(&self) -> f32 {
        1.0 / self.s
    }

    /// Fill every pixel from `f(x, screen_y, pixel_size, pixel_hash)`.
    fn fill(&mut self, f: impl Fn(f32, f32, f32, u32) -> (Rgb, f32)) {
        let ps = self.ps();
        for py in 0..self.h {
            for px in 0..self.w {
                let (x, ys) = (self.ox + (px as f32 + 0.5) * ps, self.oy + (py as f32 + 0.5) * ps);
                let (c, d) = f(x, ys, ps, hash(px as i32, py as i32, 7));
                let i = py * self.w + px;
                self.px[i] = c;
                self.depth[i] = d;
            }
        }
    }

    /// Paint the shape `f` within `reach` of screen point (cx, cy), at `depth`.
    /// Each pixel is sampled 3x3: thin parts win if they touch it at all,
    /// otherwise the most common colour if it covers most of the pixel.
    fn paint(&mut self, cx: f32, cy: f32, reach: f32, depth: f32, dither: bool, f: impl Fn(f32, f32) -> Paint) {
        let ps = self.ps();
        let c0 = (((cx - reach - self.ox) / ps).floor() as i32).max(0);
        let r0 = (((cy - reach - self.oy) / ps).floor() as i32).max(0);
        let c1 = (((cx + reach - self.ox) / ps).ceil() as i32).min(self.w as i32 - 1);
        let r1 = (((cy + reach - self.oy) / ps).ceil() as i32).min(self.h as i32 - 1);
        for r in r0..=r1 {
            for c in c0..=c1 {
                if dither && (r + c) % 2 == 0 {
                    continue;
                }
                let i = r as usize * self.w + c as usize;
                if depth < self.depth[i] {
                    continue;
                }
                let (x0, y0) = (self.ox + c as f32 * ps, self.oy + r as f32 * ps);
                let mut samples: [Option<Rgb>; 9] = [None; 9];
                let mut thin = None;
                for (k, s) in samples.iter_mut().enumerate() {
                    let (sx, sy) = ((k % 3) as f32 + 0.5, (k / 3) as f32 + 0.5);
                    if let Some((rgb, is_thin)) = f(x0 + sx * ps / 3.0, y0 + sy * ps / 3.0) {
                        *s = Some(rgb);
                        if is_thin {
                            thin = Some(rgb);
                        }
                    }
                }
                let pick = thin.or_else(|| {
                    let covered: Vec<Rgb> = samples.iter().flatten().copied().collect();
                    if covered.len() < 4 {
                        return None;
                    }
                    covered.iter().max_by_key(|c| covered.iter().filter(|d| d == c).count()).copied()
                });
                if let Some(p) = pick {
                    self.px[i] = p;
                    self.depth[i] = depth;
                }
            }
        }
    }

    /// Darken floor pixels inside an ellipse (a shadow).
    fn shade(&mut self, cx: f32, cy: f32, rx: f32, ry: f32, k: f32) {
        let ps = self.ps();
        let c0 = (((cx - rx - self.ox) / ps).floor() as i32).max(0);
        let r0 = (((cy - ry - self.oy) / ps).floor() as i32).max(0);
        let c1 = (((cx + rx - self.ox) / ps).ceil() as i32).min(self.w as i32 - 1);
        let r1 = (((cy + ry - self.oy) / ps).ceil() as i32).min(self.h as i32 - 1);
        for r in r0..=r1 {
            for c in c0..=c1 {
                let (x, y) = (self.ox + (c as f32 + 0.5) * ps, self.oy + (r as f32 + 0.5) * ps);
                let i = r as usize * self.w + c as usize;
                if self.depth[i] == FLOOR_D && ((x - cx) / rx).powi(2) + ((y - cy) / ry).powi(2) <= 1.0 {
                    self.px[i] = scale(self.px[i], k);
                    // shade each pixel once
                    self.depth[i] = FLOOR_D + 1.0;
                }
            }
        }
    }

    /// Dim everything (behind an overlay card).
    pub fn dim(&mut self, k: f32) {
        for p in &mut self.px {
            *p = scale(*p, k);
        }
    }

    pub fn flush(&self, buf: &mut Buffer, area: Rect) {
        for row in 0..area.height as usize {
            for col in 0..area.width as usize {
                if col >= self.w || 2 * row + 1 >= self.h {
                    continue;
                }
                let (t, b) = (self.px[2 * row * self.w + col], self.px[(2 * row + 1) * self.w + col]);
                if let Some(cell) = buf.cell_mut((area.x + col as u16, area.y + row as u16)) {
                    cell.set_char('▀').set_fg(Color::Rgb(t.0, t.1, t.2)).set_bg(Color::Rgb(b.0, b.1, b.2));
                }
            }
        }
    }
}

// ------------------------------------------------------------------ colours

pub fn scale(c: Rgb, f: f32) -> Rgb {
    let s = |v: u8| (v as f32 * f).clamp(0.0, 255.0) as u8;
    (s(c.0), s(c.1), s(c.2))
}

pub fn mix(c: Rgb, to: Rgb, k: f32) -> Rgb {
    let k = k.clamp(0.0, 1.0);
    let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * k) as u8;
    (m(c.0, to.0), m(c.1, to.1), m(c.2, to.2))
}

fn add(c: Rgb, d: f32) -> Rgb {
    let s = |v: u8| (v as f32 + d).clamp(0.0, 255.0) as u8;
    (s(c.0), s(c.1), s(c.2))
}

fn hash(x: i32, y: i32, t: i32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ (t as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^ (h >> 15)
}

/// The playroom floor: wide planks, one tile per row, with staggered joints.
pub fn wood(x: f32, y: f32, ps: f32) -> Rgb {
    let row = y.floor() as i32;
    let off = (hash(row, 0, 1) % 300) as f32 / 100.0;
    let seg = ((x + off) / 3.0).floor() as i32;
    let h = hash(row, seg, 2);
    let tint = (h % 9) as f32 - 4.0;
    let grain = ((x * 3.1 + (h % 13) as f32).sin() * 0.6 + (y * 11.0 + x * 0.7 + (h % 7) as f32).sin()) * 2.0;
    let mut c = add((228, 196, 150), tint + grain);
    if ps < 0.4 {
        if y - y.floor() < ps * 0.99 {
            c = scale(c, 0.9);
        } else if ((x + off) / 3.0).fract() * 3.0 < ps * 0.99 {
            c = scale(c, 0.92);
        }
    }
    c
}

fn block_top(t: Tile, u: f32, v: f32, ps: f32, h: u32) -> Rgb {
    match t {
        Tile::Crate => {
            let edge = 0.09f32.max(ps);
            if u < edge || v < edge || u > 1.0 - edge || v > 1.0 - edge {
                if u < ps || v < ps {
                    (246, 222, 176)
                } else {
                    (186, 144, 94)
                }
            } else if (u - v).abs() < 0.07f32.max(ps * 0.7) {
                (206, 166, 112)
            } else if ps < 0.25 && ((v * 3.0).fract() < ps * 3.0) {
                (214, 178, 126)
            } else {
                (236, 204, 154)
            }
        }
        _ => {
            if u < ps || v < ps {
                (232, 196, 146)
            } else if u > 1.0 - ps || v > 1.0 - ps {
                (166, 122, 76)
            } else if h % 9 == 0 {
                (178, 136, 86)
            } else if h % 13 == 1 {
                (222, 180, 124)
            } else {
                (204, 160, 104)
            }
        }
    }
}

fn block_face(t: Tile, u: f32, v: f32, ps_v: f32, h: u32) -> Rgb {
    let bottom = v > 1.0 - ps_v;
    match t {
        Tile::Crate => {
            if bottom {
                (120, 88, 56)
            } else if ((u * 3.0).fract() * 3.0) < 0.12 {
                (166, 126, 80)
            } else {
                (194, 154, 102)
            }
        }
        _ => {
            if bottom {
                (92, 64, 40)
            } else if h % 7 == 0 {
                (124, 88, 54)
            } else {
                (150, 108, 66)
            }
        }
    }
}

/// One background pixel: block top, block face, pit or floor, with its depth.
fn background(w: &World, x: f32, ys: f32, ps: f32, h: u32) -> (Rgb, f32) {
    let a = &w.arena;
    let top_y = ys + LIFT;
    if top_y < GH as f32 {
        let t = a.at(x, top_y);
        if t.blocks_shells() {
            let (tx, ty) = (x.floor(), top_y.floor());
            return (block_top(t, x - tx, top_y - ty, ps, h), ty + 1.0);
        }
    }
    let t = a.at(x, ys);
    if t.blocks_shells() {
        let ty = ys.floor();
        let v = (ys - (ty + 1.0 - LIFT)) / LIFT;
        return (block_face(t, x - x.floor(), v, ps / LIFT, h), ty + 1.0);
    }
    if t == Tile::Pit {
        let (u, v) = (x - x.floor(), ys - ys.floor());
        let (tx, ty) = (x.floor() as i32, ys.floor() as i32);
        let open_n = a.tile(tx, ty - 1) != Tile::Pit;
        if open_n && v < LIFT * 0.85 {
            return (mix((112, 80, 50), (54, 38, 26), v / (LIFT * 0.85)), FLOOR_D);
        }
        let rim = (u < ps && a.tile(tx - 1, ty) != Tile::Pit) || (u > 1.0 - ps && a.tile(tx + 1, ty) != Tile::Pit) || (v > 1.0 - ps && a.tile(tx, ty + 1) != Tile::Pit);
        return (if rim { (150, 112, 74) } else { (30, 24, 20) }, FLOOR_D);
    }
    let mut c = wood(x, ys, ps);
    if let Some(i) = stain_index(x, ys) {
        let tr = w.tracks[i];
        if tr > 0.01 {
            c = scale(c, 1.0 - tr * 0.5);
        }
        let sc = w.scorch[i];
        if sc > 0.01 {
            c = mix(c, (52, 40, 32), sc);
        }
    }
    // blocks throw shadows to the south-east
    if a.at(x - 0.22, ys - 0.14).blocks_shells() {
        c = scale(c, 0.76);
    }
    (c, FLOOR_D)
}

fn stain_index(x: f32, y: f32) -> Option<usize> {
    let (sx, sy) = ((x * STAIN as f32).floor(), (y * STAIN as f32).floor());
    if sx < 0.0 || sy < 0.0 || sx >= (GW * STAIN) as f32 || sy >= (GH * STAIN) as f32 {
        return None;
    }
    Some(sy as usize * GW * STAIN + sx as usize)
}

// ------------------------------------------------------------------ sprites

#[derive(Clone, Copy)]
pub struct Pose {
    pub kind: usize,
    pub x: f32,
    pub y: f32,
    pub heading: f32,
    pub turret: f32,
    pub odometer: f32,
    pub muzzle: f32,
    pub dither: bool,
}

const HALF_LEN: f32 = 0.48;
const HALF_WID: f32 = 0.4;
const TRACK_W: f32 = 0.1;
const INK: Rgb = (34, 26, 22);
const HULL_Z: f32 = 0.3;
const TURRET_Z: f32 = 0.55;

pub fn paint_tank(cv: &mut Canvas, p: Pose) {
    let spec = &SPECS[p.kind];
    let depth = p.y + 0.45;
    let (hx, hy) = (p.heading.cos(), p.heading.sin());
    let hull_y = p.y - HULL_Z * LIFT;
    let local = move |x: f32, y: f32, cy: f32| {
        let (dx, dy) = (x - p.x, y - cy);
        (dx * hx + dy * hy, -dx * hy + dy * hx)
    };
    let side = scale(spec.body, 0.6);
    let ps = cv.ps();
    // small sprites cannot spare pixels for outlines and treads
    let detailed = ps < 0.16;
    let track_w = if detailed { TRACK_W } else { 0.0 };
    let ink = scale(spec.body, 0.3);
    // a cartoon outline around hull and sides, then the sides of the hull
    // under its top: what makes it a box and not a sticker
    let o = if detailed { ps * 0.7 } else { 0.0 };
    cv.paint(p.x, p.y, 0.8, depth, p.dither, |x, y| {
        let (l, w) = local(x, y, p.y);
        let (l2, w2) = local(x, y, hull_y);
        let inside = |l: f32, w: f32, m: f32| l.abs() <= HALF_LEN + m && w.abs() <= HALF_WID + m;
        if inside(l, w, 0.0) && !inside(l2, w2, 0.0) {
            Some((side, false))
        } else if inside(l, w, o) || inside(l2, w2, o) {
            Some((ink, false))
        } else {
            None
        }
    });
    let outline = scale(spec.body, 0.62);
    let front = mix(spec.body, (255, 255, 255), 0.22);
    cv.paint(p.x, hull_y, 0.7, depth + 0.01, p.dither, |x, y| {
        let (l, w) = local(x, y, hull_y);
        if l.abs() > HALF_LEN || w.abs() > HALF_WID {
            return None;
        }
        if w.abs() >= HALF_WID - track_w {
            // tread bands are fixed to the ground, so they roll back as the tank drives
            let band = ((l + p.odometer) / 0.13).floor().rem_euclid(2.0) == 0.0;
            return Some((if band { (104, 98, 90) } else { (62, 58, 54) }, false));
        }
        if detailed && (l.abs() > HALF_LEN - ps * 0.8 || w.abs() > HALF_WID - track_w - ps * 0.8) {
            return Some((outline, false));
        }
        Some((if l > HALF_LEN - 0.14 { front } else { spec.body }, false))
    });
    // turret and barrel
    let ty = p.y - TURRET_Z * LIFT;
    let (ax, ay) = (p.turret.cos(), p.turret.sin());
    let barrel = scale(spec.turret, 0.78);
    let half_b = (ps * 0.4).max(0.05);
    let tr = 0.25;
    cv.paint(p.x, ty, 1.0, depth + 0.02, p.dither, |x, y| {
        let (dx, dy) = (x - p.x, y - ty);
        let (l, w) = (dx * ax + dy * ay, -dx * ay + dy * ax);
        if p.muzzle > 0.0 && (l - 0.7).hypot(w) <= 0.06 + p.muzzle * 1.4 {
            return Some(((255, 244, 190), true));
        }
        if (0.0..=0.64).contains(&l) && w.abs() <= half_b {
            return Some((if l > 0.54 { INK } else { barrel }, true));
        }
        let r = dx.hypot(dy);
        if detailed && r <= tr + ps * 0.6 && r > tr {
            return Some((ink, false));
        }
        if r <= tr {
            let c = if r <= 0.07 {
                scale(spec.turret, 0.7)
            } else if dx + dy < -0.08 {
                mix(spec.turret, (255, 255, 255), 0.25)
            } else {
                spec.turret
            };
            return Some((c, false));
        }
        None
    });
}

pub fn draw_world(cv: &mut Canvas, w: &World, aim: Option<(f32, f32)>, aim_line: bool) {
    cv.fill(|x, ys, ps, h| background(w, x, ys, ps, h));
    let t = w.time;
    // shadows first, so everything stands on them
    for tank in w.tanks.iter().filter(|t| t.alive) {
        if !(tank.spec().invisible && tank.reveal <= 0.0) {
            cv.shade(tank.x + 0.1, tank.y + 0.08, 0.5, 0.42, 0.72);
        }
    }
    for s in &w.shells {
        cv.shade(s.x + 0.06, s.y + 0.06, 0.09, 0.07, 0.75);
    }
    // mines
    for m in &w.mines {
        let period = if m.fuse > 2.5 { 1.0 } else { 0.24 };
        let lit = (t % period) < period * 0.5;
        cv.paint(m.x, m.y, 0.35, m.y, false, |x, y| {
            let r = (x - m.x).hypot(y - m.y);
            if r > 0.29 {
                None
            } else if r < 0.08 {
                Some((if lit { (255, 60, 40) } else { (120, 24, 14) }, true))
            } else if r > 0.23 {
                Some(((110, 82, 24), false))
            } else {
                Some(((236, 194, 44), false))
            }
        });
    }
    // tanks
    for tank in w.tanks.iter().filter(|t| t.alive) {
        let ghost = tank.spec().invisible;
        if ghost && tank.reveal <= 0.0 {
            continue;
        }
        paint_tank(
            cv,
            Pose { kind: tank.kind, x: tank.x, y: tank.y, heading: tank.heading, turret: tank.turret, odometer: tank.odometer, muzzle: tank.muzzle, dither: ghost },
        );
    }
    // shells
    for s in &w.shells {
        let sy = s.y - 0.45 * LIFT;
        let sp = s.vx.hypot(s.vy);
        let (ux, uy) = (s.vx / sp, s.vy / sp);
        let (len, half) = if s.rocket { (0.2, 0.08) } else { (0.15, 0.07) };
        let flicker = hash((t * 30.0) as i32, s.id as i32, 3) % 2 == 0;
        cv.paint(s.x, sy, 0.5, s.y + 0.2, false, |x, y| {
            let (dx, dy) = (x - s.x, y - sy);
            let (l, wd) = (dx * ux + dy * uy, -dx * uy + dy * ux);
            if wd.abs() > half {
                return None;
            }
            if s.rocket && (-len - 0.18..-len).contains(&l) && wd.abs() <= half * 0.8 {
                return Some((if flicker { (255, 214, 80) } else { (255, 150, 40) }, true));
            }
            if !(-len..=len).contains(&l) {
                return None;
            }
            let c = match (s.rocket, l > len * 0.45) {
                (true, true) => (240, 226, 210),
                (true, false) => (196, 56, 40),
                (false, true) => (178, 180, 190),
                (false, false) => (58, 60, 70),
            };
            Some((c, true))
        });
    }
    // smoke, fire, sparks, debris
    for p in &w.particles {
        let f = p.life / p.max;
        let (c, r, thin) = match p.kind {
            PKind::Smoke => (mix((244, 244, 238), (196, 194, 188), 1.0 - f), p.size * (0.6 + 0.8 * (1.0 - f)) * (f * 3.0).min(1.0), false),
            PKind::Fire => (mix((255, 236, 120), (220, 70, 30), 1.0 - f), p.size * f.sqrt(), false),
            PKind::Spark => ((255, 240, 160), 0.05, true),
            PKind::Debris(c) => (c, 0.07, true),
        };
        if r < 0.02 {
            continue;
        }
        let py = p.y - p.z * LIFT;
        cv.paint(p.x, py, r + 0.05, p.y + 0.3, false, |x, y| ((x - p.x).hypot(y - py) <= r).then_some((c, thin)));
    }
    // explosions
    for b in &w.blasts {
        let grow = (b.age / 0.15).min(1.0);
        let r = b.radius * (1.0 - (1.0 - grow).powi(2)) * (1.0 - ((b.age - 0.3) / 0.4).clamp(0.0, 1.0) * 0.6);
        let by = b.y - 0.3 * LIFT;
        let late = b.age > 0.4;
        cv.paint(b.x, by, r + 0.1, b.y + r, late, |x, y| {
            let d = (x - b.x).hypot(y - by) / r.max(0.01);
            if d > 1.0 {
                return None;
            }
            let wob = ((y - by).atan2(x - b.x) * 5.0 + b.age * 20.0).sin() * 0.08;
            let c = if d < 0.35 + wob {
                (255, 252, 214)
            } else if d < 0.7 + wob {
                (255, 186, 56)
            } else {
                (226, 84, 32)
            };
            Some((c, false))
        });
    }
    // the aim: a dotted line from the barrel to the pointer, as in the original
    let p = &w.tanks[0];
    if p.alive && aim_line {
        let ty = p.y - TURRET_Z * LIFT;
        let (tx, tyy, max) = match aim {
            Some((ax, ay)) => (ax, ay, (ax - p.x).hypot(ay - p.y)),
            None => (p.x + p.turret.cos() * 3.0, p.y + p.turret.sin() * 3.0, 3.0),
        };
        let ang = (tyy - p.y).atan2(tx - p.x);
        let end = w.ray_to_wall(p.x, p.y, ang, max);
        let len = (end.0 - p.x).hypot(end.1 - p.y);
        let mut d = 0.7;
        let blue = (40, 84, 200);
        while d < len {
            let (dx, dy) = (p.x + ang.cos() * d, ty + ang.sin() * d);
            cv.paint(dx, dy, 0.1, OVERLAY_D, false, |x, y| ((x - dx).hypot(y - dy) <= 0.05).then_some((blue, true)));
            d += 0.42;
        }
        if let Some((ax, ay)) = aim {
            let ay = ay - 0.45 * LIFT;
            let ps = cv.ps();
            cv.paint(ax, ay, 0.45, OVERLAY_D, false, |x, y| {
                let r = (x - ax).hypot(y - ay);
                ((r - 0.3).abs() <= ps * 0.5 || r < 0.06).then_some((blue, true))
            });
        }
    }
}

// --------------------------------------------------------------------- title

const LOGO: [&str; 7] = [
    "####...###..#...#.#####.#####.####.",
    "#...#.#...#.##..#.....#.#.....#...#",
    "#...#.#...#.#.#.#....#..#.....#...#",
    "####..#####.#..##...#...####..####.",
    "#.....#...#.#...#..#....#.....#.#..",
    "#.....#...#.#...#.#.....#.....#..#.",
    "#.....#...#.#...#.#####.#####.#...#",
];

/// The title screen picture: the floor, PANZER spelt in toy blocks (each
/// letter in a tank's colours), and the whole roster rolling past.
pub fn draw_title(buf: &mut Buffer, area: Rect, time: f32) {
    // tanks at a friendly size; the logo scales to fit the width
    let s = (area.height as f32 * 2.0 / 13.0).clamp(4.0, 12.0);
    let l = Layout { area, ox: 0.0, oy: 0.0, s };
    let mut cv = Canvas::new(&l);
    let ww = area.width as f32 / s;
    let wh = area.height as f32 * 2.0 / s;
    let logo_w = LOGO[0].len() as f32;
    let c = (ww * 0.86 / logo_w).min(wh * 0.3 / LOGO.len() as f32);
    let lx = (ww - logo_w * c) / 2.0;
    let ly = wh * 0.08 + c * LIFT;
    let lift = LIFT * c;
    // logo cell under a point: (column, row, letter)
    let cell = |x: f32, y: f32| -> Option<(f32, f32, usize)> {
        let (cx, cy) = (((x - lx) / c).floor(), ((y - ly) / c).floor());
        if cx < 0.0 || cy < 0.0 || cy as usize >= LOGO.len() || cx as usize >= LOGO[0].len() {
            return None;
        }
        (LOGO[cy as usize].as_bytes()[cx as usize] == b'#').then_some((cx, cy, cx as usize / 6))
    };
    const LETTERS: [usize; 6] = [0, 1, 3, 4, 5, 6];
    cv.fill(|x, ys, ps, h| {
        if let Some((cx, cy, letter)) = cell(x, ys + lift) {
            let (u, v) = ((x - lx) / c - cx, (ys + lift - ly) / c - cy);
            let body = SPECS[LETTERS[letter.min(5)]].body;
            let pc = ps / c;
            let col = if u < pc || v < pc {
                mix(body, (255, 255, 255), 0.35)
            } else if u > 1.0 - pc || v > 1.0 - pc {
                scale(body, 0.75)
            } else if h % 11 == 0 {
                scale(body, 0.9)
            } else {
                body
            };
            return (col, ly + (cy + 1.0) * c);
        }
        if let Some((_, cy, letter)) = cell(x, ys) {
            let v = (ys - (ly + (cy + 1.0) * c - lift)) / lift;
            let body = SPECS[LETTERS[letter.min(5)]].body;
            let col = if v > 1.0 - ps / lift { scale(body, 0.35) } else { scale(body, 0.6) };
            return (col, ly + (cy + 1.0) * c);
        }
        let mut col = wood(x, ys, ps);
        if cell(x - 0.22 * c, ys - 0.14 * c).is_some() {
            col = scale(col, 0.76);
        }
        (col, FLOOR_D)
    });
    // the parade
    let row = wh - 1.3;
    let span = ww + 4.0;
    for k in 0..SPECS.len() {
        let x = (time * 1.4 + k as f32 * span / SPECS.len() as f32).rem_euclid(span) - 2.0;
        let turret = (time * 1.3 + k as f32).sin() * 0.9;
        let ghost = SPECS[k].invisible;
        if ghost && (time * 0.7).sin() < 0.0 {
            continue;
        }
        cv.shade(x + 0.1, row + 0.08, 0.55, 0.46, 0.72);
        paint_tank(&mut cv, Pose { kind: k, x, y: row, heading: 0.0, turret: wrap(turret), odometer: time * 1.4, muzzle: 0.0, dither: ghost });
    }
    cv.flush(buf, area);
}

/// A full-screen canvas with the arena drawn, for screens over the battle.
pub fn draw_arena(buf: &mut Buffer, layout: &Layout, w: &World, aim: Option<(f32, f32)>, aim_line: bool, dim: Option<f32>) {
    let mut cv = Canvas::new(layout);
    draw_world(&mut cv, w, aim, aim_line);
    if let Some(k) = dim {
        cv.dim(k);
    }
    cv.flush(buf, layout.area);
}
