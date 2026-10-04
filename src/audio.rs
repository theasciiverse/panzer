//! All sound is synthesised here; Panzer ships no audio files.
//!
//! - Effects are rendered once at start-up into sample buffers.
//! - One endless source, the band, plays everything that changes with the
//!   battle: an engine voice per tank, each type with its own character (you
//!   should hear a Skitter twitching or an Onyx growling before you spot it),
//!   and an original march where every enemy type on the field adds its own
//!   instrument. Clear the field and the band thins out with it.
//!
//! Without an audio device, or with `--mute`, every call does nothing.

use rodio::buffer::SamplesBuffer;
use rodio::{MixerDeviceSink, Source};
use std::f32::consts::TAU;
use std::num::NonZero;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SR: u32 = 44_100;
const SRF: f32 = SR as f32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sfx {
    Shot,
    Rocket,
    Bounce,
    Fizzle,
    Cancel,
    Boom,
    MineSet,
    MineBeep,
    MineBoom,
    Crate,
    Click,
    Down,
    Start,
    Clear,
    Life,
    Select,
}

const ALL_SFX: [Sfx; 16] = [
    Sfx::Shot,
    Sfx::Rocket,
    Sfx::Bounce,
    Sfx::Fizzle,
    Sfx::Cancel,
    Sfx::Boom,
    Sfx::MineSet,
    Sfx::MineBeep,
    Sfx::MineBoom,
    Sfx::Crate,
    Sfx::Click,
    Sfx::Down,
    Sfx::Start,
    Sfx::Clear,
    Sfx::Life,
    Sfx::Select,
];

pub const SLOTS: usize = 16;

/// One tank's engine for the band: `kind` is the roster index + 1 (0: silent).
#[derive(Clone, Copy, Default, Debug)]
pub struct Engine {
    pub kind: u8,
    pub speed: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum Music {
    #[default]
    Off,
    /// Every instrument.
    Title,
    /// The drums and bass, plus one instrument per enemy type alive.
    Battle,
}

#[derive(Clone, Copy, Default)]
struct Live {
    engines: [Engine; SLOTS],
    music: Music,
    /// Bit k set: roster type k is on the field.
    layers: u16,
    silent: bool,
    music_off: bool,
}

pub struct Audio {
    // the sink must outlive everything playing on its mixer
    sink: Option<MixerDeviceSink>,
    bank: Vec<SamplesBuffer>,
    live: Arc<Mutex<Live>>,
    pub muted: bool,
    pub music_on: bool,
    last: [Option<Instant>; ALL_SFX.len()],
}

impl Audio {
    pub fn new(muted: bool) -> Audio {
        let sink = if muted {
            None
        } else {
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut s) => {
                    s.log_on_drop(false);
                    Some(s)
                }
                Err(e) => {
                    log::warn!("no audio: {e}");
                    None
                }
            }
        };
        let live = Arc::new(Mutex::new(Live::default()));
        let bank = if sink.is_some() { ALL_SFX.iter().map(|&s| buffer(render_sfx(s))).collect() } else { Vec::new() };
        if let Some(s) = &sink {
            s.mixer().add(Band::new(live.clone()));
        }
        Audio { sink, bank, live, muted, music_on: true, last: [None; ALL_SFX.len()] }
    }

    pub fn available(&self) -> bool {
        self.sink.is_some()
    }

    pub fn play(&mut self, sfx: Sfx, volume: f32) {
        if self.muted {
            return;
        }
        let Some(sink) = &self.sink else { return };
        let i = ALL_SFX.iter().position(|&s| s == sfx).unwrap_or(0);
        let now = Instant::now();
        if self.last[i].is_some_and(|t| now.duration_since(t) < Duration::from_millis(35)) {
            return;
        }
        self.last[i] = Some(now);
        sink.mixer().add(self.bank[i].clone().amplify(volume));
    }

    pub fn set_engines(&mut self, engines: &[Engine]) {
        if let Ok(mut l) = self.live.lock() {
            l.engines = [Engine::default(); SLOTS];
            for (slot, e) in l.engines.iter_mut().zip(engines) {
                *slot = *e;
            }
        }
    }

    pub fn set_music(&mut self, music: Music, layers: u16) {
        if let Ok(mut l) = self.live.lock() {
            l.music = music;
            l.layers = layers;
        }
    }

    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        if let Ok(mut l) = self.live.lock() {
            l.silent = self.muted;
        }
    }

    pub fn toggle_music(&mut self) {
        self.music_on = !self.music_on;
        if let Ok(mut l) = self.live.lock() {
            l.music_off = !self.music_on;
        }
    }
}

fn buffer(samples: Vec<f32>) -> SamplesBuffer {
    SamplesBuffer::new(NonZero::new(1).unwrap(), NonZero::new(SR).unwrap(), samples)
}

// ------------------------------------------------------------------ helpers

struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn midi(n: f32) -> f32 {
    440.0 * 2f32.powf((n - 69.0) / 12.0)
}

fn saw(ph: f32) -> f32 {
    2.0 * (ph - ph.floor()) - 1.0
}

fn square(ph: f32) -> f32 {
    if ph - ph.floor() < 0.5 {
        1.0
    } else {
        -1.0
    }
}

fn tri(ph: f32) -> f32 {
    let p = ph - ph.floor();
    4.0 * (p - 0.5).abs() - 1.0
}

/// `dur` seconds of samples from `f(t, noise)`.
fn synth(dur: f32, mut f: impl FnMut(f32, &mut Noise) -> f32) -> Vec<f32> {
    let mut n = Noise(0x1234_5678);
    (0..(dur * SRF) as usize).map(|i| f(i as f32 / SRF, &mut n)).collect()
}

/// A little tune: (midi note, seconds), played on `wave` with a soft attack.
fn tune(notes: &[(f32, f32)], gain: f32, wave: fn(f32) -> f32, vibrato: f32) -> Vec<f32> {
    let mut out = Vec::new();
    let mut ph = 0.0f32;
    let mut lp = 0.0f32;
    for &(n, d) in notes {
        let len = (d * SRF) as usize;
        for i in 0..len {
            let t = i as f32 / SRF;
            let vib = 1.0 + vibrato * (TAU * 5.5 * t).sin() * (t / d).min(1.0);
            if n > 0.0 {
                ph += midi(n) * vib / SRF;
            }
            let env = (t / 0.012).min(1.0) * ((d - t) / 0.03).clamp(0.0, 1.0);
            let x = if n > 0.0 { wave(ph) * env } else { 0.0 };
            lp += (x - lp) * 0.35;
            out.push(lp * gain);
        }
    }
    out
}

fn render_sfx(s: Sfx) -> Vec<f32> {
    match s {
        Sfx::Shot => {
            let (mut ph, mut lp) = (0.0f32, 0.0f32);
            synth(0.22, |t, n| {
                ph += (60.0 + 190.0 * (-t * 30.0).exp()) / SRF;
                lp += (n.next() - lp) * 0.3;
                (ph * TAU).sin() * (-t * 18.0).exp() * 0.75 + lp * (-t * 55.0).exp() * 0.6
            })
        }
        Sfx::Rocket => {
            let (mut ph, mut lp) = (0.0f32, 0.0f32);
            synth(0.45, |t, n| {
                ph += (380.0 + 900.0 * t) / SRF;
                lp += (n.next() - lp) * (0.04 + 0.4 * (-t * 6.0).exp());
                lp * (-t * 5.0).exp() * 0.9 + saw(ph) * 0.1 * (-t * 4.0).exp()
            })
        }
        Sfx::Bounce => synth(0.16, |t, n| {
            ((TAU * 1750.0 * t).sin() * (-t * 35.0).exp() + 0.6 * (TAU * 2630.0 * t).sin() * (-t * 50.0).exp()) * 0.3 + n.next() * (-t * 300.0).exp() * 0.2
        }),
        Sfx::Fizzle => {
            let mut lp = 0.0f32;
            synth(0.16, |t, n| {
                lp += (n.next() - lp) * 0.2;
                lp * (-t * 25.0).exp() * 0.4
            })
        }
        Sfx::Cancel => synth(0.1, |t, n| n.next() * (-t * 90.0).exp() * 0.5 + (TAU * 900.0 * t).sin() * (-t * 40.0).exp() * 0.3),
        Sfx::Boom | Sfx::MineBoom => {
            let big = s == Sfx::MineBoom;
            let (mut ph, mut lp, mut lp2) = (0.0f32, 0.0f32, 0.0f32);
            synth(if big { 1.6 } else { 1.1 }, |t, n| {
                let decay = if big { 2.4 } else { 3.2 };
                ph += ((if big { 30.0 } else { 38.0 }) + 60.0 * (-t * 6.0).exp()) / SRF;
                lp += (n.next() - lp) * (0.02 + 0.3 * (-t * 5.0).exp());
                lp2 += (lp - lp2) * 0.5;
                let crackle = if n.next() > 1.0 - 0.006 * (-t * 3.0).exp() { 0.6 } else { 0.0 };
                (lp2 * 1.6 * (-t * decay).exp() + (ph * TAU).sin() * 0.7 * (-t * 4.0).exp() + crackle).clamp(-1.0, 1.0) * 0.9
            })
        }
        Sfx::MineSet => synth(0.17, |t, _| {
            let f = if t < 0.075 { 520.0 } else { 780.0 };
            let lt = if t < 0.075 { t } else { t - 0.085 };
            if lt < 0.0 {
                0.0
            } else {
                tri(f * t) * (-lt * 30.0).exp() * 0.35
            }
        }),
        Sfx::MineBeep => synth(0.06, |t, _| square(1500.0 * t) * 0.16 * (1.0 - t / 0.06)),
        Sfx::Crate => {
            let (mut lp, mut lp2) = (0.0f32, 0.0f32);
            synth(0.3, |t, n| {
                lp += (n.next() - lp) * 0.25;
                lp2 += (lp - lp2) * 0.25;
                (lp - lp2) * 2.5 * (-t * 14.0).exp() + (TAU * 140.0 * t).sin() * (-t * 20.0).exp() * 0.5
            })
        }
        Sfx::Click => synth(0.03, |t, n| n.next() * (-t * 200.0).exp() * 0.25),
        // the sad trombone: four steps down, the last one wobbling
        Sfx::Down => tune(&[(55.0, 0.34), (54.0, 0.34), (53.0, 0.34), (52.0, 1.1)], 0.42, saw, 0.02),
        Sfx::Start => {
            let mut v = tune(&[(72.0, 0.1), (76.0, 0.1), (79.0, 0.1), (84.0, 0.5)], 0.22, square, 0.0);
            let roll = synth(0.8, |t, n| n.next() * 0.08 * (0.5 + 0.5 * (TAU * 24.0 * t).sin()) * (1.0 - t / 0.8));
            for (a, b) in v.iter_mut().zip(roll) {
                *a += b;
            }
            v
        }
        Sfx::Clear => tune(&[(67.0, 0.09), (72.0, 0.09), (76.0, 0.09), (79.0, 0.18), (76.0, 0.09), (79.0, 0.3), (84.0, 0.7)], 0.24, square, 0.004),
        Sfx::Life => tune(&[(72.0, 0.06), (76.0, 0.06), (79.0, 0.06), (84.0, 0.06), (88.0, 0.06), (91.0, 0.3)], 0.25, tri, 0.0),
        Sfx::Select => tune(&[(81.0, 0.05)], 0.2, tri, 0.0),
    }
}

// --------------------------------------------------------------------- band

#[derive(Clone, Copy, Default)]
struct EngineState {
    kind: u8,
    speed: f32,
    ph: f32,
    ph2: f32,
    pulse: f32,
    env: f32,
    lp: f32,
    f: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Wave {
    Sine,
    Tri,
    Saw,
    Square,
    Noise,
    Bell,
}

#[derive(Clone, Copy)]
struct Voice {
    freq: f32,
    ph: f32,
    wave: Wave,
    gain: f32,
    env: f32,
    attack: f32,
    rising: bool,
    hold: u32,
    decay: f32,
    release: f32,
    vibrato: f32,
    age: f32,
    /// Pitch fall per second, for drums.
    drop: f32,
    lp: f32,
}

impl Voice {
    fn new(freq: f32, wave: Wave, gain: f32, hold_s: f32, release_s: f32) -> Voice {
        Voice {
            freq,
            ph: 0.0,
            wave,
            gain,
            env: 0.0,
            attack: 1.0 / (0.005 * SRF),
            rising: true,
            hold: (hold_s * SRF) as u32,
            decay: 1.0,
            release: (-1.0 / (release_s * SRF)).exp(),
            vibrato: 0.0,
            age: 0.0,
            drop: 0.0,
            lp: 0.0,
        }
    }
    fn attack(mut self, s: f32) -> Voice {
        self.attack = 1.0 / (s * SRF);
        self
    }
    fn vibrato(mut self, depth: f32) -> Voice {
        self.vibrato = depth;
        self
    }
    fn drop(mut self, per_s: f32) -> Voice {
        self.drop = per_s;
        self
    }
}

const TEMPO: f32 = 132.0;
/// The tune, one entry per eighth note: a MIDI note, -2 holds the previous
/// one, -1 rests. Eight bars over C G Am F C G F-G C.
const MELODY: [i8; 64] = [
    67, -2, 72, -2, 76, 74, 72, -2, //
    71, -2, 74, -2, 79, 77, 74, -2, //
    72, -2, 76, 81, 79, 76, 72, -2, //
    69, 72, 77, -2, 76, -2, 74, -2, //
    67, 67, 72, -2, 76, -2, 79, -2, //
    77, 76, 74, -2, 71, -2, 67, -2, //
    69, -2, 72, 77, 74, -2, 71, 74, //
    72, -2, 67, -2, 72, -2, -1, -1,
];

/// Root (MIDI) and minor-ness of the chord at a sixteenth-note step.
fn chord(step: usize) -> (f32, bool) {
    let (bar, pos) = (step / 16 % 8, step % 16);
    match bar {
        0 | 4 | 7 => (48.0, false),
        1 | 5 => (43.0, false),
        2 => (45.0, true),
        3 => (41.0, false),
        _ => (if pos < 8 { 41.0 } else { 43.0 }, false),
    }
}

struct Band {
    live: Arc<Mutex<Live>>,
    buf: Vec<f32>,
    pos: usize,
    engines: [EngineState; SLOTS],
    voices: Vec<Voice>,
    step: usize,
    step_left: u32,
    noise: Noise,
    music_gain: f32,
}

impl Band {
    fn new(live: Arc<Mutex<Live>>) -> Band {
        Band {
            live,
            buf: vec![0.0; 512],
            pos: 512,
            engines: [EngineState::default(); SLOTS],
            voices: Vec::with_capacity(64),
            step: 0,
            step_left: 0,
            noise: Noise(0xdead_beef),
            music_gain: 0.0,
        }
    }

    fn render(&mut self) {
        let live = self.live.lock().map(|l| *l).unwrap_or_default();
        for (st, e) in self.engines.iter_mut().zip(live.engines.iter()) {
            if st.kind != e.kind {
                *st = EngineState { kind: e.kind, ..Default::default() };
            }
        }
        let step_len = (60.0 / TEMPO / 4.0 * SRF) as u32;
        let music_on = live.music != Music::Off && !live.music_off;
        let target_gain = if music_on { 1.0 } else { 0.0 };
        for k in 0..self.buf.len() {
            // the sequencer
            if self.step_left == 0 {
                self.step_left = step_len;
                if music_on {
                    let layers = if live.music == Music::Title { 0x3ff } else { live.layers | 1 };
                    self.trigger(layers);
                }
                self.step = (self.step + 1) % 128;
            }
            self.step_left -= 1;
            self.music_gain += (target_gain - self.music_gain) * 0.0002;
            let mut music = 0.0;
            for v in &mut self.voices {
                music += voice_sample(v, &mut self.noise);
            }
            let mut engines = 0.0;
            for (st, e) in self.engines.iter_mut().zip(live.engines.iter()) {
                if st.kind == 0 {
                    continue;
                }
                st.speed += (e.speed - st.speed) * 0.0006;
                engines += engine_sample(st, &mut self.noise);
            }
            let x = music * 0.55 * self.music_gain + engines * 0.6;
            self.buf[k] = if live.silent { 0.0 } else { (x * 1.1).tanh() * 0.8 };
        }
        self.voices.retain(|v| v.rising || v.env > 1e-4);
    }

    fn trigger(&mut self, layers: u16) {
        let st = self.step;
        let (bar, pos) = (st / 16, st % 16);
        let (root, minor) = chord(st);
        let third = if minor { 3.0 } else { 4.0 };
        let tones = [0.0, third, 7.0, 12.0];
        let on = |k: u16| layers & (1 << k) != 0;
        let eighth = 60.0 / TEMPO / 2.0;
        let mut add = |v: Voice| {
            if self.voices.len() < 60 {
                self.voices.push(v);
            }
        };
        // base: drums and the oom-pah bass
        if on(0) {
            if pos == 0 || pos == 8 {
                add(Voice::new(120.0, Wave::Sine, 0.55, 0.0, 0.14).drop(500.0));
            }
            if pos == 4 || pos == 12 || (bar == 7 && pos >= 12) {
                let g = if pos == 4 || pos == 12 { 0.22 } else { 0.12 };
                add(Voice::new(0.0, Wave::Noise, g, 0.0, 0.09));
            }
            if pos == 0 {
                add(Voice::new(midi(root - 12.0), Wave::Square, 0.11, 0.18, 0.06));
            }
            if pos == 8 {
                add(Voice::new(midi(root - 5.0), Wave::Square, 0.1, 0.18, 0.06));
            }
        }
        // 1 Tinpot: woodblock on the off-beats
        if on(1) && pos % 4 == 2 {
            add(Voice::new(if pos % 8 == 2 { 1600.0 } else { 1250.0 }, Wave::Sine, 0.14, 0.0, 0.025));
        }
        // 2 Plodder: tuba answering the bass
        if on(2) && (pos == 4 || pos == 12) {
            let n = if pos == 4 { root + third - 12.0 } else { root + 7.0 - 12.0 };
            add(Voice::new(midi(n), Wave::Tri, 0.22, 0.2, 0.08).attack(0.02));
        }
        // 3 Hornet: the kazoo has the tune
        // 6 Sniper: glockenspiel doubles it on the beats, an octave up
        if pos % 2 == 0 {
            let i = bar * 8 + pos / 2;
            let n = MELODY[i];
            if n >= 0 {
                let mut held = 1;
                while i + held < 64 && MELODY[i + held] == -2 {
                    held += 1;
                }
                if on(3) {
                    add(Voice::new(midi(n as f32), Wave::Saw, 0.075, eighth * held as f32 - 0.04, 0.05).attack(0.015).vibrato(0.008));
                }
                if on(6) && pos % 4 == 0 {
                    add(Voice::new(midi(n as f32 + 12.0), Wave::Bell, 0.09, 0.0, 0.5));
                }
            }
        }
        // 4 Skitter: piccolo twitters on beats two and four
        if on(4) && ((4..8).contains(&pos) || (12..16).contains(&pos)) {
            add(Voice::new(midi(root + 24.0 + tones[pos % 4]), Wave::Tri, 0.06, 0.0, 0.06));
        }
        // 5 Rascal: brass stabs on the and of two and four
        if on(5) && (pos == 6 || pos == 14) {
            for t in &tones[..3] {
                add(Voice::new(midi(root + 12.0 + t), Wave::Square, 0.035, 0.07, 0.04));
            }
        }
        // 7 Viper: a running sixteenth arpeggio
        if on(7) {
            let arp = [0.0, third, 7.0, 12.0, 7.0, third];
            add(Voice::new(midi(root + 12.0 + arp[st % 6]), Wave::Saw, 0.04, 0.0, 0.06));
        }
        // 8 Ghost: a theremin hanging over every other bar
        if on(8) && pos == 0 && bar % 2 == 0 {
            add(Voice::new(midi(root + 24.0 + third), Wave::Sine, 0.07, eighth * 14.0, 0.4).attack(0.35).vibrato(0.018));
        }
        // 9 Onyx: timpani, rolling into the turnaround
        if on(9) && (pos == 0 || pos == 8 || (bar == 7 && pos >= 8)) {
            let g = if bar == 7 && pos >= 8 && pos != 8 { 0.2 } else { 0.38 };
            add(Voice::new(midi(root - 12.0), Wave::Sine, g, 0.0, 0.35).drop(30.0));
        }
    }
}

fn voice_sample(v: &mut Voice, noise: &mut Noise) -> f32 {
    v.age += 1.0 / SRF;
    if v.rising {
        v.env += v.attack;
        if v.env >= 1.0 {
            v.env = 1.0;
            v.rising = false;
        }
    } else if v.hold > 0 {
        v.hold -= 1;
        v.env *= v.decay;
    } else {
        v.env *= v.release;
    }
    if v.drop > 0.0 {
        v.freq = (v.freq - v.drop / SRF).max(30.0);
    }
    let vib = 1.0 + v.vibrato * (TAU * 5.5 * v.age).sin();
    v.ph += v.freq * vib / SRF;
    let x = match v.wave {
        Wave::Sine => (v.ph * TAU).sin(),
        Wave::Tri => tri(v.ph),
        Wave::Saw => saw(v.ph),
        Wave::Square => square(v.ph),
        Wave::Noise => noise.next(),
        Wave::Bell => (v.ph * TAU).sin() + 0.35 * (v.ph * TAU * 2.76).sin() * (-v.age * 8.0).exp(),
    };
    // soften the bright waves
    v.lp += (x - v.lp) * if matches!(v.wave, Wave::Saw | Wave::Square) { 0.25 } else { 1.0 };
    v.lp * v.env * v.gain
}

/// A putt-putt engine: pulses at `rate` Hz, each a short burst of a low tone
/// and some grit.
fn putter(e: &mut EngineState, rate: f32, freq: f32, decay_s: f32, grit: f32, noise: &mut Noise) -> f32 {
    e.pulse += rate / SRF;
    if e.pulse >= 1.0 {
        e.pulse -= 1.0;
        e.env = 1.0;
    }
    e.env *= 1.0 - 1.0 / (decay_s * SRF);
    e.ph += freq / SRF;
    ((e.ph * TAU).sin() * (1.0 - grit) + noise.next() * grit) * e.env
}

/// One sample of a tank engine, by roster kind + 1.
fn engine_sample(e: &mut EngineState, noise: &mut Noise) -> f32 {
    let s = e.speed;
    match e.kind {
        // you: a medium diesel chug that speeds up as you roll
        1 => putter(e, 9.0 + 16.0 * s, 64.0 + 30.0 * s, 0.014, 0.3, noise) * (0.12 + 0.88 * s) * 0.55,
        // Tinpot: a sleepy idle
        2 => putter(e, 3.0, 55.0, 0.03, 0.3, noise) * 0.3,
        // Plodder: slow heavy thumps
        3 => putter(e, 4.5 + 5.0 * s, 46.0, 0.02, 0.25, noise) * (0.3 + 0.7 * s) * 0.6,
        // Hornet: a buzzing whine
        4 => {
            e.ph2 += 7.0 / SRF;
            e.ph += (205.0 + 70.0 * s + 9.0 * (e.ph2 * TAU).sin()) / SRF;
            e.lp += (saw(e.ph) - e.lp) * 0.12;
            e.lp * 0.2 * (0.35 + 0.65 * s)
        }
        // Skitter: nervous, never the same pitch twice
        5 => {
            e.pulse += (15.0 + 9.0 * s) / SRF;
            if e.pulse >= 1.0 {
                e.pulse -= 1.0 + noise.next() * 0.4;
                e.env = 1.0;
                e.f = 150.0 + 70.0 * noise.next().abs();
            }
            e.env *= 1.0 - 1.0 / (0.008 * SRF);
            e.ph += e.f / SRF;
            (tri(e.ph) * 0.7 + noise.next() * 0.3) * e.env * (0.3 + 0.7 * s) * 0.45
        }
        // Rascal: revs up when it charges
        6 => {
            e.f += (70.0 + 120.0 * s - e.f) * 0.0004;
            e.ph2 += e.f / SRF;
            e.lp += (saw(e.ph2) - e.lp) * 0.15;
            e.lp * 0.18 * (0.3 + 0.7 * s) + putter(e, 10.0 + 14.0 * s, 70.0, 0.01, 0.4, noise) * 0.3 * s
        }
        // Sniper: a patient clock tick
        7 => putter(e, 2.0, 1100.0, 0.005, 0.2, noise) * 0.5,
        // Viper: an angry growling rev
        8 => {
            e.f += (110.0 + 170.0 * s - e.f) * 0.0006;
            e.ph += e.f / SRF;
            e.ph2 += 28.0 / SRF;
            e.lp += (square(e.ph) - e.lp) * 0.2;
            e.lp * (0.6 + 0.4 * (e.ph2 * TAU).sin()) * 0.11 * (0.35 + 0.65 * s)
        }
        // Ghost: the hum you hear before you see anything
        9 => {
            e.ph2 += 5.5 / SRF;
            e.ph += (330.0 + 60.0 * s + 22.0 * (e.ph2 * TAU).sin()) / SRF;
            (e.ph * TAU).sin() * 0.075
        }
        // Onyx: a deep beating growl with rumble underneath
        10 => {
            e.ph += (42.0 + 20.0 * s) / SRF;
            e.ph2 += (43.4 + 20.0 * s) / SRF;
            e.lp += ((square(e.ph) + square(e.ph2)) * 0.5 + noise.next() * 0.3 - e.lp) * 0.08;
            e.lp * 0.26 * (0.4 + 0.6 * s)
        }
        _ => 0.0,
    }
}

impl Iterator for Band {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.buf.len() {
            self.render();
            self.pos = 0;
        }
        self.pos += 1;
        Some(self.buf[self.pos - 1])
    }
}

impl Source for Band {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> rodio::ChannelCount {
        NonZero::new(1).unwrap()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        NonZero::new(SR).unwrap()
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_render_and_stay_in_range() {
        for s in ALL_SFX {
            let v = render_sfx(s);
            assert!(!v.is_empty(), "{s:?} is empty");
            let peak = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.01 && peak <= 1.5, "{s:?} peak {peak}");
        }
    }

    #[test]
    fn band_plays_every_layer_and_engine() {
        let live = Arc::new(Mutex::new(Live::default()));
        {
            let mut l = live.lock().unwrap();
            l.music = Music::Title;
            for k in 0..10 {
                l.engines[k] = Engine { kind: k as u8 + 1, speed: 0.7 };
            }
        }
        let mut band = Band::new(live);
        let mut peak = 0.0f32;
        for _ in 0..(SR as usize * 8) {
            let x = band.next().unwrap();
            assert!(x.is_finite());
            peak = peak.max(x.abs());
        }
        assert!(peak > 0.05 && peak <= 1.0, "peak {peak}");
    }
}

#[cfg(test)]
mod levels {
    use super::*;

    fn rms(live: Live, secs: f32) -> f32 {
        let mut band = Band::new(Arc::new(Mutex::new(live)));
        let n = (SRF * secs) as usize;
        // let the music fade in first
        for _ in 0..(SRF * 3.0) as usize {
            band.next();
        }
        (band.by_ref().take(n).map(|x| x * x).sum::<f32>() / n as f32).sqrt()
    }

    /// `cargo test --release levels -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn print_levels() {
        let mut l = Live { music: Music::Battle, layers: 1, ..Default::default() };
        println!("music base only     {:.3}", rms(l, 4.0));
        l.layers = 0x3ff;
        println!("music full band     {:.3}", rms(l, 4.0));
        l.music = Music::Off;
        for k in 1..=10u8 {
            let mut e = l;
            e.engines[0] = Engine { kind: k, speed: 1.0 };
            println!("engine {k:>2} moving    {:.3}", rms(e, 1.0));
        }
        for s in ALL_SFX {
            let v = render_sfx(s);
            let r = (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
            println!("{s:?} rms {r:.3} peak {:.3}", v.iter().fold(0.0f32, |m, x| m.max(x.abs())));
        }
    }
}
