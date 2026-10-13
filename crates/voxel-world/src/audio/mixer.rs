//! Real-time voice mixer: plays bank samples with pitch, stereo gains and
//! per-category volume. Allocation-free once constructed.
use super::bank::{Bank, Category};
use glam::Vec3;

pub const VOICES: usize = 32;
const CATEGORIES: usize = 2;

/// A request to start one sample.
#[derive(Clone, Copy, Debug)]
pub struct Play {
    pub sound: u16,
    pub variant: u8,
    /// Playback rate relative to the bank (pitch × bank rate / output rate).
    pub step: f32,
    pub gain: [f32; 2],
    pub priority: u8,
    pub category: Category,
}

#[derive(Clone, Copy, Default)]
struct Voice {
    active: bool,
    sound: u16,
    variant: u8,
    position: f32,
    step: f32,
    gain: [f32; 2],
    priority: u8,
    category: u8,
    started: u64,
}

pub struct Mixer {
    voices: [Voice; VOICES],
    gains: [f32; CATEGORIES],
    smoothing: f32,
    started: u64,
}

/// Linear distance attenuation and equal-power panning relative to a
/// listener at `ear` looking along `yaw` (0 = −Z, increasing clockwise).
pub fn spatial(ear: Vec3, yaw: f32, source: Vec3, range: f32) -> [f32; 2] {
    let d = source - ear;
    let distance = d.length();
    let attenuation = (1.0 - distance / range.max(0.1)).clamp(0.0, 1.0);
    if attenuation == 0.0 {
        return [0.0, 0.0];
    }
    let right = Vec3::new(yaw.cos(), 0.0, yaw.sin());
    // Sources very close to the head are heard in both ears.
    let pan = if distance > 1e-3 {
        (d.dot(right) / distance * (distance / 2.0).min(1.0)).clamp(-0.8, 0.8)
    } else {
        0.0
    };
    let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
    let s = std::f32::consts::SQRT_2 * attenuation;
    [angle.cos() * s, angle.sin() * s]
}

/// Transparent below 0.7, smoothly saturating towards 1.0 above.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= 0.7 {
        x
    } else {
        x.signum() * (0.7 + 0.3 * ((a - 0.7) / 0.3).tanh())
    }
}

impl Mixer {
    pub fn new(rate: u32) -> Self {
        Self {
            voices: [Voice::default(); VOICES],
            gains: [0.0; CATEGORIES],
            // About 10 ms to follow volume changes without zipper noise.
            smoothing: 1.0 - (-1.0 / (0.01 * rate.max(1) as f32)).exp(),
            started: 0,
        }
    }
    pub fn active(&self) -> usize {
        self.voices.iter().filter(|v| v.active).count()
    }
    pub fn start(&mut self, play: Play) {
        if play.step <= 0.0 || !play.step.is_finite() || play.gain == [0.0, 0.0] {
            return;
        }
        // A free voice, otherwise the oldest of the lowest priority that is
        // not above the new sound's priority.
        let slot = self.voices.iter().position(|v| !v.active).or_else(|| {
            self.voices
                .iter()
                .enumerate()
                .filter(|(_, v)| v.priority <= play.priority)
                .min_by_key(|(_, v)| (v.priority, v.started))
                .map(|(i, _)| i)
        });
        let Some(slot) = slot else {
            return;
        };
        self.started += 1;
        self.voices[slot] = Voice {
            active: true,
            sound: play.sound,
            variant: play.variant,
            position: 0.0,
            step: play.step,
            gain: play.gain,
            priority: play.priority,
            category: play.category as u8,
            started: self.started,
        };
    }
    /// Mixes into stereo frames; `targets` are the category volumes.
    pub fn render(&mut self, bank: &Bank, out: &mut [[f32; 2]], targets: [f32; CATEGORIES]) {
        for frame in out.iter_mut() {
            *frame = [0.0, 0.0];
        }
        let mut ramps = [[0.0f32; 2]; CATEGORIES];
        for (c, ramp) in ramps.iter_mut().enumerate() {
            let start = self.gains[c];
            // Advance the one-pole smoother over the whole buffer at once.
            let end =
                targets[c] + (start - targets[c]) * (1.0 - self.smoothing).powi(out.len() as i32);
            *ramp = [start, end];
            self.gains[c] = end;
        }
        let n = out.len().max(1) as f32;
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            let data = bank.get(usize::from(voice.sound), usize::from(voice.variant));
            let last = data.len().saturating_sub(1);
            let [g0, g1] = ramps[usize::from(voice.category)];
            for (i, frame) in out.iter_mut().enumerate() {
                let index = voice.position as usize;
                if index >= last {
                    voice.active = false;
                    break;
                }
                let frac = voice.position - index as f32;
                let s = data[index] + (data[index + 1] - data[index]) * frac;
                let g = g0 + (g1 - g0) * (i as f32 / n);
                frame[0] += s * voice.gain[0] * g;
                frame[1] += s * voice.gain[1] * g;
                voice.position += voice.step;
            }
        }
        for frame in out.iter_mut() {
            frame[0] = soft_clip(frame[0]);
            frame[1] = soft_clip(frame[1]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::bank::{Kind, Material, Sound};

    fn play(sound: Sound, gain: [f32; 2]) -> Play {
        Play {
            sound: sound.index() as u16,
            variant: 0,
            step: 1.0,
            gain,
            priority: sound.spec().priority,
            category: sound.spec().category,
        }
    }
    fn energy(mixer: &mut Mixer, bank: &Bank, frames: usize) -> [f32; 2] {
        let mut out = vec![[0.0; 2]; 256];
        let mut e = [0.0; 2];
        for _ in 0..frames / 256 {
            mixer.render(bank, &mut out, [1.0, 1.0]);
            for f in &out {
                assert!(f[0].is_finite() && f[1].is_finite());
                assert!(f[0].abs() <= 1.0 && f[1].abs() <= 1.0);
                e[0] += f[0] * f[0];
                e[1] += f[1] * f[1];
            }
        }
        e
    }

    #[test]
    fn source_on_the_right_is_louder_on_the_right() {
        let ear = Vec3::new(0.0, 64.0, 0.0);
        // Facing −Z, +X is to the right.
        let g = spatial(ear, 0.0, ear + Vec3::new(4.0, 0.0, 0.0), 16.0);
        assert!(g[1] > g[0] * 3.0, "{g:?}");
        let g = spatial(
            ear,
            std::f32::consts::PI,
            ear + Vec3::new(4.0, 0.0, 0.0),
            16.0,
        );
        assert!(g[0] > g[1] * 3.0, "turned around: {g:?}");
        let centre = spatial(ear, 0.0, ear - Vec3::Z * 3.0, 16.0);
        assert!((centre[0] - centre[1]).abs() < 1e-4);
        let bank = Bank::generate(48_000);
        let mut mixer = Mixer::new(48_000);
        let right = spatial(ear, 0.0, ear + Vec3::new(4.0, 0.0, 1.0), 16.0);
        mixer.start(play(Sound::Block(Kind::Break, Material::Stone), right));
        let e = energy(&mut mixer, &bank, 48_000 / 2);
        assert!(e[1] > e[0] * 2.0, "{e:?}");
    }

    #[test]
    fn distance_attenuation_is_monotonic() {
        let ear = Vec3::ZERO;
        let mut previous = f32::MAX;
        for d in 0..20 {
            let g = spatial(ear, 0.3, Vec3::new(0.0, 0.0, -(d as f32)), 16.0);
            let power = g[0] * g[0] + g[1] * g[1];
            assert!(power <= previous + 1e-6, "{d}");
            previous = power;
        }
        assert_eq!(
            spatial(ear, 0.0, Vec3::new(0.0, 0.0, -17.0), 16.0),
            [0.0, 0.0]
        );
    }

    #[test]
    fn many_simultaneous_sounds_stay_bounded_and_steal_low_priority() {
        let bank = Bank::generate(48_000);
        let mut mixer = Mixer::new(48_000);
        for i in 0..64 {
            let m = Material::ALL[i % Material::ALL.len()];
            mixer.start(play(Sound::Block(Kind::Break, m), [1.4, 1.4]));
        }
        assert_eq!(mixer.active(), VOICES);
        let e = energy(&mut mixer, &bank, 48_000);
        assert!(e[0] > 1.0);
        assert_eq!(mixer.active(), 0, "all voices finish");
        // Steps never steal from breaks; UI always gets a voice.
        for _ in 0..VOICES {
            mixer.start(play(Sound::Block(Kind::Break, Material::Dirt), [1.0, 1.0]));
        }
        mixer.start(play(Sound::Block(Kind::Step, Material::Dirt), [1.0, 1.0]));
        assert!(mixer.voices.iter().all(|v| v.priority == 2));
        mixer.start(play(Sound::UiClick, [1.0, 1.0]));
        assert!(mixer.voices.iter().any(|v| v.priority == 3));
    }

    #[test]
    fn muted_category_is_silent_and_clip_is_transparent_when_quiet() {
        let bank = Bank::generate(48_000);
        let mut mixer = Mixer::new(48_000);
        mixer.start(play(Sound::Block(Kind::Place, Material::Wood), [1.0, 1.0]));
        let mut out = vec![[0.0; 2]; 512];
        for _ in 0..20 {
            mixer.render(&bank, &mut out, [0.0, 1.0]);
            assert!(out.iter().all(|f| f[0] == 0.0 && f[1] == 0.0));
        }
        assert_eq!(soft_clip(0.5), 0.5);
        assert!(soft_clip(5.0) <= 1.0 && soft_clip(-5.0) >= -1.0);
        assert!(soft_clip(0.8) > 0.7 && soft_clip(0.8) < 0.8);
    }
}
