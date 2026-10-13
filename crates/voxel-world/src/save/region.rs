//! Region files: up to 32×32 modified columns per `region/r.X.Z.bin`.
//!
//! Only columns the player changed are stored; everything else is
//! regenerated from the seed. Layout:
//!
//! ```text
//! "TVREGN01"  u32 count
//! count × { u16 index, u16 reserved, u32 offset, u32 length, u32 crc32 }
//! zlib-compressed column records
//! ```
//!
//! Files are rewritten atomically. A damaged file is copied to
//! `*.corrupt` before anything else touches it; unreadable columns are
//! regenerated and the player is told.
use crate::world::{BLOCK_COUNT, Column};
use anyhow::{Context, Result, bail, ensure};
use glam::IVec2;
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

const MAGIC: &[u8; 8] = b"TVREGN01";
pub const REGION: i32 = 32;
const ENTRY: usize = 16;
/// Upper bound of an uncompressed column record.
const MAX_RAW: usize = 2 + 8 * 2 * crate::world::chunk::CELLS + 4 + 1024 * 1024;

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
}
static CRC_TABLE: [u32; 256] = crc_table();

/// IEEE CRC-32 as used by zip and PNG.
pub fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |c, &b| {
        CRC_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8)
    })
}

pub fn region_of(c: IVec2) -> IVec2 {
    c.div_euclid(IVec2::splat(REGION))
}
fn index_of(c: IVec2) -> u16 {
    let l = c.rem_euclid(IVec2::splat(REGION));
    (l.x + l.y * REGION) as u16
}
fn column_at(region: IVec2, index: u16) -> IVec2 {
    region * REGION + IVec2::new(i32::from(index) % REGION, i32::from(index) / REGION)
}
fn file_name(region: IVec2) -> String {
    format!("r.{}.{}.bin", region.x, region.y)
}

/// Compressed records of one region file.
#[derive(Default)]
struct Region {
    entries: BTreeMap<u16, Vec<u8>>,
}

/// Parsed region file; damaged entries are skipped and reported.
fn parse(bytes: &[u8]) -> Result<(Region, usize)> {
    ensure!(
        bytes.len() >= 12 && &bytes[..8] == MAGIC,
        "Not a region file"
    );
    let count = u32::from_le_bytes(bytes[8..12].try_into()?) as usize;
    ensure!(
        count <= (REGION * REGION) as usize,
        "Bad entry count {count}"
    );
    let table_end = 12 + count * ENTRY;
    ensure!(bytes.len() >= table_end, "Truncated entry table");
    let mut region = Region::default();
    let mut damaged = 0;
    for i in 0..count {
        let e = &bytes[12 + i * ENTRY..12 + (i + 1) * ENTRY];
        let index = u16::from_le_bytes([e[0], e[1]]);
        let offset = u32::from_le_bytes(e[4..8].try_into()?) as usize;
        let length = u32::from_le_bytes(e[8..12].try_into()?) as usize;
        let crc = u32::from_le_bytes(e[12..16].try_into()?);
        let valid = i32::from(index) < REGION * REGION
            && offset >= table_end
            && offset
                .checked_add(length)
                .is_some_and(|end| end <= bytes.len())
            && crc32(&bytes[offset..offset + length]) == crc;
        if valid {
            region
                .entries
                .insert(index, bytes[offset..offset + length].to_vec());
        } else {
            damaged += 1;
        }
    }
    Ok((region, damaged))
}

fn serialize(region: &Region) -> Vec<u8> {
    let table_end = 12 + region.entries.len() * ENTRY;
    let mut out =
        Vec::with_capacity(table_end + region.entries.values().map(Vec::len).sum::<usize>());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(region.entries.len() as u32).to_le_bytes());
    let mut offset = table_end;
    for (index, data) in &region.entries {
        out.extend_from_slice(&index.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&crc32(data).to_le_bytes());
        offset += data.len();
    }
    for data in region.entries.values() {
        out.extend_from_slice(data);
    }
    out
}

/// Thread-safe cache of region files of one world.
pub struct RegionStore {
    dir: Option<PathBuf>,
    regions: Mutex<HashMap<IVec2, Region>>,
    warnings: Mutex<Vec<String>>,
}

impl RegionStore {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir: Some(dir),
            regions: Mutex::new(HashMap::new()),
            warnings: Mutex::new(Vec::new()),
        }
    }
    /// A store that keeps nothing (benchmarks).
    pub fn memory() -> Self {
        Self {
            dir: None,
            regions: Mutex::new(HashMap::new()),
            warnings: Mutex::new(Vec::new()),
        }
    }
    fn lock(&self) -> MutexGuard<'_, HashMap<IVec2, Region>> {
        self.regions.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn take_warnings(&self) -> Vec<String> {
        std::mem::take(&mut *self.warnings.lock().unwrap_or_else(|e| e.into_inner()))
    }
    fn warn(&self, text: String) {
        log::error!("{text}");
        self.warnings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(text);
    }
    /// Keeps a damaged file (or a copy of it) next to the original.
    fn quarantine(&self, path: &Path, reason: &str) {
        let backup = path.with_extension("bin.corrupt");
        let result = fs::copy(path, &backup);
        self.warn(format!(
            "区域文件 {} 损坏（{reason}），已备份为 {}，受损区块将重新生成",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?"),
            match &result {
                Ok(_) => backup
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
                    .to_owned(),
                Err(e) => format!("（备份失败：{e}）"),
            }
        ));
    }
    fn ensure_loaded<'a>(
        &self,
        regions: &'a mut HashMap<IVec2, Region>,
        r: IVec2,
    ) -> &'a mut Region {
        regions.entry(r).or_insert_with(|| {
            let Some(dir) = &self.dir else {
                return Region::default();
            };
            let path = dir.join(file_name(r));
            match fs::read(&path) {
                Ok(bytes) => match parse(&bytes) {
                    Ok((region, 0)) => region,
                    Ok((region, damaged)) => {
                        self.quarantine(&path, &format!("{damaged} 个区块校验失败"));
                        region
                    }
                    Err(e) => {
                        self.quarantine(&path, &format!("{e:#}"));
                        Region::default()
                    }
                },
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Region::default(),
                Err(e) => {
                    self.warn(format!("读取区域文件 {} 失败：{e}", path.display()));
                    Region::default()
                }
            }
        })
    }
    /// The stored copy of a column, if the player ever changed it.
    pub fn load(&self, c: IVec2) -> Option<Column> {
        let compressed = {
            let mut regions = self.lock();
            self.ensure_loaded(&mut regions, region_of(c))
                .entries
                .get(&index_of(c))
                .cloned()?
        };
        let result = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&compressed, MAX_RAW)
            .map_err(|e| anyhow::anyhow!("{e:?}"))
            .and_then(|raw| Column::decode(c, &raw, BLOCK_COUNT));
        match result {
            Ok(mut column) => {
                // Stored columns are, by definition, the player's edits.
                column.changes = 1;
                column.saved_changes = 1;
                Some(column)
            }
            Err(e) => {
                if let Some(dir) = &self.dir {
                    self.quarantine(
                        &dir.join(file_name(region_of(c))),
                        &format!("区块 {c}: {e:#}"),
                    );
                }
                self.lock()
                    .get_mut(&region_of(c))
                    .map(|r| r.entries.remove(&index_of(c)));
                None
            }
        }
    }
    /// Compresses and stores raw column records, rewriting each affected
    /// region file atomically.
    pub fn store(&self, columns: &[(IVec2, Vec<u8>)]) -> Result<()> {
        let compressed: Vec<_> = columns
            .iter()
            .map(|(c, raw)| (*c, miniz_oxide::deflate::compress_to_vec_zlib(raw, 3)))
            .collect();
        let mut regions = self.lock();
        let mut touched = Vec::new();
        for (c, data) in compressed {
            let r = region_of(c);
            self.ensure_loaded(&mut regions, r)
                .entries
                .insert(index_of(c), data);
            if !touched.contains(&r) {
                touched.push(r);
            }
        }
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        fs::create_dir_all(dir).with_context(|| format!("Creating {}", dir.display()))?;
        for r in touched {
            let bytes = serialize(&regions[&r]);
            let path = dir.join(file_name(r));
            let temp = path.with_extension("tmp");
            let mut file = File::create(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temp, &path)?;
        }
        File::open(dir)?.sync_all()?;
        Ok(())
    }
}

/// Number of stored columns and a digest of their checksums across every
/// region file in `dir`; used to compare saves in device tests.
pub fn digest(dir: &Path) -> Result<(usize, u64)> {
    let mut entries = Vec::new();
    let Ok(listing) = fs::read_dir(dir) else {
        return Ok((0, 0xcbf2_9ce4_8422_2325));
    };
    for file in listing.flatten() {
        let name = file.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(coords) = name.strip_prefix("r.").and_then(|n| n.strip_suffix(".bin")) else {
            continue;
        };
        let mut parts = coords.split('.');
        let (Some(Ok(x)), Some(Ok(z))) = (
            parts.next().map(str::parse::<i32>),
            parts.next().map(str::parse::<i32>),
        ) else {
            continue;
        };
        let bytes = fs::read(file.path())?;
        let (region, damaged) = parse(&bytes)?;
        if damaged > 0 {
            bail!("{name}: {damaged} damaged entries");
        }
        for (index, data) in region.entries {
            let c = column_at(IVec2::new(x, z), index);
            entries.push((c.x, c.y, crc32(&data)));
        }
    }
    entries.sort_unstable();
    let digest = entries
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |v, (x, z, crc)| {
            [*x as u32, *z as u32, *crc]
                .iter()
                .fold(v, |v, w| (v ^ u64::from(*w)).wrapping_mul(0x0100_0000_01b3))
        });
    Ok((entries.len(), digest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("voxel-region-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }
    fn column(c: IVec2, block: u8) -> Column {
        let mut col = Column::empty(c);
        col.put(1, 0, 2, 9);
        col.put(3, 70, 4, block);
        col
    }

    #[test]
    fn crc_matches_reference() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn round_trip_across_regions_and_negative_coordinates() {
        let dir = temp("roundtrip");
        let store = RegionStore::new(dir.clone());
        let cols = [
            IVec2::new(0, 0),
            IVec2::new(-1, -1),
            IVec2::new(31, 31),
            IVec2::new(32, -33),
        ];
        let records: Vec<_> = cols
            .iter()
            .enumerate()
            .map(|(i, c)| (*c, column(*c, i as u8 + 1).encode()))
            .collect();
        store.store(&records).unwrap();
        let mut files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        assert_eq!(files, ["r.-1.-1.bin", "r.0.0.bin", "r.1.-2.bin"]);
        let reopened = RegionStore::new(dir.clone());
        for (i, c) in cols.iter().enumerate() {
            let col = reopened.load(*c).unwrap();
            assert_eq!(col.get(3, 70, 4), i as u8 + 1);
            assert!(!col.dirty());
        }
        assert!(reopened.load(IVec2::new(5, 5)).is_none(), "never stored");
        let (count, a) = digest(&dir).unwrap();
        assert_eq!(count, 4);
        // Rewriting with the same contents keeps the digest; a change alters it.
        reopened.store(&records[..1]).unwrap();
        assert_eq!(digest(&dir).unwrap().1, a);
        reopened
            .store(&[(cols[0], column(cols[0], 7).encode())])
            .unwrap();
        assert_ne!(digest(&dir).unwrap().1, a);
        assert!(reopened.take_warnings().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn damaged_files_are_backed_up_and_columns_regenerate() {
        let dir = temp("corrupt");
        let store = RegionStore::new(dir.clone());
        let a = IVec2::new(2, 3);
        let b = IVec2::new(4, 3);
        store
            .store(&[(a, column(a, 5).encode()), (b, column(b, 6).encode())])
            .unwrap();
        let path = dir.join("r.0.0.bin");
        let mut bytes = fs::read(&path).unwrap();
        let last = bytes.len() - 3;
        bytes[last] ^= 0x55;
        fs::write(&path, &bytes).unwrap();
        let reopened = RegionStore::new(dir.clone());
        // One entry fails its checksum, the other still loads.
        let loaded = [reopened.load(a).is_some(), reopened.load(b).is_some()];
        assert_eq!(loaded.iter().filter(|x| **x).count(), 1, "{loaded:?}");
        assert_eq!(fs::read(dir.join("r.0.0.bin.corrupt")).unwrap(), bytes);
        assert_eq!(reopened.take_warnings().len(), 1);
        fs::write(&path, b"garbage").unwrap();
        let reopened = RegionStore::new(dir.clone());
        assert!(reopened.load(a).is_none());
        assert_eq!(fs::read(dir.join("r.0.0.bin.corrupt")).unwrap(), b"garbage");
        assert!(reopened.take_warnings()[0].contains("损坏"));
        // A later save rewrites the file with only valid data.
        reopened.store(&[(a, column(a, 8).encode())]).unwrap();
        assert_eq!(
            RegionStore::new(dir.clone()).load(a).unwrap().get(3, 70, 4),
            8
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn memory_store_keeps_nothing_on_disk() {
        let store = RegionStore::memory();
        let c = IVec2::new(1, 1);
        store.store(&[(c, column(c, 2).encode())]).unwrap();
        assert_eq!(store.load(c).unwrap().get(3, 70, 4), 2);
    }
}
