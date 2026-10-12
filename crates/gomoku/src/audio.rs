use cpal::{
    FromSample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use ringbuf::{
    HeapProd, HeapRb,
    traits::{Consumer, Producer, Split},
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};

#[derive(Clone, Copy, Debug)]
pub enum Sound {
    Place(bool),
    Win,
    Lose,
    Draw,
}

#[derive(Clone, Copy)]
pub struct Settings {
    pub muted: bool,
    pub effects_enabled: bool,
    pub music_enabled: bool,
    pub effects_volume: f32,
    pub music_volume: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            muted: false,
            effects_enabled: true,
            music_enabled: true,
            effects_volume: 0.6,
            music_volume: 0.2,
        }
    }
}

struct Controls {
    effects: AtomicU32,
    music: AtomicU32,
}
impl Controls {
    fn new(settings: Settings) -> Self {
        let controls = Self {
            effects: AtomicU32::new(0),
            music: AtomicU32::new(0),
        };
        controls.update(settings);
        controls
    }
    fn update(&self, settings: Settings) {
        self.effects.store(
            (if settings.effects_enabled && !settings.muted {
                settings.effects_volume.clamp(0.0, 1.0)
            } else {
                0.0
            })
            .to_bits(),
            Ordering::Relaxed,
        );
        self.music.store(
            (if settings.music_enabled && !settings.muted {
                settings.music_volume.clamp(0.0, 1.0)
            } else {
                0.0
            })
            .to_bits(),
            Ordering::Relaxed,
        );
    }
    fn levels(&self) -> (f32, f32) {
        (
            f32::from_bits(self.effects.load(Ordering::Relaxed)),
            f32::from_bits(self.music.load(Ordering::Relaxed)),
        )
    }
}

pub struct Audio {
    _stream: Option<Stream>,
    producer: HeapProd<Sound>,
    controls: Arc<Controls>,
    failed: Arc<AtomicBool>,
    pub settings: Settings,
}
impl Audio {
    pub fn new() -> Self {
        let settings = Settings::default();
        let controls = Arc::new(Controls::new(settings));
        let failed = Arc::new(AtomicBool::new(false));
        let (producer, mut consumer) = HeapRb::<Sound>::new(64).split();
        let result = (|| -> anyhow::Result<Stream> {
            let (device, config) = output_device()?;
            log::info!(
                "Audio: {} Hz, {} channels, {:?}",
                config.sample_rate(),
                config.channels(),
                config.sample_format()
            );
            let mut mixer = Mixer::new(config.sample_rate());
            let levels = Arc::clone(&controls);
            let render = move || {
                while let Some(event) = consumer.try_pop() {
                    mixer.trigger(event);
                }
                mixer.sample(levels.levels())
            };
            macro_rules! open {
                ($sample:ty) => {
                    open_stream::<$sample>(&device, config.into(), render, Arc::clone(&failed))
                };
            }
            let stream = match config.sample_format() {
                SampleFormat::F32 => open!(f32),
                SampleFormat::F64 => open!(f64),
                SampleFormat::I8 => open!(i8),
                SampleFormat::I16 => open!(i16),
                SampleFormat::I24 => open!(cpal::I24),
                SampleFormat::I32 => open!(i32),
                SampleFormat::I64 => open!(i64),
                SampleFormat::U8 => open!(u8),
                SampleFormat::U16 => open!(u16),
                SampleFormat::U32 => open!(u32),
                SampleFormat::U64 => open!(u64),
                other => return Err(anyhow::anyhow!("不支持的音频格式：{other}")),
            }?;
            stream.play()?;
            log::info!("Audio output stream started");
            Ok(stream)
        })();
        let stream = match result {
            Ok(stream) => Some(stream),
            Err(error) => {
                log::warn!("Audio unavailable: {error:#}");
                None
            }
        };
        Self {
            _stream: stream,
            producer,
            controls,
            failed,
            settings,
        }
    }
    #[cfg(test)]
    pub fn silent() -> Self {
        let settings = Settings::default();
        let (producer, _) = HeapRb::<Sound>::new(64).split();
        Self {
            _stream: None,
            producer,
            controls: Arc::new(Controls::new(settings)),
            failed: Arc::new(AtomicBool::new(false)),
            settings,
        }
    }

    pub fn available(&self) -> bool {
        self._stream.is_some() && !self.failed.load(Ordering::Relaxed)
    }
    pub fn update(&self) {
        self.controls.update(self.settings);
    }
    pub fn play(&mut self, sound: Sound) {
        if self.available() && self.settings.effects_enabled && !self.settings.muted {
            let _ = self.producer.try_push(sound);
        }
    }
}

fn output_device() -> anyhow::Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    device_for_host(cpal::default_host())
}

fn device_for_host(
    host: cpal::Host,
) -> anyhow::Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("未找到音频输出设备"))?;
    let config = device.default_output_config()?;
    Ok((device, config))
}

fn open_stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut render: impl FnMut() -> f32 + Send + 'static,
    failed: Arc<AtomicBool>,
) -> anyhow::Result<Stream> {
    let channels = usize::from(config.channels);
    Ok(device.build_output_stream(
        config,
        move |output: &mut [T], _| {
            for frame in output.chunks_mut(channels) {
                let sample = T::from_sample(render());
                frame.fill(sample);
            }
        },
        move |error| stream_error(error, &failed),
        None,
    )?)
}

fn stream_error(error: cpal::Error, failed: &AtomicBool) {
    match error.kind() {
        cpal::ErrorKind::Xrun
        | cpal::ErrorKind::DeviceChanged
        | cpal::ErrorKind::RealtimeDenied => {
            // ALSA prepares the PCM again after an xrun; keep accepting sounds.
            log::debug!("Recoverable audio event: {error}");
        }
        _ => {
            log::warn!("Audio stream error: {error}");
            failed.store(true, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Voice {
    age: f32,
    duration: f32,
    frequency: f32,
    amplitude: f32,
    click: bool,
}

struct Mixer {
    rate: f32,
    clock: f64,
    voices: [Voice; 16],
    next_voice: usize,
    noise: u32,
    effects_gain: f32,
    music_gain: f32,
}
impl Mixer {
    fn new(rate: u32) -> Self {
        Self {
            rate: rate.max(1) as f32,
            clock: 0.0,
            voices: [Voice::default(); 16],
            next_voice: 0,
            noise: 0x1234abcd,
            effects_gain: 0.0,
            music_gain: 0.0,
        }
    }
    fn voice(&mut self, frequency: f32, amplitude: f32, duration: f32, delay: f32, click: bool) {
        self.voices[self.next_voice] = Voice {
            age: -delay,
            duration,
            frequency,
            amplitude,
            click,
        };
        self.next_voice = (self.next_voice + 1) % self.voices.len();
    }
    fn trigger(&mut self, sound: Sound) {
        match sound {
            Sound::Place(black) => {
                self.voice(if black { 440.0 } else { 580.0 }, 0.55, 0.14, 0.0, true)
            }
            Sound::Win => {
                for (i, midi) in [72, 76, 79, 84].into_iter().enumerate() {
                    self.voice(note(midi), 0.22, 0.75, i as f32 * 0.13, false);
                }
            }
            Sound::Lose => {
                for (i, midi) in [67, 64, 60].into_iter().enumerate() {
                    self.voice(note(midi), 0.22, 0.65, i as f32 * 0.16, false);
                }
            }
            Sound::Draw => {
                self.voice(note(67), 0.2, 0.5, 0.0, false);
                self.voice(note(72), 0.2, 0.6, 0.18, false);
            }
        }
    }
    fn sample(&mut self, (effects, music): (f32, f32)) -> f32 {
        let dt = 1.0 / self.rate;
        let smoothing = (dt * 80.0).min(1.0);
        self.effects_gain += (effects - self.effects_gain) * smoothing;
        self.music_gain += (music - self.music_gain) * smoothing;
        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 17;
        self.noise ^= self.noise << 5;
        let noise = (self.noise as f32 / u32::MAX as f32) * 2.0 - 1.0;
        let mut fx = 0.0;
        for voice in &mut self.voices {
            if voice.age < voice.duration {
                if voice.age >= 0.0 {
                    let t = voice.age;
                    let attack = (t / 0.003).min(1.0);
                    let envelope = attack
                        * (-t * if voice.click { 40.0 } else { 6.0 }).exp()
                        * ((voice.duration - t) / 0.03).clamp(0.0, 1.0);
                    let phase = std::f32::consts::TAU * voice.frequency * t;
                    let tone = phase.sin() + 0.25 * (2.01 * phase).sin();
                    fx += voice.amplitude
                        * envelope
                        * if voice.click {
                            tone * 0.55 + noise * (-t * 120.0).exp() * 0.35
                        } else {
                            tone
                        };
                }
                voice.age += dt;
            }
        }
        let bgm = music_sample(self.clock);
        self.clock = (self.clock + f64::from(dt)) % MUSIC_LENGTH;
        (fx * self.effects_gain + bgm * self.music_gain).clamp(-0.95, 0.95)
    }
}

fn note(midi: u8) -> f32 {
    440.0 * 2.0_f32.powf((f32::from(midi) - 69.0) / 12.0)
}
const STEP: f64 = 60.0 / 72.0 / 2.0;
const MUSIC_LENGTH: f64 = STEP * 128.0;
// Original melody in C major pentatonic; 0 denotes a breath between phrases.
const MELODY: [u8; 64] = [
    72, 0, 76, 79, 81, 0, 79, 76, 74, 0, 72, 74, 76, 0, 0, 0, 76, 0, 79, 81, 84, 0, 81, 79, 76, 74,
    72, 0, 74, 0, 0, 0, 79, 0, 76, 74, 72, 0, 74, 76, 79, 0, 81, 79, 76, 0, 0, 0, 74, 0, 76, 79,
    76, 0, 74, 72, 69, 0, 72, 74, 72, 0, 0, 0,
];
fn music_sample(clock: f64) -> f32 {
    let clock = clock.rem_euclid(MUSIC_LENGTH);
    let step = (clock / STEP) as usize;
    let t = (clock % STEP) as f32;
    let midi = MELODY[step % MELODY.len()];
    let mut value = 0.0;
    if midi != 0 {
        let frequency = note(if step >= 64 {
            midi.saturating_sub(12)
        } else {
            midi
        });
        let phase = std::f32::consts::TAU * frequency * t;
        let envelope =
            (t / 0.008).min(1.0) * (-t * 5.5).exp() * (((STEP as f32) - t) / 0.045).clamp(0.0, 1.0);
        value += (phase.sin() + 0.22 * (phase * 2.0).sin() + 0.08 * (phase * 3.0).sin())
            * envelope
            * 0.34;
    }
    let bar = (clock / (STEP * 8.0)) as usize;
    let bass_midi = [48, 57, 53, 55][bar % 4];
    let bt = (clock % (STEP * 8.0)) as f32;
    let phase = std::f32::consts::TAU * note(bass_midi) * bt;
    let envelope = (bt / 0.03).min(1.0)
        * (-bt * 0.8).exp()
        * ((STEP as f32 * 8.0 - bt) / 0.15).clamp(0.0, 1.0);
    value + (phase.sin() + (phase * 1.5).sin() * 0.22) * envelope * 0.14
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn underrun_keeps_audio_available_but_device_loss_disables_it() {
        let failed = AtomicBool::new(false);
        stream_error(cpal::ErrorKind::Xrun.into(), &failed);
        assert!(!failed.load(Ordering::Relaxed));
        stream_error(cpal::ErrorKind::DeviceNotAvailable.into(), &failed);
        assert!(failed.load(Ordering::Relaxed));
    }
    #[test]
    fn mix_is_finite_bounded_and_contains_sound() {
        for rate in [22_050, 44_100, 48_000] {
            let mut mixer = Mixer::new(rate);
            for _ in 0..5 {
                for sound in [
                    Sound::Place(true),
                    Sound::Place(false),
                    Sound::Win,
                    Sound::Lose,
                    Sound::Draw,
                ] {
                    mixer.trigger(sound);
                }
            }
            let mut energy = 0.0;
            for _ in 0..rate {
                let sample = mixer.sample((1.0, 1.0));
                assert!(sample.is_finite() && sample.abs() <= 0.95);
                energy += sample.abs();
            }
            assert!(energy > 10.0);
        }
    }
    #[test]
    fn mute_and_loop() {
        let mut mixer = Mixer::new(48_000);
        mixer.trigger(Sound::Win);
        for _ in 0..1000 {
            assert_eq!(mixer.sample((0.0, 0.0)), 0.0);
        }
        for t in [0.0, 0.02, 2.5, 13.1, 50.0] {
            assert!((music_sample(t) - music_sample(t + MUSIC_LENGTH)).abs() < 0.002);
        }
        let controls = Controls::new(Settings::default());
        let settings = Settings {
            music_enabled: false,
            ..Settings::default()
        };
        controls.update(settings);
        assert_eq!(controls.levels(), (0.6, 0.0));
        let muted = Settings {
            muted: true,
            ..settings
        };
        controls.update(muted);
        assert_eq!(controls.levels(), (0.0, 0.0));
        controls.update(settings);
        assert_eq!(controls.levels(), (0.6, 0.0));
    }
}
