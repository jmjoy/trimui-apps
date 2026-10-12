use crate::{
    player::Player,
    world::{CHUNKS, COUNT, World},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::Duration,
};
#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub sensitivity: f32,
    pub volume: f32,
    pub view_distance: i32,
    pub selected: u8,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            sensitivity: 1.0,
            volume: 0.6,
            view_distance: 3,
            selected: 1,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.sensitivity.is_finite() && (0.5..=2.0).contains(&self.sensitivity),
            "Invalid sensitivity"
        );
        ensure!(
            self.volume.is_finite() && (0.0..=1.0).contains(&self.volume),
            "Invalid volume"
        );
        ensure!(
            (2..=4).contains(&self.view_distance) && (1..=8).contains(&self.selected),
            "Invalid settings"
        );
        Ok(())
    }
}
#[derive(Serialize, Deserialize)]
struct Metadata {
    version: u32,
    seed: u64,
    player: Player,
    settings: Settings,
}
pub struct Snapshot {
    pub world: World,
    pub player: Player,
    pub settings: Settings,
}
const MAGIC: &[u8; 8] = b"TVWORLD1";
impl Snapshot {
    pub fn load(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join("world.dat");
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            file.metadata()?.len() < 8 * 1024 * 1024,
            "Save is too large"
        );
        let mut bytes = Vec::new();
        file.take(8 * 1024 * 1024).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() >= 12 && &bytes[..8] == MAGIC,
            "Unknown save format"
        );
        let length = u32::from_le_bytes(bytes[8..12].try_into()?) as usize;
        ensure!(
            length <= 65536 && 12 + length < bytes.len(),
            "Invalid save header"
        );
        let meta: Metadata = serde_json::from_slice(&bytes[12..12 + length])?;
        ensure!(meta.version == 1, "Unsupported save version");
        meta.settings.validate()?;
        let blocks =
            miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&bytes[12 + length..], COUNT)
                .map_err(|e| anyhow::anyhow!("Corrupt world: {e:?}"))?;
        ensure!(
            blocks.len() == COUNT && blocks.iter().all(|b| *b <= 9),
            "Invalid blocks"
        );
        let world = World {
            seed: meta.seed,
            blocks,
            revisions: vec![1; CHUNKS],
        };
        let mut player = meta.player;
        player.restore_if_invalid(&world);
        log::info!(
            "World loaded: seed={}, position={:?}, selected={}, digest={:016x}",
            world.seed,
            player.position,
            meta.settings.selected,
            digest(&world.blocks)
        );
        Ok(Some(Self {
            world,
            player,
            settings: meta.settings,
        }))
    }
    pub fn write(&self, dir: &Path) -> Result<()> {
        fs::create_dir_all(dir)?;
        self.settings.validate()?;
        let metadata = serde_json::to_vec(&Metadata {
            version: 1,
            seed: self.world.seed,
            player: self.player.clone(),
            settings: self.settings.clone(),
        })?;
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&self.world.blocks, 3);
        let temp = dir.join("world.tmp");
        let mut file = File::create(&temp)?;
        file.write_all(MAGIC)?;
        file.write_all(&(metadata.len() as u32).to_le_bytes())?;
        file.write_all(&metadata)?;
        file.write_all(&compressed)?;
        file.sync_all()?;
        fs::rename(&temp, dir.join("world.dat"))?;
        File::open(dir)?.sync_all()?;
        log::info!(
            "World saved: seed={}, position={:?}, selected={}, digest={:016x}",
            self.world.seed,
            self.player.position,
            self.settings.selected,
            digest(&self.world.blocks)
        );
        Ok(())
    }
}
pub fn digest(blocks: &[u8]) -> u64 {
    blocks.iter().fold(0xcbf29ce484222325, |v, b| {
        (v ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}
pub struct SaveWorker {
    sender: Option<Sender<Snapshot>>,
    receiver: Receiver<std::result::Result<(), String>>,
    thread: Option<JoinHandle<()>>,
    pub busy: bool,
}
impl SaveWorker {
    pub fn new(dir: PathBuf) -> Self {
        let (sender, jobs) = mpsc::channel::<Snapshot>();
        let (results, receiver) = mpsc::channel();
        let thread = thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                let result = job.write(&dir).map_err(|e| format!("{e:#}"));
                if results.send(result).is_err() {
                    break;
                }
            }
        });
        Self {
            sender: Some(sender),
            receiver,
            thread: Some(thread),
            busy: false,
        }
    }
    pub fn request(&mut self, snapshot: Snapshot) -> Result<()> {
        ensure!(!self.busy, "Save already in progress");
        self.sender
            .as_ref()
            .context("Save worker stopped")?
            .send(snapshot)?;
        self.busy = true;
        Ok(())
    }
    pub fn poll(&mut self) -> Option<std::result::Result<(), String>> {
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
    pub fn finish(&mut self) -> Result<()> {
        if self.busy {
            let result = self
                .receiver
                .recv_timeout(Duration::from_secs(5))
                .context("Waiting for save")?;
            self.busy = false;
            result.map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }
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
    #[test]
    fn roundtrip_corruption_and_failed_write_keep_old_save() {
        let dir = std::env::temp_dir().join(format!("trimui-voxel-save-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let world = World::generate(73);
        let snapshot = Snapshot {
            player: Player::new(&world),
            world,
            settings: Settings::default(),
        };
        snapshot.write(&dir).unwrap();
        let loaded = Snapshot::load(&dir).unwrap().unwrap();
        assert_eq!(snapshot.world.blocks, loaded.world.blocks);
        assert_eq!(snapshot.player.position, loaded.player.position);
        let old = fs::read(dir.join("world.dat")).unwrap();
        fs::create_dir(dir.join("world.tmp")).unwrap();
        assert!(snapshot.write(&dir).is_err());
        assert_eq!(old, fs::read(dir.join("world.dat")).unwrap());
        fs::remove_dir(dir.join("world.tmp")).unwrap();
        let mut corrupt = old;
        let end = corrupt.len() - 1;
        corrupt[end] ^= 55;
        fs::write(dir.join("world.dat"), &corrupt).unwrap();
        assert!(Snapshot::load(&dir).is_err());
        assert_eq!(corrupt, fs::read(dir.join("world.dat")).unwrap());
        fs::remove_dir_all(dir).unwrap();
    }
}
