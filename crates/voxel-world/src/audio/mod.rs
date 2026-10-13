//! Procedural audio: a synthesised sample bank played by a stereo mixer on
//! the CPAL/ALSA output stream.
//!
//! The game thread sends play requests through a lock-free ring buffer; the
//! bank is generated on a background thread and published once ready. The
//! audio callback performs no allocation, locking or logging.
mod bank;
mod mixer;
mod synth;

pub use bank::{Bank, Kind, Material, Sound};
use cpal::{
    FromSample, SampleFormat, SizedSample, Stream,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use glam::Vec3;
use mixer::{Mixer, Play};
use ringbuf::{
    HeapProd, HeapRb,
    traits::{Consumer, Producer, Split},
};
use std::{
    path::Path,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
    time::Instant,
};

/// Volumes shared with the callback as f32 bits.
struct Levels {
    master: AtomicU32,
    effects: AtomicU32,
}
impl Levels {
    fn targets(&self) -> [f32; 2] {
        let master = f32::from_bits(self.master.load(Ordering::Relaxed));
        let effects = f32::from_bits(self.effects.load(Ordering::Relaxed));
        // Interface sounds follow the effects volume.
        [master * effects, master * effects]
    }
}

/// Callback timing, written by the audio thread and drained by the game.
#[derive(Default)]
struct Stats {
    callbacks: AtomicU64,
    frames: AtomicU64,
    busy_ns: AtomicU64,
    max_ns: AtomicU64,
    voices_peak: AtomicU32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AudioStats {
    pub callbacks: u64,
    pub frames_per_callback: f32,
    pub avg_us: f32,
    pub max_us: f32,
    /// Share of real time spent in the callback.
    pub load: f32,
    pub voices_peak: u32,
}

pub struct Audio {
    stream: Option<Stream>,
    producer: HeapProd<Play>,
    levels: Arc<Levels>,
    failed: Arc<AtomicBool>,
    stats: Arc<Stats>,
    bank_ready: Arc<OnceLock<Arc<Bank>>>,
    rate: u32,
    rng: synth::Rng,
    last_variant: Vec<u8>,
    ear: Vec3,
    yaw: f32,
    /// Sounds requested in tests, recorded even without an output device.
    #[cfg(test)]
    pub played: Vec<Sound>,
}

impl Audio {
    pub fn new() -> Self {
        let mut audio = Self::detached();
        let result = (|| -> anyhow::Result<(Stream, u32)> {
            let (device, config) = output_device()?;
            log::info!(
                "Audio: {} Hz, {} channels, {:?}",
                config.sample_rate(),
                config.channels(),
                config.sample_format()
            );
            let rate = config.sample_rate();
            let (producer, mut consumer) = HeapRb::<Play>::new(128).split();
            audio.producer = producer;
            let mut mixer = Mixer::new(rate);
            let levels = Arc::clone(&audio.levels);
            let stats = Arc::clone(&audio.stats);
            let ready = Arc::clone(&audio.bank_ready);
            let mut bank: Option<Arc<Bank>> = None;
            let render = move |out: &mut [[f32; 2]]| {
                let start = Instant::now();
                if bank.is_none() {
                    bank = ready.get().cloned();
                }
                match &bank {
                    Some(bank) => {
                        while let Some(play) = consumer.try_pop() {
                            mixer.start(play);
                        }
                        mixer.render(bank, out, levels.targets());
                    }
                    None => {
                        while consumer.try_pop().is_some() {}
                        out.fill([0.0, 0.0]);
                    }
                }
                let ns = start.elapsed().as_nanos() as u64;
                stats.callbacks.fetch_add(1, Ordering::Relaxed);
                stats.frames.fetch_add(out.len() as u64, Ordering::Relaxed);
                stats.busy_ns.fetch_add(ns, Ordering::Relaxed);
                stats.max_ns.fetch_max(ns, Ordering::Relaxed);
                stats
                    .voices_peak
                    .fetch_max(mixer.active() as u32, Ordering::Relaxed);
            };
            macro_rules! open {
                ($sample:ty) => {
                    open_stream::<$sample>(
                        &device,
                        config.into(),
                        render,
                        Arc::clone(&audio.failed),
                    )
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
            Ok((stream, rate))
        })();
        match result {
            Ok((stream, rate)) => {
                audio.stream = Some(stream);
                audio.rate = rate;
                let ready = Arc::clone(&audio.bank_ready);
                let spawned = std::thread::Builder::new()
                    .name("voxel-sound-bank".into())
                    .spawn(move || {
                        let start = Instant::now();
                        let bank = Bank::generate(rate);
                        log::info!(
                            "Audio bank ready: {} sounds, {} clips, {:.0} ms, {} KiB",
                            bank::COUNT,
                            bank.clip_count(),
                            start.elapsed().as_secs_f64() * 1000.0,
                            bank.sample_count() * 4 / 1024
                        );
                        let _ = ready.set(Arc::new(bank));
                    });
                if let Err(e) = spawned {
                    log::warn!("Sound bank thread failed: {e}");
                }
            }
            Err(error) => log::warn!("Audio unavailable: {error:#}"),
        }
        audio
    }
    fn detached() -> Self {
        let (producer, _) = HeapRb::<Play>::new(1).split();
        Self {
            stream: None,
            producer,
            levels: Arc::new(Levels {
                master: AtomicU32::new(1.0f32.to_bits()),
                effects: AtomicU32::new(0.7f32.to_bits()),
            }),
            failed: Arc::new(AtomicBool::new(false)),
            stats: Arc::new(Stats::default()),
            bank_ready: Arc::new(OnceLock::new()),
            rate: 48_000,
            rng: synth::Rng::new(0x5151),
            last_variant: vec![u8::MAX; bank::COUNT],
            ear: Vec3::ZERO,
            yaw: 0.0,
            #[cfg(test)]
            played: Vec::new(),
        }
    }
    #[cfg(test)]
    pub fn silent() -> Self {
        Self::detached()
    }
    pub fn available(&self) -> bool {
        self.stream.is_some() && !self.failed.load(Ordering::Relaxed)
    }
    pub fn set_levels(&self, master: f32, effects: f32) {
        self.levels
            .master
            .store(master.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        self.levels
            .effects
            .store(effects.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
    /// Position and heading used for positional sounds.
    pub fn set_listener(&mut self, ear: Vec3, yaw: f32) {
        self.ear = ear;
        self.yaw = yaw;
    }
    /// Non-positional sound (interface).
    pub fn play(&mut self, sound: Sound) {
        self.start(sound, [1.0, 1.0], 1.0, 1.0);
    }
    /// Positional sound with extra volume and pitch factors.
    pub fn play_at(&mut self, sound: Sound, position: Vec3, volume: f32, pitch: f32) {
        let gain = mixer::spatial(self.ear, self.yaw, position, sound.spec().range);
        self.start(sound, gain, volume, pitch);
    }
    fn start(&mut self, sound: Sound, gain: [f32; 2], volume: f32, pitch: f32) {
        #[cfg(test)]
        self.played.push(sound);
        if !self.available() || self.bank_ready.get().is_none() {
            return;
        }
        let spec = sound.spec();
        let index = sound.index();
        // Pick a different variant than last time to avoid machine-gunning.
        let mut variant = self.rng.below(spec.variants) as u8;
        if spec.variants > 1 && variant == self.last_variant[index] {
            variant = ((usize::from(variant) + 1 + self.rng.below(spec.variants - 1))
                % spec.variants) as u8;
        }
        self.last_variant[index] = variant;
        let jitter = 1.0 + spec.pitch_jitter * self.rng.noise();
        let volume = spec.volume * volume;
        let bank_rate = self.bank_ready.get().map_or(self.rate, |b| b.rate);
        let _ = self.producer.try_push(Play {
            sound: index as u16,
            variant,
            step: pitch * jitter * bank_rate as f32 / self.rate as f32,
            gain: [gain[0] * volume, gain[1] * volume],
            priority: spec.priority,
            category: spec.category,
        });
    }
    /// Returns and resets callback statistics since the previous call.
    pub fn take_stats(&self) -> AudioStats {
        let s = &self.stats;
        let callbacks = s.callbacks.swap(0, Ordering::Relaxed);
        let frames = s.frames.swap(0, Ordering::Relaxed);
        let busy = s.busy_ns.swap(0, Ordering::Relaxed);
        let max = s.max_ns.swap(0, Ordering::Relaxed);
        let voices_peak = s.voices_peak.swap(0, Ordering::Relaxed);
        let seconds = frames as f32 / self.rate.max(1) as f32;
        AudioStats {
            callbacks,
            frames_per_callback: frames as f32 / callbacks.max(1) as f32,
            avg_us: busy as f32 / callbacks.max(1) as f32 / 1000.0,
            max_us: max as f32 / 1000.0,
            load: if seconds > 0.0 {
                busy as f32 / 1e9 / seconds
            } else {
                0.0
            },
            voices_peak,
        }
    }
}

/// Writes the whole bank as WAV files for listening on a PC.
pub fn dump_sounds(dir: &Path, rate: u32) -> anyhow::Result<usize> {
    let start = Instant::now();
    let bank = Bank::generate(rate);
    let n = bank.write_wavs(dir)?;
    log::info!(
        "Dumped {n} clips ({} sounds) to {} in {:.0} ms",
        bank::COUNT,
        dir.display(),
        start.elapsed().as_secs_f64() * 1000.0
    );
    Ok(n)
}

fn output_device() -> anyhow::Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("未找到音频输出设备"))?;
    let config = device.default_output_config()?;
    Ok((device, config))
}

fn open_stream<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut render: impl FnMut(&mut [[f32; 2]]) + Send + 'static,
    failed: Arc<AtomicBool>,
) -> anyhow::Result<Stream> {
    let channels = usize::from(config.channels).max(1);
    let mut scratch = [[0.0f32; 2]; 256];
    Ok(device.build_output_stream(
        config,
        move |output: &mut [T], _| {
            for chunk in output.chunks_mut(channels * scratch.len()) {
                let frames = chunk.len() / channels;
                let mix = &mut scratch[..frames];
                render(mix);
                for (frame, [l, r]) in chunk.chunks_mut(channels).zip(mix.iter().copied()) {
                    match frame.len() {
                        1 => frame[0] = T::from_sample((l + r) * 0.5),
                        _ => {
                            frame[0] = T::from_sample(l);
                            frame[1] = T::from_sample(r);
                            for extra in &mut frame[2..] {
                                *extra = T::from_sample((l + r) * 0.5);
                            }
                        }
                    }
                }
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
    fn levels_combine_master_and_effects() {
        let audio = Audio::silent();
        audio.set_levels(0.5, 0.8);
        assert_eq!(audio.levels.targets(), [0.4, 0.4]);
        audio.set_levels(2.0, -1.0);
        assert_eq!(audio.levels.targets(), [0.0, 0.0]);
        assert_eq!(audio.take_stats().callbacks, 0);
    }
}
