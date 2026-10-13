//! Global settings shared by every world, stored as `config/settings.json`.
//!
//! Writes are atomic (temporary file, fsync, rename) and run on a background
//! thread. A file that cannot be parsed is kept as `settings.json.bak` and the
//! defaults are used instead; it is never silently overwritten in place.
use crate::input::Bindings;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    thread::JoinHandle,
};

pub const VERSION: u32 = 1;
pub const VIEW_DISTANCE: std::ops::RangeInclusive<i32> = 2..=6;
pub const SENSITIVITY: std::ops::RangeInclusive<f32> = 0.5..=2.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub video: Video,
    pub audio: Audio,
    pub controls: Controls,
    pub advanced: Advanced,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            version: VERSION,
            video: Video::default(),
            audio: Audio::default(),
            controls: Controls::default(),
            advanced: Advanced::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Video {
    pub view_distance: i32,
    pub smooth_lighting: bool,
    /// Leaves with see-through holes (alpha tested) near the player instead
    /// of solid cubes. Off by default: it costs about 10 % of the frame
    /// rate on the GE8300.
    pub fancy_leaves: bool,
    pub clouds: bool,
}
impl Default for Video {
    fn default() -> Self {
        Self {
            view_distance: 4,
            smooth_lighting: true,
            fancy_leaves: false,
            clouds: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Audio {
    pub master: f32,
    pub effects: f32,
}
impl Default for Audio {
    fn default() -> Self {
        Self {
            master: 1.0,
            effects: 0.7,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Controls {
    pub sensitivity: f32,
    pub invert_y: bool,
    pub bindings: Bindings,
}
impl Default for Controls {
    fn default() -> Self {
        Self {
            sensitivity: 1.0,
            invert_y: false,
            bindings: Bindings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Advanced {
    pub all_cores: bool,
    pub debug_overlay: bool,
}
impl Default for Advanced {
    fn default() -> Self {
        Self {
            all_cores: true,
            debug_overlay: false,
        }
    }
}

fn clamp_or(value: f32, range: std::ops::RangeInclusive<f32>, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(*range.start(), *range.end())
    } else {
        fallback
    }
}

impl Config {
    /// Brings every value into range; used after loading and before saving.
    pub fn sanitize(&mut self) {
        let d = Self::default();
        self.version = VERSION;
        self.video.view_distance = self
            .video
            .view_distance
            .clamp(*VIEW_DISTANCE.start(), *VIEW_DISTANCE.end());
        self.audio.master = clamp_or(self.audio.master, 0.0..=1.0, d.audio.master);
        self.audio.effects = clamp_or(self.audio.effects, 0.0..=1.0, d.audio.effects);
        self.controls.sensitivity = clamp_or(
            self.controls.sensitivity,
            SENSITIVITY,
            d.controls.sensitivity,
        );
        // Snap to the 0.1 steps used by the menu.
        self.controls.sensitivity = (self.controls.sensitivity * 10.0).round() / 10.0;
        self.audio.master = (self.audio.master * 10.0).round() / 10.0;
        self.audio.effects = (self.audio.effects * 10.0).round() / 10.0;
    }
}

const FILE: &str = "settings.json";

pub enum Loaded {
    Existing(Config),
    Fresh(Config),
    /// The file was unreadable; it was moved aside and defaults are used.
    Recovered(Config, String),
}

pub struct ConfigStore {
    dir: Option<PathBuf>,
    saved: Config,
    writer: Option<JoinHandle<std::result::Result<(), String>>>,
}

impl ConfigStore {
    /// A store that never touches the disk (benchmarks).
    pub fn memory(config: &Config) -> Self {
        Self {
            dir: None,
            saved: config.clone(),
            writer: None,
        }
    }
    pub fn load(dir: &Path) -> (Self, Loaded) {
        let path = dir.join(FILE);
        let loaded = match read(&path) {
            Ok(Some(mut config)) => {
                config.sanitize();
                log::info!("Config loaded: {}", path.display());
                Loaded::Existing(config)
            }
            Ok(None) => Loaded::Fresh(Config::default()),
            Err(e) => {
                let backup = dir.join(format!("{FILE}.bak"));
                let moved = fs::rename(&path, &backup);
                log::error!("Config unreadable ({e:#}); backup: {moved:?}");
                let text = match moved {
                    Ok(()) => format!("设置文件损坏，已备份为 {FILE}.bak 并恢复默认"),
                    Err(_) => "设置文件损坏，已恢复默认设置".to_owned(),
                };
                Loaded::Recovered(Config::default(), text)
            }
        };
        let saved = match &loaded {
            Loaded::Existing(c) => c.clone(),
            // Nothing valid on disk: the first save must write.
            _ => Config {
                version: 0,
                ..Config::default()
            },
        };
        (
            Self {
                dir: Some(dir.to_owned()),
                saved,
                writer: None,
            },
            loaded,
        )
    }
    /// Starts a background write when `config` differs from the last save.
    /// Returns an error message from the previous write, if it failed.
    pub fn save(&mut self, config: &Config) -> Option<String> {
        let mut config = config.clone();
        config.sanitize();
        if config == self.saved {
            return None;
        }
        let error = self.join();
        let Some(dir) = self.dir.clone() else {
            self.saved = config;
            return error;
        };
        self.saved = config.clone();
        self.writer = Some(std::thread::spawn(move || {
            write(&dir, &config).map_err(|e| format!("{e:#}"))
        }));
        error
    }
    fn join(&mut self) -> Option<String> {
        match self.writer.take()?.join() {
            Ok(Ok(())) => None,
            Ok(Err(e)) => {
                log::error!("Config save failed: {e}");
                // Allow the next save attempt to retry.
                self.saved.version = 0;
                Some(e)
            }
            Err(_) => Some("Config writer panicked".into()),
        }
    }
    pub fn flush(&mut self) -> Result<()> {
        match self.join() {
            None => Ok(()),
            Some(e) => Err(anyhow::anyhow!("Saving settings: {e}")),
        }
    }
}

impl Drop for ConfigStore {
    fn drop(&mut self) {
        self.join();
    }
}

fn read(path: &Path) -> Result<Option<Config>> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut text = String::new();
    file.take(256 * 1024).read_to_string(&mut text)?;
    let config: Config = serde_json::from_str(&text).context("Parsing settings")?;
    ensure!(config.version <= VERSION, "Settings from a newer version");
    Ok(Some(config))
}

fn write(dir: &Path, config: &Config) -> Result<()> {
    fs::create_dir_all(dir)?;
    let temp = dir.join(format!("{FILE}.tmp"));
    let mut file = File::create(&temp)?;
    file.write_all(&serde_json::to_vec_pretty(config)?)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temp, dir.join(FILE))?;
    File::open(dir)?.sync_all()?;
    log::info!("Config saved: {}", dir.join(FILE).display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Action, Button};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("voxel-config-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trip_and_only_writes_changes() {
        let dir = temp("roundtrip");
        let (mut store, loaded) = ConfigStore::load(&dir);
        let mut config = match loaded {
            Loaded::Fresh(c) => c,
            _ => panic!("expected fresh config"),
        };
        config.controls.bindings.bind(Action::Jump, Button::Menu);
        config.video.view_distance = 6;
        config.controls.invert_y = true;
        assert!(store.save(&config).is_none());
        store.flush().unwrap();
        let first = fs::metadata(dir.join(FILE)).unwrap().modified().unwrap();
        assert!(store.writer.is_none());
        store.save(&config);
        assert!(store.writer.is_none(), "unchanged config must not rewrite");
        drop(store);
        let (_, loaded) = ConfigStore::load(&dir);
        let Loaded::Existing(back) = loaded else {
            panic!("expected existing config")
        };
        assert_eq!(back, config);
        assert_eq!(
            fs::metadata(dir.join(FILE)).unwrap().modified().unwrap(),
            first
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_file_is_backed_up_and_values_are_clamped() {
        let dir = temp("corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE), b"{not json").unwrap();
        let (mut store, loaded) = ConfigStore::load(&dir);
        let Loaded::Recovered(config, message) = loaded else {
            panic!("expected recovery")
        };
        assert_eq!(config, Config::default());
        assert!(message.contains(".bak"));
        assert_eq!(
            fs::read(dir.join("settings.json.bak")).unwrap(),
            b"{not json"
        );
        assert!(!dir.join(FILE).exists());
        store.save(&config);
        store.flush().unwrap();
        assert!(dir.join(FILE).exists());

        fs::write(
            dir.join(FILE),
            r#"{"video":{"view_distance":99},"controls":{"sensitivity":-3.0},"audio":{"effects":0.44}}"#,
        )
        .unwrap();
        let (_, loaded) = ConfigStore::load(&dir);
        let Loaded::Existing(config) = loaded else {
            panic!("expected existing config")
        };
        assert_eq!(config.video.view_distance, 6);
        assert_eq!(config.controls.sensitivity, 0.5);
        assert_eq!(config.audio.effects, 0.4);
        assert_eq!(config.controls.bindings, Bindings::default());
        assert!(config.advanced.all_cores);
        fs::remove_dir_all(dir).unwrap();
    }
}
