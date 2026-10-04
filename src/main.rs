mod ai;
mod arena;
mod audio;
mod game;
mod input;
mod lan;
mod net;
mod proto;
mod render;
mod world;

use game::{Game, Phase, Start, DT};
use input::Input;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::supports_keyboard_enhancement;
use render::Layout;
use std::io;
use std::time::{Duration, Instant};

const SNAPSHOT_EVERY: Duration = Duration::from_millis(500);
const FRAME: Duration = Duration::from_micros(16_667);

fn main() -> io::Result<()> {
    // Session logging: RUST_LOG=debug panzer 2>panzer.log
    env_logger::init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "panzer [--mute] [--mission N | --host | --join [ID]]\n\n\
             Toy tank combat in the terminal. In game, H shows the controls.\n\
             --host       host a two-player game on the local network\n\
             --join       look for games on the local network\n\
             --join ID    join a host by its iroh id (works across the internet)"
        );
        return Ok(());
    }
    let arg_after = |flag: &str| args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).filter(|v| !v.starts_with("--"));
    let start = if args.iter().any(|a| a == "--host") {
        Start::Host
    } else if args.iter().any(|a| a == "--join") {
        match arg_after("--join") {
            Some(id) => match id.parse::<iroh::EndpointId>() {
                Ok(id) => Start::Join(iroh::EndpointAddr::new(id)),
                Err(e) => {
                    eprintln!("not an iroh id: {id}: {e}");
                    std::process::exit(2);
                }
            },
            None => Start::Browse,
        }
    } else if let Some(n) = arg_after("--mission").and_then(|v| v.parse::<usize>().ok()) {
        Start::Mission(n.max(1) - 1)
    } else {
        Start::Title
    };
    // open audio before the alternate screen, so any ALSA chatter lands above it
    let mut audio = audio::Audio::new(args.iter().any(|a| a == "--mute"));
    log::info!("audio available {} muted {}", audio.available(), audio.muted);
    let mut terminal = ratatui::init();
    let kitty = supports_keyboard_enhancement().unwrap_or(false);
    if kitty {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES | KeyboardEnhancementFlags::REPORT_EVENT_TYPES | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
        execute!(io::stdout(), PushKeyboardEnhancementFlags(flags))?;
    }
    execute!(io::stdout(), EnableMouseCapture)?;
    log::info!("start kitty_keyboard={kitty}");
    let mut input = Input::new(kitty);
    input.open_pad();
    let mut game = Game::new(start);
    let result = run(&mut terminal, &mut game, &mut input, &mut audio);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    if kitty {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, game: &mut Game, input: &mut Input, audio: &mut audio::Audio) -> io::Result<()> {
    let mut last = Instant::now();
    let mut acc = 0.0f32;
    let mut snapshot = Instant::now();
    let mut layout = Layout::arena(terminal.get_frame().area());
    loop {
        let frame_start = Instant::now();
        input.poll_pad();
        while event::poll(Duration::ZERO)? {
            match event::read()? {
                Event::Key(k) => {
                    log::debug!("key {:?} {:?} {:?}", k.code, k.kind, k.modifiers);
                    input.on_key(k, Instant::now());
                }
                Event::Mouse(m) => {
                    if !matches!(m.kind, event::MouseEventKind::Moved | event::MouseEventKind::Drag(_)) {
                        log::debug!("mouse {:?} at {},{}", m.kind, m.column, m.row);
                    }
                    input.on_mouse(m);
                }
                Event::Resize(w, h) => log::info!("resize {w}x{h}"),
                _ => {}
            }
        }
        let now = Instant::now();
        acc += now.duration_since(last).as_secs_f32().min(0.1);
        last = now;
        while acc >= DT {
            acc -= DT;
            let l = layout;
            game.tick(input, |inp| inp.controls(Instant::now(), inp.mouse.and_then(|(c, r)| l.cell_to_world(c, r))), audio);
            if game.exit {
                return Ok(());
            }
        }
        terminal.draw(|f| {
            layout = Layout::arena(f.area());
            game.draw(f, input, &layout);
        })?;
        if snapshot.elapsed() >= SNAPSHOT_EVERY {
            snapshot = Instant::now();
            let w = &game.world;
            let p = &w.tanks[game.me.min(w.tanks.len() - 1)];
            if game.phase == Phase::Play {
                log::debug!(
                    "snapshot mission {} lives {} t {:.1} player ({:.2},{:.2}) alive {} turret {:.2} enemies {} shells {} mines {}",
                    game.mission + 1,
                    game.lives,
                    w.time,
                    p.x,
                    p.y,
                    p.alive,
                    p.turret,
                    w.enemies_left(),
                    w.shells.len(),
                    w.mines.len()
                );
            } else {
                log::debug!("snapshot phase {:?} mission {} lives {}", game.phase, game.mission + 1, game.lives);
            }
        }
        let spent = frame_start.elapsed();
        if spent < FRAME {
            // wait for input or the next frame, whichever comes first
            let _ = event::poll(FRAME - spent)?;
        }
    }
}
