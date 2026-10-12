use std::f32::consts::TAU;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

#[derive(Clone, Copy, Debug)]
pub enum AudioEvent {
    Move,
    Capture,
    Check,
    Invalid,
    Victory,
    Defeat,
    Draw,
}

#[derive(Clone)]
struct Voice {
    frequency: f32,
    phase: f32,
    remaining: usize,
    total: usize,
    delay: usize,
    amplitude: f32,
}

struct Mixer {
    sample_rate: f32,
    voices: Vec<Voice>,
}

impl Mixer {
    fn next_sample(&mut self) -> f32 {
        let mut sample = 0.0;
        for voice in &mut self.voices {
            if voice.delay > 0 {
                voice.delay -= 1;
                continue;
            }
            if voice.remaining == 0 {
                continue;
            }
            let elapsed = voice.total.saturating_sub(voice.remaining);
            let progress = elapsed as f32 / voice.total.max(1) as f32;
            let envelope = (progress * 16.0).min(1.0) * ((1.0 - progress) * 10.0).min(1.0);
            sample += voice.phase.sin() * voice.amplitude * envelope;
            voice.phase = (voice.phase + TAU * voice.frequency / self.sample_rate) % TAU;
            voice.remaining -= 1;
        }
        self.voices
            .retain(|voice| voice.remaining > 0 || voice.delay > 0);
        sample.clamp(-1.0, 1.0)
    }

    fn add_tone(&mut self, frequency: f32, duration_ms: u32, delay_ms: u32, amplitude: f32) {
        let total = (self.sample_rate * duration_ms as f32 / 1000.0).max(1.0) as usize;
        let delay = (self.sample_rate * delay_ms as f32 / 1000.0) as usize;
        self.voices.push(Voice {
            frequency,
            phase: 0.0,
            remaining: total,
            total,
            delay,
            amplitude,
        });
    }

    fn add_event(&mut self, event: AudioEvent) {
        match event {
            AudioEvent::Move => self.add_tone(560.0, 75, 0, 0.18),
            AudioEvent::Capture => {
                self.add_tone(620.0, 85, 0, 0.2);
                self.add_tone(360.0, 110, 70, 0.16);
            }
            AudioEvent::Check => {
                self.add_tone(760.0, 95, 0, 0.2);
                self.add_tone(980.0, 120, 90, 0.18);
            }
            AudioEvent::Invalid => self.add_tone(160.0, 90, 0, 0.15),
            AudioEvent::Victory => {
                self.add_tone(520.0, 110, 0, 0.18);
                self.add_tone(680.0, 110, 90, 0.18);
                self.add_tone(900.0, 180, 180, 0.2);
            }
            AudioEvent::Defeat => {
                self.add_tone(600.0, 120, 0, 0.16);
                self.add_tone(420.0, 140, 100, 0.16);
                self.add_tone(280.0, 190, 220, 0.18);
            }
            AudioEvent::Draw => {
                self.add_tone(500.0, 150, 0, 0.16);
                self.add_tone(500.0, 150, 160, 0.16);
            }
        }
    }
}

pub struct AudioEngine {
    mixer: Arc<Mutex<Mixer>>,
    stream: Option<cpal::Stream>,
    enabled: bool,
}

impl AudioEngine {
    pub fn new() -> Self {
        let mixer = Arc::new(Mutex::new(Mixer {
            sample_rate: 48_000.0,
            voices: Vec::new(),
        }));
        if cfg!(test) {
            return Self {
                mixer,
                stream: None,
                enabled: false,
            };
        }
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else {
            log::warn!("cpal: no default output device; sound disabled");
            return Self {
                mixer,
                stream: None,
                enabled: false,
            };
        };
        let Ok(config) = device.default_output_config() else {
            log::warn!("cpal: unable to query output configuration; sound disabled");
            return Self {
                mixer,
                stream: None,
                enabled: false,
            };
        };
        let sample_rate = config.sample_rate() as f32;
        if let Ok(mut guard) = mixer.lock() {
            guard.sample_rate = sample_rate;
        }
        let channels = config.channels() as usize;
        // Use the default 48 kHz configuration on TrimUI. Its dmix plugin
        // fixes the hardware period; arbitrary fixed periods are rejected.
        let stream_config = config.config();
        let callback_mixer = Arc::clone(&mixer);
        let error_callback = |error| log::error!("cpal output stream error: {error}");
        let stream_result = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                stream_config,
                move |data: &mut [f32], _| fill_output(data, channels, &callback_mixer),
                error_callback,
                None,
            ),
            cpal::SampleFormat::I16 => {
                let callback_mixer = Arc::clone(&mixer);
                device.build_output_stream(
                    stream_config,
                    move |data: &mut [i16], _| fill_output(data, channels, &callback_mixer),
                    |error| log::error!("cpal output stream error: {error}"),
                    None,
                )
            }
            cpal::SampleFormat::U16 => {
                let callback_mixer = Arc::clone(&mixer);
                device.build_output_stream(
                    stream_config,
                    move |data: &mut [u16], _| fill_output(data, channels, &callback_mixer),
                    |error| log::error!("cpal output stream error: {error}"),
                    None,
                )
            }
            format => {
                log::warn!("cpal: unsupported sample format {format:?}; sound disabled");
                return Self {
                    mixer,
                    stream: None,
                    enabled: false,
                };
            }
        };

        let stream = match stream_result {
            Ok(stream) => stream,
            Err(error) => {
                log::warn!("cpal: unable to create output stream: {error}; sound disabled");
                return Self {
                    mixer,
                    stream: None,
                    enabled: false,
                };
            }
        };
        if let Err(error) = stream.play() {
            log::warn!("cpal: unable to start output stream: {error}");
            return Self {
                mixer,
                stream: None,
                enabled: false,
            };
        }
        log::info!(
            "Audio: {sample_rate} Hz, {channels} channels, {:?}",
            config.sample_format()
        );
        Self {
            mixer,
            stream: Some(stream),
            enabled: true,
        }
    }

    pub fn is_available(&self) -> bool {
        self.stream.is_some()
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled && self.is_available();
        if !self.enabled
            && let Ok(mut mixer) = self.mixer.lock()
        {
            mixer.voices.clear();
        }
    }

    pub fn play(&self, event: AudioEvent) {
        if !self.enabled {
            return;
        }
        if let Ok(mut mixer) = self.mixer.lock() {
            mixer.add_event(event);
        }
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn fill_output<T>(data: &mut [T], channels: usize, mixer: &Arc<Mutex<Mixer>>)
where
    T: SampleOutput,
{
    let mut guard = mixer.lock().ok();
    for frame in data.chunks_mut(channels.max(1)) {
        let sample = guard.as_mut().map_or(0.0, |mixer| mixer.next_sample());
        for channel in frame {
            *channel = T::from_f32(sample);
        }
    }
}

trait SampleOutput: Sized {
    fn from_f32(sample: f32) -> Self;
}

impl SampleOutput for f32 {
    fn from_f32(sample: f32) -> Self {
        sample
    }
}

impl SampleOutput for i16 {
    fn from_f32(sample: f32) -> Self {
        (sample * i16::MAX as f32) as i16
    }
}

impl SampleOutput for u16 {
    fn from_f32(sample: f32) -> Self {
        ((sample * 0.5 + 0.5) * u16::MAX as f32) as u16
    }
}
