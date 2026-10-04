# Panzer

Asciiverse top-down tank combat game. Rust + Ratatui, half-block pixels.
Spiritual successor to Wii Play Tanks (Nintendo, 2006): original IP, no Nintendo assets.
Swedish: pansar = armour. German: panzer = armour/tanks. Same Germanic root.
The word carries the whole history of armoured warfare in six letters.

## Name

Panzer: Swedish and German simultaneously, shared Germanic root ancestry.
Resonates across Swedish, Norwegian, Danish, German, Dutch language families.
Hard, punchy, memorable. Carries weight without being serious.
Culturally loaded in exactly the right way for a comic tank game.

## The Wii Play Tanks inspiration

Not a port: Nintendo's IP, active, untouchable. An original game carrying
the design wisdom forward:

- **Ricocheting bullets off walls**: the geometry puzzle underneath the action game.
  The soul of what made Wii Play Tanks extraordinary.
- **Enemy tanks with distinct AI personalities**: aggressive, cautious, stationary-but-deadly,
  erratic. Each readable from movement behaviour alone.
- **Escalating arena complexity**: simple layouts teaching mechanics, complex layouts
  demanding mastery.
- **Comic feel**: toy tanks with personality, not serious military hardware.
  Genuinely challenging gameplay wearing a completely unserious aesthetic.
  Stressed and smiling simultaneously.

## The tonal combination

Wii Play Tanks separated itself from every other tank game by committing fully
to the comic register. Not gritty, not realistic, not trying to be serious.
Toy tanks. Personality. Warmth.

That combination, genuine challenge + unserious aesthetic, is what made it
irresistible. Panzer inherits this completely. The terminal aesthetic actually
serves it: box-drawing geometry for tanks, Braille dot trails for ricochets,
cartoon explosion characters. Clean, geometric, playful.

## Sound is load-bearing

This is the Asciiverse title that most demands the audio layer be done properly.
Not atmospheric (Terminal Orbit), not functional (Rusty Pinball):
genuinely characterful, comic, expressive audio that gives each tank a personality.

Enemy tanks should have distinct sound signatures that telegraph their AI
personality before the player even sees them clearly: the nervous shuffling
engine note of the cautious type, the aggressive revving of the charger type.
Game design and audio design working together, not audio as an afterthought
layered on top.

(The plan above imagined box-drawing tanks and Braille ricochet trails; the
build went with half-block pixels instead, see Design decisions.)

## Status (2026-10-04)

Playable from title to credits:

- 20-mission campaign (`data/missions.txt`), three tanks to start, a spare
  tank after every fifth mission, and (as in the original) enemies you
  destroyed stay destroyed when you lose a tank and retry.
- Nine original enemy personalities, ricocheting shells, shells cancelling
  each other in the air, mines (timed, proximity, shootable, chain), crates
  that mines break, pits that stop tanks but not shells.
- Tread marks that fade, scorch crosses where tanks died, smoke trails,
  explosions, a dotted aim line to the mouse pointer.
- Fully procedural audio: every tank type has its own engine voice, and the
  march gains one instrument per enemy type on the field.
- Keyboard, mouse and twin-stick gamepad. Session logging per the
  Asciiverse convention.

- Two-player co-op over the LAN (iroh), found automatically, see
  Multiplayer.

Not built yet: versus mode, a mission editor, a sextant renderer for small
terminals (see Next).

## Running

```bash
cargo run --release                 # title screen
cargo run --release -- --mission 9  # straight into a mission
cargo run --release -- --mute
cargo run --release -- --host       # host a LAN game (or pick it on the title)
cargo run --release -- --join       # find LAN games; --join ID dials an iroh id directly
RUST_LOG=debug ./target/release/panzer 2>panzer.log   # session log
```

Needs a truecolour terminal. 160x50 is playable, 200x60 or more is
comfortable (the arena scales to fit; tank outlines and treads appear once
there are enough pixels for them).

Controls: WASD/arrows drive; the mouse aims (or IJKL eight ways, or Q/E to
turn slowly); left click or Space fires (five shells at most); right click or
F drops a mine (two at most); P/Esc pause; H help; M music; F8 all sound.
Gamepad: left stick drives, right stick aims, right trigger fires, left
trigger or A mines. Without the kitty keyboard protocol a held key is
inferred from key repeats, as in Calvary.

## The roster (`src/ai.rs`)

Map digits 1-9, in order of menace. Each is meant to be readable from how it
moves and sounds before you see its colour.

| # | Name | Moves | Shoots | Engine sound |
|---|---|---|---|---|
| 1 | Tinpot | never | sweeps its turret, fires when a sweep lines up (often off a wall) | sleepy idle |
| 2 | Plodder | wanders slowly | sweeping, slow | heavy thumps |
| 3 | Hornet | keeps about 7 tiles away | rockets: fast, no ricochet | buzzing whine |
| 4 | Skitter | nervous dashes, flees when close | sweeping; lays mines | twitchy, never the same pitch |
| 5 | Rascal | charges | bursts of three | revs as it charges |
| 6 | Sniper | never | searches every angle for a two-bounce rocket path | a patient clock tick |
| 7 | Viper | fast, closes in, dodges | bank shots, four shells; the odd mine | angry growling rev |
| 8 | Ghost | wanders | direct; invisible except tread marks and a shimmer when it fires | theremin hum |
| 9 | Onyx | fast charger, dodges everything | rockets | deep beating growl |

The AI fires by tracing where a shell fired right now would go, bounces
included, and pulling the trigger when that path ends in the player and not
in a friend. Movement uses breadth-first path fields over the tile grid.
Friendly fire is real, as in the original.

## Multiplayer (built 2026-10-04)

Two-player co-op through the campaign, one machine each (split screen was
ruled out: the arena already needs every pixel). Player 2 drives a red tank
spawned next to player 1; the spare tanks are shared and a mission fails only
when both tanks are down. Enemies go for the nearest living player.

- **Host-authoritative.** The host runs the only simulation. The guest sends
  its `Controls` every tick (fire and mine presses are kept until the host
  uses them) and menu presses (confirm, pause). The host sends a `Frame`
  every tick: the campaign state plus a `world::Snapshot` (tanks, shells,
  mines, and the events since the last frame). The guest replays the events
  through `World::fx`, so smoke, sparks, blasts, scorch, broken crates and
  sounds match, and stamps its own tread marks. A full frame is under 2.5 kB.
  Chosen over lockstep because the sim leans on sin/cos/atan2, which are not
  guaranteed bit-identical across platforms.
- **Network code is Tetrix's** (`src/lan.rs`, `src/net.rs`): a UDP beacon on
  port 31458 for discovery (iroh cannot browse), iroh/QUIC for the
  connection, postcard frames, 6 s idle timeout. Works with no internet.
- **Flow:** title menu Host / Join; the host's lobby picks the mission and
  starts (alone if nobody joined); the guest's screen follows the host's
  phase. Joining is refused once a campaign is under way. Host Esc from
  pause or game over returns to the lobby with the guest still connected.

Verified 2026-10-04 with two processes on one machine over real iroh:
discovery, lobby, mission start, guest driving and firing seen identically on
both screens, guest pause and resume, host quitting (guest back at the title
within about a second), guest killed (host notices in 6 s and plays on).
Tests: `guest_mirrors_the_host` runs 900 ticks of a two-player battle with
mines and checks the guest matches tank for tank, shell for shell, tile for
tile. Not yet tried across two physical machines.

## Design decisions (the old open questions)

- **Rendering:** everything is half-block pixels (`▀`, two freely coloured
  pixels per cell), no Braille: one foreground colour per cell would muddy
  small coloured tanks. The camera looks down from slightly south: a block's
  top face shifts up by `LIFT` and its south face shows, with a depth buffer
  so blocks hide what stands behind them. Same approach as Calvary's sprites.
- **Audio:** synthesised at run time (`src/audio.rs`), no sample files.
  One-shot effects are rendered at start-up; one endless rodio source (the
  band) plays the engines and the music, fed from the game each frame. The
  march (original, 132 bpm, C G Am F C G F-G C) has drums and an oom-pah bass
  always, plus one layer per enemy type alive: woodblock (Tinpot), tuba
  (Plodder), kazoo melody (Hornet), piccolo (Skitter), brass stabs (Rascal),
  glockenspiel (Sniper), running arpeggio (Viper), theremin (Ghost), timpani
  (Onyx). The title plays the full band. Losing a tank gets a sad trombone.
- **Arena and weapons:** 22x16 tiles inside a fixed border, one-hit kills,
  five shells and two mines for the player, no pickups (the original had
  none either).
- **v1 scope:** single-player campaign, then LAN co-op (above).

## References (checked 2026-10-04)

`asciiverse/references/Wii Tanks/`:

- `Wii Tank Battle.zip`: an SVN checkout of tank-battle-wii.googlecode.com
  (2008). It holds only the GRRLIB and libpng libraries and a Makefile;
  `trunk/source/` is empty. No game code or design to learn from.
- `tank.zip`: "Wii play tank minigame remake", a 2014 audio game in BGT by
  guilevi (Spanish readme): one more tank per level, 4-way grid movement,
  shells that travel 8 cells. Its WAVs (level music, fire, move, death) have
  no stated source or licence, so none of them are used.

Everything in Panzer is therefore designed from the Wii Play Tanks rules as
publicly known, with original names, maps, art and music.

## Code layout

- `src/arena.rs`: tile grid and the mission file parser (format at the top).
- `src/world.rs`: the simulation: driving, collisions, shells, mines,
  effects, stains; also the headless balance bot.
- `src/ai.rs`: the roster table and the enemy brains.
- `src/render.rs`: the pixel canvas, floor, blocks, sprites, title picture.
- `src/game.rs`: campaign flow, HUD, cards.
- `src/audio.rs`: the synthesiser.
- `src/input.rs`: keyboard, mouse, gamepad.
- `src/proto.rs`: wire messages; `src/lan.rs`, `src/net.rs`: discovery and
  iroh, from Tetrix.
- `tools/ansi2png.py`: turns a `tmux capture-pane -e -p` dump into a PNG.

## Verifying

```bash
cargo test --release                                     # parser, physics, audio sanity
cargo test --release balance -- --ignored --nocapture    # a bot plays every mission
cargo test --release levels -- --ignored --nocapture     # mix levels
```

The balance bot aims perfectly and dodges but plays no strategy; use its
wins, deaths and "killed by" columns to spot a mission that is broken or a
tank type that never fires, not as a difficulty score.

Screenshots: run the game in tmux and convert the capture. Use a private
tmux socket (`tmux -L panzer ...`): other sessions on this machine have
killed the default tmux server mid-run.

## Next

- Two physical machines on a real LAN (and Wi-Fi that may block broadcast).
- Versus mode on the same network code, in arenas built for two.
- Sextant (2x3) rendering for small terminals, picked with `drawprobe`.
- A mission editor, like Calvary's `--edit`.
- Tune the campaign against human play (the bot is only a smoke test).
