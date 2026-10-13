//! World saves: `<save>/worlds/<slot>/level.json` plus region files.
//!
//! The game thread copies the raw records of modified columns and hands
//! them to a background thread that compresses and writes them atomically.
//! Columns stay loaded until their save is acknowledged.
pub mod region;

use crate::player::Player;
use anyhow::{Context, Result, ensure};
use glam::IVec2;
use region::RegionStore;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub const LEVEL_VERSION: u32 = 2;
/// Version of the block id table stored columns refer to. Ids are only
/// appended, so older tables remain readable.
pub const BLOCK_TABLE: u32 = 2;
pub const HOTBAR: usize = 9;

/// Creative hotbar of a new world.
pub fn default_hotbar() -> [u8; HOTBAR] {
    use crate::world::block::*;
    [
        GRASS,
        DIRT,
        STONE,
        COBBLESTONE,
        OAK_PLANKS,
        OAK_LOG,
        BRICKS,
        SAND,
        GLOWSTONE,
    ]
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Level {
    pub version: u32,
    pub name: String,
    pub seed: u64,
    pub block_table: u32,
    pub player: Player,
    #[serde(default = "default_hotbar")]
    pub hotbar: [u8; HOTBAR],
    #[serde(default)]
    pub slot: u8,
    /// World time in ticks (24 000 per day).
    #[serde(default = "default_time")]
    pub time: f64,
}
/// New worlds start in the morning.
pub fn default_time() -> f64 {
    1_000.0
}
impl Level {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.version == LEVEL_VERSION,
            "Unsupported level version {}",
            self.version
        );
        ensure!(
            (1..=BLOCK_TABLE).contains(&self.block_table),
            "Unknown block table {}",
            self.block_table
        );
        ensure!(usize::from(self.slot) < HOTBAR, "Invalid hotbar slot");
        ensure!(
            self.time.is_finite() && self.time >= 0.0,
            "Invalid world time"
        );
        ensure!(
            self.hotbar.iter().all(|&b| b < crate::world::BLOCK_COUNT),
            "Invalid hotbar block"
        );
        Ok(())
    }
}

/// Paths of one world.
#[derive(Clone)]
pub struct WorldDir {
    pub root: PathBuf,
}
impl WorldDir {
    pub fn new(save_dir: &Path, slot: usize) -> Self {
        Self {
            root: save_dir.join("worlds").join(slot.to_string()),
        }
    }
    pub fn level(&self) -> PathBuf {
        self.root.join("level.json")
    }
    pub fn regions(&self) -> PathBuf {
        self.root.join("region")
    }
    pub fn exists(&self) -> bool {
        self.level().exists()
    }
    pub fn read_level(&self) -> Result<Option<Level>> {
        let file = match File::open(self.level()) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut text = String::new();
        file.take(1024 * 1024).read_to_string(&mut text)?;
        let mut level: Level = serde_json::from_str(&text).context("Parsing level.json")?;
        level.validate()?;
        level.player.restore_if_invalid(level.seed);
        Ok(Some(level))
    }
    pub fn write_level(&self, level: &Level) -> Result<()> {
        level.validate()?;
        fs::create_dir_all(&self.root)?;
        let temp = self.root.join("level.json.tmp");
        let mut file = File::create(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(level)?)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, self.level())?;
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
    /// Moves this world aside as `<slot>.bak`, replacing an older backup.
    pub fn retire(&self) -> Result<()> {
        if !self.root.exists() {
            return Ok(());
        }
        let backup = self.root.with_extension("bak");
        if backup.exists() {
            fs::remove_dir_all(&backup)?;
        }
        fs::rename(&self.root, &backup)?;
        Ok(())
    }
}

/// Renames the single-file save of earlier versions so it is kept but not
/// loaded; returns whether a file was renamed.
pub fn retire_legacy(save_dir: &Path) -> Result<bool> {
    let old = save_dir.join("world.dat");
    if !old.is_file() {
        return Ok(false);
    }
    let mut target = save_dir.join("world.v1.dat");
    let mut n = 1;
    while target.exists() {
        n += 1;
        target = save_dir.join(format!("world.v1-{n}.dat"));
    }
    fs::rename(&old, &target)?;
    log::warn!("Legacy save kept as {}", target.display());
    Ok(true)
}

pub struct SaveJob {
    pub level: Level,
    /// Column position, its change counter when copied, raw record.
    pub columns: Vec<(IVec2, u64, Vec<u8>)>,
}

#[derive(Debug)]
pub struct Saved {
    pub columns: Vec<(IVec2, u64)>,
    pub stored: usize,
    pub digest: u64,
}

pub struct SaveWorker {
    sender: Option<Sender<SaveJob>>,
    receiver: Receiver<std::result::Result<Saved, String>>,
    thread: Option<JoinHandle<()>>,
    pub busy: bool,
}

impl SaveWorker {
    /// `dir` is `None` for sessions that never write (benchmarks).
    pub fn new(dir: Option<WorldDir>, store: Arc<RegionStore>) -> Self {
        let (sender, jobs) = mpsc::channel::<SaveJob>();
        let (results, receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("voxel-save".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    let result = write(dir.as_ref(), &store, &job).map_err(|e| format!("{e:#}"));
                    if results.send(result).is_err() {
                        break;
                    }
                }
            })
            .expect("spawning save thread");
        Self {
            sender: Some(sender),
            receiver,
            thread: Some(thread),
            busy: false,
        }
    }
    pub fn request(&mut self, job: SaveJob) -> Result<()> {
        ensure!(!self.busy, "Save already in progress");
        self.sender
            .as_ref()
            .context("Save worker stopped")?
            .send(job)?;
        self.busy = true;
        Ok(())
    }
    pub fn poll(&mut self) -> Option<std::result::Result<Saved, String>> {
        if !self.busy {
            return None;
        }
        match self.receiver.try_recv() {
            Ok(r) => {
                self.busy = false;
                Some(r)
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(_) => {
                self.busy = false;
                Some(Err("Save worker disconnected".into()))
            }
        }
    }
    /// Waits for the save in progress, if any.
    pub fn wait(&mut self) -> Option<std::result::Result<Saved, String>> {
        if !self.busy {
            return None;
        }
        self.busy = false;
        Some(
            self.receiver
                .recv_timeout(Duration::from_secs(15))
                .unwrap_or_else(|e| Err(format!("Waiting for save: {e}"))),
        )
    }
}

fn write(dir: Option<&WorldDir>, store: &RegionStore, job: &SaveJob) -> Result<Saved> {
    let records: Vec<_> = job
        .columns
        .iter()
        .map(|(c, _, raw)| (*c, raw.clone()))
        .collect();
    store.store(&records)?;
    let (stored, digest) = match dir {
        Some(dir) => {
            dir.write_level(&job.level)?;
            region::digest(&dir.regions())?
        }
        None => (0, 0),
    };
    log::info!(
        "World saved: seed={}, position={:?}, slot={}, changed={}, columns={stored}, digest={digest:016x}",
        job.level.seed,
        job.level.player.position,
        job.level.slot,
        job.columns.len()
    );
    Ok(Saved {
        columns: job.columns.iter().map(|(c, n, _)| (*c, *n)).collect(),
        stored,
        digest,
    })
}

impl Drop for SaveWorker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Column, World};

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("voxel-save-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }
    fn level(seed: u64) -> Level {
        Level {
            version: LEVEL_VERSION,
            name: "测试".into(),
            seed,
            block_table: BLOCK_TABLE,
            player: Player::new(&World::new(seed)),
            hotbar: default_hotbar(),
            slot: 3,
            time: 7_000.5,
        }
    }

    #[test]
    fn level_round_trip_and_validation() {
        let dir = WorldDir::new(&temp("level"), 0);
        assert!(dir.read_level().unwrap().is_none());
        dir.write_level(&level(5)).unwrap();
        let back = dir.read_level().unwrap().unwrap();
        assert_eq!((back.seed, back.slot, back.time), (5, 3, 7_000.5));
        assert_eq!(back.hotbar, default_hotbar());
        let mut bad = level(5);
        bad.slot = 9;
        assert!(dir.write_level(&bad).is_err());
        let mut bad = level(5);
        bad.hotbar[0] = 250;
        assert!(dir.write_level(&bad).is_err());
        // Levels of the first infinite-world build (block table 1, no hotbar).
        let mut old = serde_json::to_value(level(6)).unwrap();
        old["block_table"] = 1.into();
        old.as_object_mut().unwrap().remove("hotbar");
        old.as_object_mut().unwrap().remove("slot");
        old.as_object_mut().unwrap().remove("time");
        old["selected"] = 2.into();
        std::fs::write(dir.level(), serde_json::to_vec(&old).unwrap()).unwrap();
        let back = dir.read_level().unwrap().unwrap();
        assert_eq!(
            (back.seed, back.slot, back.hotbar),
            (6, 0, default_hotbar())
        );
        assert_eq!(back.time, default_time());
        fs::write(dir.level(), b"{broken").unwrap();
        assert!(dir.read_level().is_err());
        assert_eq!(
            fs::read(dir.level()).unwrap(),
            b"{broken",
            "never rewritten"
        );
        dir.retire().unwrap();
        assert!(!dir.root.exists());
        assert!(dir.root.with_extension("bak").join("level.json").exists());
        fs::remove_dir_all(dir.root.parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn legacy_save_is_renamed_not_loaded() {
        let root = temp("legacy");
        fs::create_dir_all(&root).unwrap();
        assert!(!retire_legacy(&root).unwrap());
        fs::write(root.join("world.dat"), b"old").unwrap();
        assert!(retire_legacy(&root).unwrap());
        assert_eq!(fs::read(root.join("world.v1.dat")).unwrap(), b"old");
        fs::write(root.join("world.dat"), b"older").unwrap();
        assert!(retire_legacy(&root).unwrap());
        assert_eq!(fs::read(root.join("world.v1-2.dat")).unwrap(), b"older");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_writes_regions_and_level_and_reports_digest() {
        let dir = WorldDir::new(&temp("worker"), 0);
        let store = Arc::new(RegionStore::new(dir.regions()));
        let mut worker = SaveWorker::new(Some(dir.clone()), store);
        let mut column = Column::empty(IVec2::new(-4, 9));
        column.put(0, 5, 0, 3);
        worker
            .request(SaveJob {
                level: level(7),
                columns: vec![(column.pos, 4, column.encode())],
            })
            .unwrap();
        assert!(
            worker
                .request(SaveJob {
                    level: level(7),
                    columns: Vec::new()
                })
                .is_err()
        );
        let saved = worker.wait().unwrap().unwrap();
        assert_eq!(saved.columns, [(IVec2::new(-4, 9), 4)]);
        assert_eq!(saved.stored, 1);
        assert_eq!(region::digest(&dir.regions()).unwrap(), (1, saved.digest));
        assert_eq!(
            RegionStore::new(dir.regions())
                .load(column.pos)
                .unwrap()
                .get(0, 5, 0),
            3
        );
        assert_eq!(dir.read_level().unwrap().unwrap().seed, 7);
        drop(worker);
        fs::remove_dir_all(dir.root.parent().unwrap().parent().unwrap()).unwrap();
    }
}
