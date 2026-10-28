# Panzer

**TOY TANKS. REAL RICOCHETS.**

Twenty missions, one little tank, and a toy box full of enemies who want you
scorched off the map. Every shell bounces off the walls, theirs too: bank
your shots round corners, shoot their shells out of the air, and drop mines
behind the crates. Nine enemy tanks each have their own trick, their own
engine voice and their own instrument in the battle march, from the Tinpot
that can't move to the Ghost you can only hear. Bring a friend on the local
network for co-op, and mind the friendly fire.

## Play

With [av](https://asciiverse.sh), the Asciiverse client:

```
av install panzer
av play panzer
```

Or download the archive for your system from this repository's Releases,
unpack it and run `./panzer`.

To build it yourself: `cargo run --release`.

Needs a truecolour terminal: 160x50 is playable, 200x60 or more is
comfortable.

## Keys

| Key | |
|---|---|
| W A S D, arrows | drive |
| Mouse (or I J K L, Q / E) | aim |
| Left click, Space | fire (five shells at most) |
| Right click, F | drop a mine (two at most) |
| P, Esc | pause |
| H | help |
| M / F8 | music / all sound on or off |

Gamepad: left stick drives, right stick aims, right trigger fires, left
trigger or A drops a mine.

## Options

`panzer --help` lists them: `--mission N`, `--host`, `--join`, `--mute`.

Session log for bug reports: `RUST_LOG=debug panzer 2>panzer.log`

## Credits

After Wii Play Tanks (Nintendo, 2006): original names, maps, art and music,
no code or art from it. Music and sound are synthesised in the game.
Networking uses [iroh](https://iroh.computer).

## Licence

MIT; see [LICENSE](LICENSE).
