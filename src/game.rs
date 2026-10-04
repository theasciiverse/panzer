//! The campaign around the battles: title, mission cards, lives, the
//! results, and the HUD and cards drawn over the arena.

use crate::ai::SPECS;
use crate::arena::{parse_missions, Mission, MISSIONS_TXT};
use crate::audio::{Audio, Engine, Music, Sfx, SLOTS};
use crate::input::{Cmd, Input};
use crate::render::{self, Layout};
use crate::world::{Controls, Event, World};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::Frame;

pub const DT: f32 = 1.0 / 60.0;
const START_LIVES: u32 = 3;
/// A spare tank after clearing every this many missions.
const BONUS_EVERY: usize = 5;

const AMBER: Color = Color::Rgb(255, 180, 40);
const GREEN: Color = Color::Rgb(90, 220, 120);
const DIM: Color = Color::Rgb(120, 130, 120);
const INK: Color = Color::Rgb(16, 18, 16);
const PAPER: Color = Color::Rgb(232, 226, 210);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Title,
    Intro,
    Play,
    Paused,
    Won,
    Lost,
    Over,
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
    /// Tanks destroyed in this campaign, by roster kind.
    pub tally: [u32; 10],
    pub world: World,
    pub pick: usize,
    pub help: bool,
    pub victory: bool,
    pub bonus: bool,
    pub time: f32,
    seed: u64,
    pub exit: bool,
}

impl Game {
    pub fn new(start: Option<usize>) -> Game {
        let missions = parse_missions(MISSIONS_TXT).expect("bundled missions parse");
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64);
        let world = World::new(&missions[0], &[], seed);
        let mut g = Game {
            missions,
            phase: Phase::Title,
            phase_t: 0.0,
            mission: 0,
            lives: START_LIVES,
            gone: Vec::new(),
            tally: [0; 10],
            world,
            pick: 0,
            help: false,
            victory: false,
            bonus: false,
            time: 0.0,
            seed,
            exit: false,
        };
        if let Some(m) = start {
            g.pick = m.min(g.missions.len() - 1);
            g.start_campaign();
        }
        g
    }

    fn set_phase(&mut self, p: Phase) {
        log::info!("phase {:?} -> {:?} (mission {}, lives {})", self.phase, p, self.mission + 1, self.lives);
        self.phase = p;
        self.phase_t = 0.0;
    }

    fn start_campaign(&mut self) {
        self.mission = self.pick;
        self.lives = START_LIVES;
        self.tally = [0; 10];
        self.gone.clear();
        self.victory = false;
        self.begin_mission();
    }

    fn begin_mission(&mut self) {
        self.seed = self.seed.wrapping_add(0x9e37_79b9);
        self.world = World::new(&self.missions[self.mission], &self.gone, self.seed);
        self.set_phase(Phase::Intro);
    }

    /// Handle commands, advance one tick (`DT`), and play the sounds that go with it.
    pub fn tick(&mut self, input: &mut Input, controls: impl FnOnce(&mut Input) -> Controls, audio: &mut Audio) {
        self.time += DT;
        self.phase_t += DT;
        let cmds: Vec<Cmd> = input.cmds.drain(..).collect();
        for c in &cmds {
            match c {
                Cmd::Exit => self.exit = true,
                Cmd::Mute => audio.toggle_mute(),
                Cmd::Music => audio.toggle_music(),
                Cmd::Help if self.phase != Phase::Play => self.help = !self.help,
                Cmd::Help => {
                    self.help = true;
                    self.set_phase(Phase::Paused);
                }
                _ => {}
            }
        }
        if self.help && cmds.iter().any(|c| matches!(c, Cmd::Back | Cmd::Confirm)) {
            self.help = false;
            input.clear();
            return;
        }
        let confirm = cmds.contains(&Cmd::Confirm);
        let back = cmds.contains(&Cmd::Back);
        match self.phase {
            Phase::Title => {
                let n = self.missions.len();
                for c in &cmds {
                    match c {
                        Cmd::Left | Cmd::Down => self.pick = (self.pick + n - 1) % n,
                        Cmd::Right | Cmd::Up => self.pick = (self.pick + 1) % n,
                        _ => {}
                    }
                    if matches!(c, Cmd::Left | Cmd::Right | Cmd::Up | Cmd::Down) {
                        audio.play(Sfx::Select, 0.6);
                    }
                }
                if back && !self.help {
                    self.exit = true;
                } else if confirm && !self.help {
                    input.clear();
                    self.start_campaign();
                }
            }
            Phase::Intro => {
                if self.phase_t < DT * 1.5 {
                    audio.play(Sfx::Start, 0.8);
                }
                if back {
                    self.set_phase(Phase::Title);
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
                let c = controls(input);
                self.world.step(DT, c);
                self.sounds(audio);
                match self.world.outcome() {
                    Some(true) => {
                        self.gone.clear();
                        self.bonus = (self.mission + 1) % BONUS_EVERY == 0 && self.mission + 1 < self.missions.len();
                        if self.bonus {
                            self.lives += 1;
                        }
                        audio.play(Sfx::Clear, 0.9);
                        self.set_phase(Phase::Won);
                    }
                    Some(false) => {
                        self.gone.extend(self.world.kills.iter().copied());
                        self.lives = self.lives.saturating_sub(1);
                        audio.play(Sfx::Down, 0.9);
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
                    self.set_phase(Phase::Title);
                }
            }
            Phase::Won => {
                if self.bonus && (self.phase_t - 1.0).abs() < DT * 0.6 {
                    audio.play(Sfx::Life, 0.8);
                }
                // keep the smoke drifting while the card is up
                self.world.step(DT, Controls::default());
                self.world.events.clear();
                if self.phase_t > 3.5 || (confirm && self.phase_t > 0.8) {
                    input.clear();
                    if self.mission + 1 >= self.missions.len() {
                        self.victory = true;
                        self.set_phase(Phase::Over);
                    } else {
                        self.mission += 1;
                        self.begin_mission();
                    }
                }
            }
            Phase::Lost => {
                self.world.step(DT, Controls::default());
                self.world.events.clear();
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
                    self.set_phase(Phase::Title);
                }
            }
        }
        self.update_band(audio);
    }

    fn sounds(&mut self, audio: &mut Audio) {
        for e in self.world.events.drain(..) {
            match e {
                Event::Shot { rocket } => audio.play(if rocket { Sfx::Rocket } else { Sfx::Shot }, 0.7),
                Event::Bounce => audio.play(Sfx::Bounce, 0.6),
                Event::Fizzle => audio.play(Sfx::Fizzle, 0.5),
                Event::Cancel => audio.play(Sfx::Cancel, 0.7),
                Event::Empty => audio.play(Sfx::Click, 0.6),
                Event::TankDown { kind } => {
                    audio.play(Sfx::Boom, if kind == 0 { 1.0 } else { 0.85 });
                    if kind != 0 {
                        self.tally[kind] += 1;
                    }
                }
                Event::MineSet => audio.play(Sfx::MineSet, 0.7),
                Event::MineBeep => audio.play(Sfx::MineBeep, 0.5),
                Event::MineBoom => audio.play(Sfx::MineBoom, 1.0),
                Event::CrateBroken => audio.play(Sfx::Crate, 0.7),
            }
        }
    }

    /// Engines and music follow the battle.
    fn update_band(&self, audio: &mut Audio) {
        let mut engines = [Engine::default(); SLOTS];
        if self.phase == Phase::Play {
            for (slot, t) in engines.iter_mut().zip(self.world.tanks.iter()) {
                if t.alive {
                    *slot = Engine { kind: t.kind as u8 + 1, speed: t.speed };
                }
            }
        }
        audio.set_engines(&engines);
        match self.phase {
            Phase::Title | Phase::Over => audio.set_music(Music::Title, 0),
            Phase::Play => {
                let mut layers = 0u16;
                for t in self.world.tanks[1..].iter().filter(|t| t.alive) {
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
        if self.phase == Phase::Title {
            render::draw_title(frame.buffer_mut(), full, self.time);
            self.draw_title_text(frame, full);
            if self.help {
                help_card(frame, full);
            }
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
        render::draw_arena(frame.buffer_mut(), layout, &self.world, mouse_aim, self.phase == Phase::Play, dim);
        self.draw_hud(frame, full);
        match self.phase {
            Phase::Intro => self.intro_card(frame, full),
            Phase::Paused if !self.help => card(frame, full, "PAUSED", vec![Line::from(""), Line::from("P or Space: carry on"), Line::from("Esc: back to the title"), Line::from("H: help")], AMBER),
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
    }

    fn draw_hud(&self, frame: &mut Frame, full: Rect) {
        let w = &self.world;
        let p = &w.tanks[0];
        let spec = p.spec();
        let shells = spec.max_shells - w.shells_of(0).min(spec.max_shells);
        let mines = spec.max_mines - w.mines_of(0).min(spec.max_mines);
        let total = w.tanks.len() - 1 + self.gone.len();
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
        for t in self.world.tanks[1..].iter() {
            counts[t.kind] += 1;
        }
        let seen_before: Vec<bool> = (0..10).map(|k| self.missions[..self.mission].iter().any(|m| m.enemies.iter().any(|e| e.0 == k))).collect();
        let mut lines = vec![
            Line::styled(m.name.clone(), Style::new().fg(PAPER).add_modifier(Modifier::BOLD)),
            Line::from(""),
            Line::styled(m.brief.clone(), Style::new().fg(PAPER)),
            Line::from(""),
            Line::from(format!("Enemy tanks: {}", self.world.tanks.len() - 1)),
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
        let lines = vec![
            Line::styled("Toy tanks. Real ricochets.", Style::new().fg(PAPER).add_modifier(Modifier::BOLD)),
            Line::from(""),
            Line::from(vec![
                Span::styled("◀  ", Style::new().fg(AMBER)),
                Span::styled(format!("Start at mission {:>2}: {}", self.pick + 1, m.name), Style::new().fg(PAPER).add_modifier(Modifier::BOLD)),
                Span::styled("  ▶", Style::new().fg(AMBER)),
            ]),
            Line::from(""),
            Line::styled("Space or Enter: start    Left/Right: choose mission    H: help    Esc: quit", Style::new().fg(DIM)),
            Line::styled("An Asciiverse game, after Wii Play Tanks (Nintendo, 2006)", Style::new().fg(DIM)),
        ];
        let h = lines.len() as u16 + 2;
        let w = 84.min(full.width);
        let area = Rect { x: full.x + (full.width - w) / 2, y: full.y + full.height * 50 / 100, width: w, height: h.min(full.height) };
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
        Line::from(""),
        Line::styled("Esc or Space to close", Style::new().fg(DIM)),
    ];
    card(frame, full, "HOW TO PLAY", lines, GREEN);
}
