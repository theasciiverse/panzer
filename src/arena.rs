//! The arena: a tile grid with a solid border, and the mission list it is
//! parsed from (`data/missions.txt`, compiled into the binary).
//!
//! Mission format: a `== Name` line starts a mission, `> text` lines are its
//! briefing, then exactly `H` rows of `W` characters:
//!
//! - `.` floor, `#` block, `%` crate (mines break it), `o` pit (stops tanks, not shells)
//! - `P` the player, `1`-`9` enemy tanks (the roster in `ai.rs`)
//!
//! Lines starting with `;` are comments.

pub const W: usize = 22;
pub const H: usize = 16;
/// Grid size including the border wall.
pub const GW: usize = W + 2;
pub const GH: usize = H + 2;

pub static MISSIONS_TXT: &str = include_str!("../data/missions.txt");

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tile {
    Floor,
    Block,
    Crate,
    Pit,
}

impl Tile {
    pub fn blocks_tanks(self) -> bool {
        self != Tile::Floor
    }
    pub fn blocks_shells(self) -> bool {
        matches!(self, Tile::Block | Tile::Crate)
    }
}

#[derive(Clone)]
pub struct Arena {
    tiles: Vec<Tile>,
}

impl Arena {
    pub fn tile(&self, tx: i32, ty: i32) -> Tile {
        if tx < 0 || ty < 0 || tx >= GW as i32 || ty >= GH as i32 {
            return Tile::Block;
        }
        self.tiles[ty as usize * GW + tx as usize]
    }

    pub fn at(&self, x: f32, y: f32) -> Tile {
        self.tile(x.floor() as i32, y.floor() as i32)
    }

    pub fn set(&mut self, tx: i32, ty: i32, t: Tile) {
        // the border is permanent
        if tx >= 1 && ty >= 1 && tx <= W as i32 && ty <= H as i32 {
            self.tiles[ty as usize * GW + tx as usize] = t;
        }
    }

    pub fn walkable(&self, tx: i32, ty: i32) -> bool {
        !self.tile(tx, ty).blocks_tanks()
    }
}

#[derive(Clone)]
pub struct Mission {
    pub name: String,
    pub brief: String,
    pub arena: Arena,
    /// Player spawn, tile centre.
    pub player: (f32, f32),
    /// Enemy kind (1-9) and tile centre.
    pub enemies: Vec<(usize, f32, f32)>,
}

pub fn parse_missions(text: &str) -> Result<Vec<Mission>, String> {
    let mut out = Vec::new();
    let mut lines = text.lines().enumerate().filter(|(_, l)| !l.starts_with(';')).peekable();
    while let Some((n, line)) = lines.next() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let Some(name) = line.strip_prefix("== ") else {
            return Err(format!("line {}: expected `== Mission name`, got {line:?}", n + 1));
        };
        let mut brief = Vec::new();
        while let Some((_, l)) = lines.peek() {
            if let Some(b) = l.strip_prefix("> ") {
                brief.push(b.trim().to_string());
                lines.next();
            } else {
                break;
            }
        }
        let mut tiles = vec![Tile::Block; GW * GH];
        let mut player = None;
        let mut enemies = Vec::new();
        for row in 0..H {
            let Some((ln, l)) = lines.next() else {
                return Err(format!("mission {name:?}: map ends after {row} rows"));
            };
            let l = l.trim_end();
            if l.chars().count() != W {
                return Err(format!("line {}: mission {name:?} row {row} is {} wide, not {W}", ln + 1, l.chars().count()));
            }
            for (col, ch) in l.chars().enumerate() {
                let (tx, ty) = (col + 1, row + 1);
                let centre = (tx as f32 + 0.5, ty as f32 + 0.5);
                tiles[ty * GW + tx] = match ch {
                    '#' => Tile::Block,
                    '%' => Tile::Crate,
                    'o' => Tile::Pit,
                    '.' => Tile::Floor,
                    'P' => {
                        player = Some(centre);
                        Tile::Floor
                    }
                    '1'..='9' => {
                        enemies.push((ch as usize - '0' as usize, centre.0, centre.1));
                        Tile::Floor
                    }
                    _ => return Err(format!("line {}: unknown map character {ch:?}", ln + 1)),
                };
            }
        }
        let player = player.ok_or_else(|| format!("mission {name:?} has no player spawn P"))?;
        if enemies.is_empty() {
            return Err(format!("mission {name:?} has no enemy tanks"));
        }
        out.push(Mission { name: name.to_string(), brief: brief.join(" "), arena: Arena { tiles }, player, enemies });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_missions_parse() {
        let m = parse_missions(MISSIONS_TXT).unwrap();
        assert_eq!(m.len(), 20, "the campaign has 20 missions");
    }
}
