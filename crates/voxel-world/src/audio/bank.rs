//! Sound catalogue and the procedurally generated sample bank.
//!
//! Every sound is synthesised from filtered noise, granular crunches and
//! damped modal resonances; no recorded or third-party audio is used.
//! Generation is deterministic, so a given build always sounds the same.
use super::synth::{Buf, Color, Filter, Mode, Rng};
use anyhow::{Context, Result};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

/// Acoustic material of a block.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Material {
    Grass,
    Dirt,
    Gravel,
    Stone,
    Wood,
    Sand,
    Plant,
    Glass,
    Cloth,
    Snow,
}
impl Material {
    pub const ALL: [Material; 10] = [
        Material::Grass,
        Material::Dirt,
        Material::Gravel,
        Material::Stone,
        Material::Wood,
        Material::Sand,
        Material::Plant,
        Material::Glass,
        Material::Cloth,
        Material::Snow,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Material::Grass => "grass",
            Material::Dirt => "dirt",
            Material::Gravel => "gravel",
            Material::Stone => "stone",
            Material::Wood => "wood",
            Material::Sand => "sand",
            Material::Plant => "plant",
            Material::Glass => "glass",
            Material::Cloth => "cloth",
            Material::Snow => "snow",
        }
    }
    fn index(self) -> usize {
        Self::ALL.iter().position(|m| *m == self).unwrap()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Break,
    Place,
    Step,
    /// One tick of mining progress.
    Hit,
}
const KINDS: [Kind; 4] = [Kind::Break, Kind::Place, Kind::Step, Kind::Hit];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sound {
    Block(Kind, Material),
    UiMove,
    UiClick,
    UiBack,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Category {
    Effects = 0,
    Ui = 1,
}

/// Playback parameters of a sound.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub volume: f32,
    /// Relative random pitch deviation, e.g. 0.08 = ±8 %.
    pub pitch_jitter: f32,
    /// Higher priorities steal voices from lower ones.
    pub priority: u8,
    pub category: Category,
    pub variants: usize,
    /// Audible distance in blocks for positional playback.
    pub range: f32,
}

const UI_SOUNDS: [Sound; 3] = [Sound::UiMove, Sound::UiClick, Sound::UiBack];
pub const COUNT: usize = Material::ALL.len() * KINDS.len() + UI_SOUNDS.len();

impl Sound {
    pub fn all() -> Vec<Sound> {
        let mut all = Vec::with_capacity(COUNT);
        for m in Material::ALL {
            for k in KINDS {
                all.push(Sound::Block(k, m));
            }
        }
        all.extend(UI_SOUNDS);
        all
    }
    pub fn index(self) -> usize {
        match self {
            Sound::Block(k, m) => {
                m.index() * KINDS.len() + KINDS.iter().position(|x| *x == k).unwrap()
            }
            Sound::UiMove => Material::ALL.len() * KINDS.len(),
            Sound::UiClick => Material::ALL.len() * KINDS.len() + 1,
            Sound::UiBack => Material::ALL.len() * KINDS.len() + 2,
        }
    }
    pub fn name(self) -> String {
        match self {
            Sound::Block(k, m) => format!(
                "{}-{}",
                match k {
                    Kind::Break => "break",
                    Kind::Place => "place",
                    Kind::Step => "step",
                    Kind::Hit => "hit",
                },
                m.key()
            ),
            Sound::UiMove => "ui-move".into(),
            Sound::UiClick => "ui-click".into(),
            Sound::UiBack => "ui-back".into(),
        }
    }
    pub fn spec(self) -> Spec {
        let block = |volume, pitch_jitter, priority, variants| Spec {
            volume,
            pitch_jitter,
            priority,
            category: Category::Effects,
            variants,
            range: 16.0,
        };
        let ui = |volume| Spec {
            volume,
            pitch_jitter: 0.02,
            priority: 3,
            category: Category::Ui,
            variants: 2,
            range: 1.0,
        };
        match self {
            Sound::Block(Kind::Break, _) => block(0.9, 0.08, 2, 4),
            Sound::Block(Kind::Place, _) => block(0.8, 0.07, 2, 4),
            Sound::Block(Kind::Step, _) => block(0.3, 0.1, 0, 6),
            Sound::Block(Kind::Hit, _) => block(0.42, 0.06, 1, 4),
            Sound::UiMove => ui(0.3),
            Sound::UiClick => ui(0.5),
            Sound::UiBack => ui(0.45),
        }
    }
}

pub struct Bank {
    pub rate: u32,
    sounds: Vec<Vec<Vec<f32>>>,
}

impl Bank {
    pub fn generate(rate: u32) -> Self {
        let sounds = Sound::all()
            .into_iter()
            .map(|sound| {
                (0..sound.spec().variants)
                    .map(|variant| {
                        let seed = 0x5eed_0000 ^ ((sound.index() as u64) << 8) ^ variant as u64;
                        synth(sound, rate as f32, &mut Rng::new(seed))
                    })
                    .collect()
            })
            .collect();
        Self { rate, sounds }
    }
    pub fn get(&self, index: usize, variant: usize) -> &[f32] {
        self.sounds
            .get(index)
            .and_then(|v| v.get(variant))
            .map_or(&[], Vec::as_slice)
    }
    pub fn sample_count(&self) -> usize {
        self.sounds.iter().flatten().map(Vec::len).sum()
    }
    pub fn clip_count(&self) -> usize {
        self.sounds.iter().map(Vec::len).sum()
    }
    /// Writes every variant as 16-bit mono WAV for listening on a PC.
    pub fn write_wavs(&self, dir: &Path) -> Result<usize> {
        std::fs::create_dir_all(dir)?;
        let mut written = 0;
        for sound in Sound::all() {
            for (variant, data) in self.sounds[sound.index()].iter().enumerate() {
                let path = dir.join(format!("{}-{}.wav", sound.name(), variant + 1));
                write_wav(&path, self.rate, data)
                    .with_context(|| format!("Writing {}", path.display()))?;
                written += 1;
            }
        }
        Ok(written)
    }
}

fn write_wav(path: &Path, rate: u32, data: &[f32]) -> Result<()> {
    let mut w = BufWriter::new(File::create(path)?);
    let bytes = data.len() as u32 * 2;
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + bytes).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?;
    w.write_all(&rate.to_le_bytes())?;
    w.write_all(&(rate * 2).to_le_bytes())?;
    w.write_all(&2u16.to_le_bytes())?;
    w.write_all(&16u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&bytes.to_le_bytes())?;
    for x in data {
        w.write_all(&((x.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())?;
    }
    w.flush()?;
    Ok(())
}

/// Loudness targets (max 30 ms RMS) before the per-sound volume.
const BLOCK_LOUDNESS: f32 = 0.2;
const UI_LOUDNESS: f32 = 0.12;

fn synth(sound: Sound, rate: f32, rng: &mut Rng) -> Vec<f32> {
    match sound {
        Sound::Block(kind, material) => {
            let buf = match material {
                Material::Grass => rustle(rate, rng, kind, 1.0, 1.0),
                Material::Plant => rustle(rate, rng, kind, 1.35, 0.35),
                Material::Dirt => crumble(rate, rng, kind, &DIRT),
                Material::Gravel => crumble(rate, rng, kind, &GRAVEL),
                Material::Snow => crumble(rate, rng, kind, &SNOW),
                Material::Sand => sand(rate, rng, kind),
                Material::Cloth => cloth(rate, rng, kind),
                Material::Stone => stone(rate, rng, kind),
                Material::Wood => wood(rate, rng, kind),
                Material::Glass => glass(rate, rng, kind),
            };
            buf.finish(BLOCK_LOUDNESS)
        }
        Sound::UiMove => ui(rate, rng, &[(1850.0, 0.0035, 1.0), (3900.0, 0.002, 0.4)], 1),
        Sound::UiClick => ui(rate, rng, &[(1150.0, 0.009, 1.0), (2650.0, 0.006, 0.55)], 2),
        Sound::UiBack => ui(rate, rng, &[(760.0, 0.011, 1.0), (1720.0, 0.007, 0.5)], 2),
    }
}

/// Duration, event count scale and time spread for each kind of sound.
fn shape(kind: Kind) -> (f32, f32, f32) {
    match kind {
        Kind::Break => (0.36, 1.0, 0.2),
        Kind::Place => (0.22, 0.6, 0.11),
        Kind::Step => (0.16, 0.3, 0.07),
        Kind::Hit => (0.12, 0.25, 0.05),
    }
}

fn scaled(rng: &mut Rng, base: std::ops::RangeInclusive<usize>, scale: f32) -> usize {
    let n = rng.count(base) as f32 * scale;
    n.round().max(1.0) as usize
}

/// Early-weighted onset time inside `spread` seconds.
fn onset(rng: &mut Rng, spread: f32) -> f32 {
    spread * rng.f32().powf(1.7)
}

/// Grass and leaves: a pink-noise rustle with crunchy high grains.
fn rustle(rate: f32, rng: &mut Rng, kind: Kind, bright: f32, thump: f32) -> Buf {
    let (dur, scale, spread) = shape(kind);
    let mut b = Buf::new(rate, dur);
    b.noise(
        rng,
        0.0,
        0.004,
        spread * 0.45,
        dur,
        Color::Pink,
        &[Filter::Hp(700.0 * bright, 0.7), Filter::Lp(7000.0, 0.7)],
        0.35,
    );
    for _ in 0..scaled(rng, 12..=16, scale) {
        let t = onset(rng, spread);
        let length = rng.range(0.006, 0.022);
        let center = rng.range(1600.0, 4800.0) * bright;
        let q = rng.range(1.2, 2.8);
        let amp = rng.range(0.4, 1.0) * (1.0 - t / (spread * 1.6)).max(0.2);
        b.grain(rng, t, length, center, q, amp);
    }
    b.noise(
        rng,
        0.0,
        0.002,
        0.022,
        0.08,
        Color::Brown,
        &[Filter::Lp(260.0, 0.7)],
        0.5 * thump,
    );
    b
}

/// Parameters for crumbly materials.
struct Crumble {
    bed_lp: f32,
    bed_hp: f32,
    grains: std::ops::RangeInclusive<usize>,
    center: (f32, f32),
    length: (f32, f32),
    q: (f32, f32),
    thump: f32,
    attack: f32,
}
const DIRT: Crumble = Crumble {
    bed_lp: 1400.0,
    bed_hp: 150.0,
    grains: 14..=20,
    center: (500.0, 2000.0),
    length: (0.004, 0.014),
    q: (1.0, 2.0),
    thump: 0.7,
    attack: 0.003,
};
const GRAVEL: Crumble = Crumble {
    bed_lp: 3000.0,
    bed_hp: 400.0,
    grains: 22..=30,
    center: (900.0, 3200.0),
    length: (0.003, 0.010),
    q: (1.2, 2.4),
    thump: 0.45,
    attack: 0.002,
};
const SNOW: Crumble = Crumble {
    bed_lp: 2500.0,
    bed_hp: 250.0,
    grains: 10..=14,
    center: (700.0, 2400.0),
    length: (0.008, 0.02),
    q: (2.0, 4.0),
    thump: 0.3,
    attack: 0.008,
};

fn crumble(rate: f32, rng: &mut Rng, kind: Kind, p: &Crumble) -> Buf {
    let (dur, scale, spread) = shape(kind);
    let mut b = Buf::new(rate, dur);
    b.noise(
        rng,
        0.0,
        p.attack,
        spread * 0.4,
        dur,
        Color::Brown,
        &[Filter::Lp(p.bed_lp, 0.7), Filter::Hp(p.bed_hp, 0.7)],
        0.4,
    );
    for _ in 0..scaled(rng, p.grains.clone(), scale) {
        let t = onset(rng, spread);
        let length = rng.range(p.length.0, p.length.1);
        let center = rng.range(p.center.0, p.center.1);
        let q = rng.range(p.q.0, p.q.1);
        let amp = rng.range(0.35, 1.0) * (1.0 - t / (spread * 1.6)).max(0.2);
        b.grain(rng, t, length, center, q, amp);
    }
    b.noise(
        rng,
        0.0,
        0.002,
        0.03,
        0.1,
        Color::Brown,
        &[Filter::Lp(200.0, 0.7)],
        p.thump,
    );
    b
}

/// Sand: a soft hiss made of many tiny grains.
fn sand(rate: f32, rng: &mut Rng, kind: Kind) -> Buf {
    let (dur, scale, spread) = shape(kind);
    let mut b = Buf::new(rate, dur + 0.04);
    let attack = if kind == Kind::Step { 0.006 } else { 0.01 };
    b.noise(
        rng,
        0.0,
        attack,
        spread * 0.55,
        dur + 0.04,
        Color::White,
        &[Filter::Hp(350.0, 0.7), Filter::Lp(3800.0, 0.7)],
        0.5,
    );
    for _ in 0..scaled(rng, 25..=45, scale) {
        let t = onset(rng, spread * 1.2);
        let length = rng.range(0.0015, 0.004);
        let center = rng.range(2000.0, 6000.0);
        let (q, amp) = (rng.range(0.8, 1.5), rng.range(0.15, 0.4));
        b.grain(rng, t, length, center, q, amp);
    }
    b.noise(
        rng,
        0.0,
        0.002,
        0.02,
        0.08,
        Color::Brown,
        &[Filter::Lp(180.0, 0.7)],
        0.25,
    );
    b
}

/// Wool and other fabrics: a muffled thump.
fn cloth(rate: f32, rng: &mut Rng, kind: Kind) -> Buf {
    let (dur, scale, spread) = shape(kind);
    let mut b = Buf::new(rate, dur);
    b.noise(
        rng,
        0.0,
        0.006,
        spread * 0.4,
        dur,
        Color::Pink,
        &[Filter::Lp(900.0, 0.7), Filter::Hp(120.0, 0.7)],
        0.5,
    );
    for _ in 0..scaled(rng, 4..=6, scale) {
        let t = onset(rng, spread);
        let (length, center) = (rng.range(0.01, 0.025), rng.range(500.0, 1200.0));
        b.grain(rng, t, length, center, 1.0, 0.3);
    }
    b.noise(
        rng,
        0.0,
        0.003,
        0.03,
        0.1,
        Color::Brown,
        &[Filter::Lp(220.0, 0.7)],
        0.5,
    );
    b
}

/// A hard, dense impact: short inharmonic ring, grit and a low thud.
fn stone_strike(b: &mut Buf, rng: &mut Rng, t: f32, amp: f32, tight: f32) {
    let base = rng.range(950.0, 1400.0);
    let detune = |rng: &mut Rng, r: f32| r * rng.range(0.97, 1.03);
    let modes: [Mode; 5] = [
        (base, 0.022 * tight, 1.0),
        (base * detune(rng, 1.47), 0.016 * tight, 0.7),
        (base * detune(rng, 2.09), 0.012 * tight, 0.5),
        (base * detune(rng, 2.76), 0.009 * tight, 0.35),
        (rng.range(260.0, 380.0), 0.018 * tight, 0.6),
    ];
    b.modes(rng, t, &modes, amp * 0.5);
    b.noise(
        rng,
        t,
        0.0005,
        0.006,
        0.05,
        Color::White,
        &[Filter::Hp(1500.0, 0.7), Filter::Lp(9000.0, 0.7)],
        amp * 0.8,
    );
    b.noise(
        rng,
        t,
        0.0005,
        0.015,
        0.06,
        Color::Brown,
        &[Filter::Lp(400.0, 0.7)],
        amp * 0.4,
    );
}

fn debris(b: &mut Buf, rng: &mut Rng, n: usize, from: f32, to: f32, center: (f32, f32)) {
    for _ in 0..n {
        let t = from + (to - from) * rng.f32().powf(1.4);
        let length = rng.range(0.002, 0.006);
        let c = rng.range(center.0, center.1);
        let q = rng.range(1.5, 3.0);
        let amp = rng.range(0.2, 0.5) * (1.0 - (t - from) / (to - from + 0.1));
        b.grain(rng, t, length, c, q, amp);
    }
}

fn stone(rate: f32, rng: &mut Rng, kind: Kind) -> Buf {
    let (dur, _, _) = shape(kind);
    let mut b = Buf::new(rate, dur);
    match kind {
        Kind::Break => {
            stone_strike(&mut b, rng, 0.0, 1.0, 1.0);
            let second = rng.range(0.03, 0.05);
            stone_strike(&mut b, rng, second, 0.7, 0.9);
            let third = rng.range(0.08, 0.12);
            stone_strike(&mut b, rng, third, 0.45, 0.8);
            let n = rng.count(8..=12);
            debris(&mut b, rng, n, 0.06, 0.3, (2500.0, 7000.0));
        }
        Kind::Place => {
            stone_strike(&mut b, rng, 0.0, 1.0, 1.0);
            debris(&mut b, rng, 3, 0.02, 0.12, (2500.0, 6000.0));
        }
        Kind::Step => {
            stone_strike(&mut b, rng, 0.0, 0.6, 0.7);
            // The scuff of a sole on rock.
            b.noise(
                rng,
                0.004,
                0.004,
                0.02,
                0.09,
                Color::White,
                &[Filter::Hp(2000.0, 0.7), Filter::Lp(7000.0, 0.7)],
                0.2,
            );
        }
        Kind::Hit => {
            stone_strike(&mut b, rng, 0.0, 0.9, 0.85);
            debris(&mut b, rng, 2, 0.01, 0.08, (3000.0, 6500.0));
        }
    }
    b
}

/// A hollow knock: low modes with longer decays than stone.
fn wood_strike(b: &mut Buf, rng: &mut Rng, t: f32, amp: f32, f0: f32, tight: f32) {
    let detune = |rng: &mut Rng, r: f32| r * rng.range(0.96, 1.04);
    let modes: [Mode; 5] = [
        (f0, 0.055 * tight, 1.0),
        (f0 * detune(rng, 2.32), 0.038 * tight, 0.55),
        (f0 * detune(rng, 3.86), 0.024 * tight, 0.35),
        (f0 * detune(rng, 5.4), 0.016 * tight, 0.2),
        (rng.range(1100.0, 1500.0), 0.01 * tight, 0.25),
    ];
    b.modes(rng, t, &modes, amp * 0.55);
    b.noise(
        rng,
        t,
        0.0003,
        0.004,
        0.03,
        Color::White,
        &[Filter::Bp(1800.0, 0.8)],
        amp * 0.6,
    );
}

fn wood(rate: f32, rng: &mut Rng, kind: Kind) -> Buf {
    let (dur, _, _) = shape(kind);
    let mut b = Buf::new(rate, dur + 0.05);
    let f0 = rng.range(170.0, 260.0);
    match kind {
        Kind::Break => {
            wood_strike(&mut b, rng, 0.0, 1.0, f0, 1.0);
            let second = rng.range(0.05, 0.08);
            wood_strike(&mut b, rng, second, 0.7, f0 * 1.1, 0.9);
            // Splintering crack.
            b.noise(
                rng,
                0.02,
                0.002,
                0.025,
                0.12,
                Color::White,
                &[Filter::Hp(2200.0, 0.7)],
                0.35,
            );
            let n = rng.count(6..=9);
            debris(&mut b, rng, n, 0.03, 0.25, (1500.0, 4500.0));
        }
        Kind::Place => {
            wood_strike(&mut b, rng, 0.0, 1.0, f0, 1.0);
            b.noise(
                rng,
                0.0,
                0.001,
                0.02,
                0.08,
                Color::Brown,
                &[Filter::Lp(300.0, 0.7)],
                0.5,
            );
        }
        Kind::Step => wood_strike(&mut b, rng, 0.0, 0.55, f0 * 1.25, 0.6),
        Kind::Hit => wood_strike(&mut b, rng, 0.0, 0.9, f0 * 1.05, 0.85),
    }
    b
}

fn tink(b: &mut Buf, rng: &mut Rng, t: f32, amp: f32, tight: f32) {
    let f = rng.range(2800.0, 3600.0);
    let modes: [Mode; 3] = [
        (f, 0.05 * tight, 1.0),
        (f * rng.range(1.85, 1.95), 0.03 * tight, 0.6),
        (f * rng.range(3.0, 3.2), 0.02 * tight, 0.35),
    ];
    b.modes(rng, t, &modes, amp * 0.5);
    b.noise(
        rng,
        t,
        0.0003,
        0.003,
        0.02,
        Color::White,
        &[Filter::Hp(3000.0, 0.7)],
        amp * 0.5,
    );
}

fn glass(rate: f32, rng: &mut Rng, kind: Kind) -> Buf {
    match kind {
        Kind::Break => {
            let mut b = Buf::new(rate, 0.55);
            for i in 0..rng.count(14..=22) {
                let t = 0.18 * rng.f32().powf(1.5);
                let f = rng.range(2500.0, 7500.0);
                let amp = rng.range(0.3, 1.0) * if i == 0 { 1.5 } else { 1.0 };
                let modes: [Mode; 2] = [
                    (f, rng.range(0.03, 0.12), 1.0),
                    (f * rng.range(1.4, 2.2), rng.range(0.02, 0.06), 0.5),
                ];
                b.modes(rng, t, &modes, amp * 0.3);
            }
            b.noise(
                rng,
                0.0,
                0.001,
                0.05,
                0.3,
                Color::White,
                &[Filter::Hp(3500.0, 0.7)],
                0.5,
            );
            b
        }
        Kind::Place => {
            let mut b = Buf::new(rate, 0.25);
            tink(&mut b, rng, 0.0, 1.0, 1.0);
            b
        }
        Kind::Step => {
            let mut b = Buf::new(rate, 0.14);
            tink(&mut b, rng, 0.0, 0.6, 0.5);
            b
        }
        Kind::Hit => {
            let mut b = Buf::new(rate, 0.16);
            tink(&mut b, rng, 0.0, 0.8, 0.7);
            b
        }
    }
}

/// Soft, very short interface clicks; never a melodic beep.
fn ui(rate: f32, rng: &mut Rng, modes: &[Mode], clicks: usize) -> Vec<f32> {
    let mut b = Buf::new(rate, 0.07);
    for i in 0..clicks {
        let t = i as f32 * 0.009;
        let amp = if i == 0 { 1.0 } else { 0.5 };
        let detuned: Vec<Mode> = modes
            .iter()
            .map(|&(f, tau, a)| (f * rng.range(0.98, 1.02), tau, a))
            .collect();
        b.modes(rng, t, &detuned, amp * 0.6);
        b.noise(
            rng,
            t,
            0.0002,
            0.0015,
            0.01,
            Color::White,
            &[Filter::Bp(3000.0, 1.0)],
            amp * 0.4,
        );
    }
    b.finish(UI_LOUDNESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Peak-to-neighbourhood ratio of a Welch power spectrum: high for
    /// sounds with distinct resonances, low for broadband noise.
    fn tonality(data: &[f32], rate: f32) -> f32 {
        const N: usize = 1024;
        let bins: Vec<f32> = (4..128).map(|k| k as f32 * rate / N as f32).collect();
        let mut power = vec![0.0f64; bins.len()];
        let mut start = 0;
        while start + N <= data.len() {
            for (i, &f) in bins.iter().enumerate() {
                // Goertzel on a Hann-windowed segment.
                let w = std::f64::consts::TAU * f as f64 / rate as f64;
                let c = 2.0 * w.cos();
                let (mut s1, mut s2) = (0.0f64, 0.0f64);
                for n in 0..N {
                    let hann = 0.5 - 0.5 * (std::f64::consts::TAU * n as f64 / N as f64).cos();
                    let s = data[start + n] as f64 * hann + c * s1 - s2;
                    s2 = s1;
                    s1 = s;
                }
                power[i] += s1 * s1 + s2 * s2 - c * s1 * s2;
            }
            start += N / 2;
        }
        // Compare each bin with the geometric mean of its left and right
        // neighbourhoods so that a smooth spectral slope scores about 1.
        let mean =
            |r: std::ops::Range<usize>| r.clone().map(|j| power[j]).sum::<f64>() / r.len() as f64;
        (10..power.len() - 10)
            .map(|i| {
                let side = (mean(i - 10..i - 2) * mean(i + 3..i + 11)).sqrt();
                if side > 0.0 { power[i] / side } else { 0.0 }
            })
            .fold(0.0f64, f64::max) as f32
    }

    fn joined(bank: &Bank, sound: Sound) -> Vec<f32> {
        let mut all = Vec::new();
        for v in 0..sound.spec().variants {
            all.extend_from_slice(bank.get(sound.index(), v));
            all.extend(std::iter::repeat_n(0.0, 512));
        }
        all
    }

    #[test]
    fn bank_is_deterministic_bounded_and_complete() {
        let a = Bank::generate(48_000);
        let b = Bank::generate(48_000);
        assert_eq!(Sound::all().len(), COUNT);
        let mut names: Vec<_> = Sound::all().iter().map(|s| s.name()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), COUNT);
        for (i, sound) in Sound::all().into_iter().enumerate() {
            assert_eq!(sound.index(), i);
            for v in 0..sound.spec().variants {
                let x = a.get(i, v);
                assert_eq!(x, b.get(i, v), "{} not deterministic", sound.name());
                let seconds = x.len() as f32 / 48_000.0;
                assert!(
                    (0.02..=0.6).contains(&seconds),
                    "{} length {seconds}",
                    sound.name()
                );
                assert!(x.iter().all(|s| s.is_finite() && s.abs() <= 0.95));
                assert!(
                    Buf::loudness(x, 48_000.0) > 0.05,
                    "{} too quiet",
                    sound.name()
                );
            }
            // Variants must actually differ.
            if sound.spec().variants > 1 {
                assert_ne!(a.get(i, 0), a.get(i, 1), "{}", sound.name());
            }
        }
        let kb = a.sample_count() * 4 / 1024;
        assert!(kb < 8 * 1024, "bank uses {kb} KiB");
    }

    #[test]
    #[ignore]
    fn print_tonality() {
        let bank = Bank::generate(48_000);
        for s in Sound::all() {
            println!(
                "{:14} {:6.1}",
                s.name(),
                tonality(&joined(&bank, s), 48_000.0)
            );
        }
    }
    #[test]
    fn noisy_and_resonant_materials_differ_spectrally() {
        let bank = Bank::generate(48_000);
        let t = |k, m| tonality(&joined(&bank, Sound::Block(k, m)), 48_000.0);
        let noisy = [Material::Grass, Material::Sand, Material::Dirt];
        // Shattering glass is deliberately noisy; its place sound rings.
        let resonant = [Material::Stone, Material::Wood];
        for kind in [Kind::Break, Kind::Place] {
            let noise_max = noisy.iter().map(|m| t(kind, *m)).fold(0.0, f32::max);
            let tone_min = resonant
                .iter()
                .map(|m| t(kind, *m))
                .fold(f32::MAX, f32::min);
            assert!(
                tone_min > noise_max * 1.5,
                "{kind:?}: resonant {tone_min:.1} vs noisy {noise_max:.1}"
            );
        }
    }

    #[test]
    fn wav_export_writes_valid_headers() {
        let dir = std::env::temp_dir().join(format!("voxel-wav-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let bank = Bank::generate(22_050);
        let n = bank.write_wavs(&dir).unwrap();
        assert_eq!(n, bank.clip_count());
        let bytes = std::fs::read(dir.join("break-stone-1.wav")).unwrap();
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(
            u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            22_050
        );
        let data = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 44 + data);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
