//! Column storage: a 16×16×128 column split into eight 16³ sections.
//!
//! All-air sections are not allocated. Every cell stores a block id and a
//! state byte (orientation, fluid level, growth stage…). Light is kept per
//! column (sky light in the high nibble, block light in the low nibble),
//! because air above the terrain needs light too.
//!
//! Sections and light are reference counted: background jobs take cheap
//! snapshots and edits copy only data that a job still holds.
use anyhow::{Result, bail, ensure};
use glam::IVec2;
use std::sync::Arc;

pub const SIZE: i32 = 16;
pub const HEIGHT: i32 = 128;
pub const SECTIONS: usize = (HEIGHT / SIZE) as usize;
pub const CELLS: usize = (SIZE * SIZE * SIZE) as usize;
pub const COLUMN_CELLS: usize = CELLS * SECTIONS;
/// Light of air above the world and of unlit columns' sky.
pub const FULL_SKY: u8 = 0xF0;

/// Index of a cell in the column light array; `y` in 0..HEIGHT.
#[inline]
pub fn light_index(x: i32, y: i32, z: i32) -> usize {
    (x | (z << 4) | (y << 8)) as usize
}
const FORMAT: u8 = 1;

/// Index inside a section from local coordinates (each 0..16).
#[inline]
pub fn local_index(x: i32, y: i32, z: i32) -> usize {
    (x | (z << 4) | (y << 8)) as usize
}

#[derive(Clone)]
pub struct Section {
    pub blocks: [u8; CELLS],
    pub state: [u8; CELLS],
    count: u16,
}
impl Section {
    fn shared() -> Arc<Self> {
        Arc::new(Self {
            blocks: [0; CELLS],
            state: [0; CELLS],
            count: 0,
        })
    }
    /// Number of non-air cells.
    pub fn count(&self) -> usize {
        usize::from(self.count)
    }
}

#[derive(Clone)]
pub struct Column {
    pub pos: IVec2,
    sections: [Option<Arc<Section>>; SECTIONS],
    light: Arc<Vec<u8>>,
    /// Light has been computed; until then `light` holds no meaning.
    pub lit: bool,
    /// Mesh revision per section; assigned by the world when inserted.
    pub revisions: [u64; SECTIONS],
    /// Edits since creation; compared with `saved_changes` to find unsaved data.
    pub changes: u64,
    pub saved_changes: u64,
}

impl Column {
    pub fn empty(pos: IVec2) -> Self {
        Self {
            pos,
            sections: Default::default(),
            light: Arc::new(vec![0; COLUMN_CELLS]),
            lit: false,
            revisions: [0; SECTIONS],
            changes: 0,
            saved_changes: 0,
        }
    }
    /// Block at local coordinates; `y` in 0..HEIGHT.
    #[inline]
    pub fn get(&self, x: i32, y: i32, z: i32) -> u8 {
        match &self.sections[(y >> 4) as usize] {
            Some(s) => s.blocks[local_index(x, y & 15, z)],
            None => 0,
        }
    }
    #[inline]
    pub fn state(&self, x: i32, y: i32, z: i32) -> u8 {
        match &self.sections[(y >> 4) as usize] {
            Some(s) => s.state[local_index(x, y & 15, z)],
            None => 0,
        }
    }
    /// Writes a block (and resets its state) without any bookkeeping.
    pub fn put(&mut self, x: i32, y: i32, z: i32, block: u8) {
        self.put_with_state(x, y, z, block, 0);
    }
    pub fn put_with_state(&mut self, x: i32, y: i32, z: i32, block: u8, state: u8) {
        let s = (y >> 4) as usize;
        if block == 0 && self.sections[s].is_none() {
            return;
        }
        let section = Arc::make_mut(self.sections[s].get_or_insert_with(Section::shared));
        let i = local_index(x, y & 15, z);
        let old = section.blocks[i];
        match (old == 0, block == 0) {
            (true, false) => section.count += 1,
            (false, true) => section.count -= 1,
            _ => {}
        }
        section.blocks[i] = block;
        section.state[i] = if block == 0 { 0 } else { state };
        if section.count == 0 {
            self.sections[s] = None;
        }
    }
    pub fn section(&self, s: usize) -> Option<&Section> {
        self.sections[s].as_deref()
    }
    /// Shared handle for background snapshots.
    pub fn section_arc(&self, s: usize) -> Option<Arc<Section>> {
        self.sections.get(s)?.clone()
    }
    pub fn light_arc(&self) -> Arc<Vec<u8>> {
        self.light.clone()
    }
    #[inline]
    pub fn light(&self, x: i32, y: i32, z: i32) -> u8 {
        self.light[light_index(x, y, z)]
    }
    /// Writes one light value; returns whether it changed.
    #[inline]
    pub fn set_light(&mut self, x: i32, y: i32, z: i32, value: u8) -> bool {
        let i = light_index(x, y, z);
        if self.light[i] == value {
            return false;
        }
        Arc::make_mut(&mut self.light)[i] = value;
        true
    }
    pub fn replace_light(&mut self, light: Vec<u8>) {
        debug_assert_eq!(light.len(), COLUMN_CELLS);
        self.light = Arc::new(light);
        self.lit = true;
    }
    pub fn is_empty(&self, s: usize) -> bool {
        self.sections[s].is_none()
    }
    pub fn dirty(&self) -> bool {
        self.changes != self.saved_changes
    }
    /// Highest non-air block at local (x, z).
    #[cfg(test)]
    pub fn top(&self, x: i32, z: i32) -> Option<i32> {
        (0..HEIGHT).rev().find(|&y| self.get(x, y, z) != 0)
    }
    /// Allocated sections, a memory estimate for diagnostics.
    pub fn allocated(&self) -> usize {
        self.sections.iter().filter(|s| s.is_some()).count()
    }
    /// Uncompressed save representation: format, section mask, then blocks
    /// and states of every present section, then a length-prefixed extension
    /// area for block entities and other later additions.
    pub fn encode(&self) -> Vec<u8> {
        let mask = self
            .sections
            .iter()
            .enumerate()
            .filter(|(_, s)| s.is_some())
            .fold(0u8, |m, (i, _)| m | 1 << i);
        let mut out = Vec::with_capacity(6 + mask.count_ones() as usize * CELLS * 2);
        out.push(FORMAT);
        out.push(mask);
        for section in self.sections.iter().flatten() {
            out.extend_from_slice(&section.blocks);
            out.extend_from_slice(&section.state);
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out
    }
    /// Parses `encode` output; every block id must be below `block_count`.
    pub fn decode(pos: IVec2, bytes: &[u8], block_count: u8) -> Result<Self> {
        ensure!(bytes.len() >= 6, "Column record too short");
        ensure!(bytes[0] == FORMAT, "Unknown column format {}", bytes[0]);
        let mask = bytes[1];
        let body = 2 + mask.count_ones() as usize * CELLS * 2;
        ensure!(bytes.len() >= body + 4, "Truncated column record");
        let extra = u32::from_le_bytes(bytes[body..body + 4].try_into()?) as usize;
        if bytes.len() != body + 4 + extra {
            bail!("Column record length mismatch");
        }
        let mut column = Self::empty(pos);
        let mut offset = 2;
        for s in 0..SECTIONS {
            if mask & (1 << s) == 0 {
                continue;
            }
            let mut section = Section::shared();
            let section_mut = Arc::make_mut(&mut section);
            section_mut
                .blocks
                .copy_from_slice(&bytes[offset..offset + CELLS]);
            section_mut
                .state
                .copy_from_slice(&bytes[offset + CELLS..offset + 2 * CELLS]);
            offset += 2 * CELLS;
            ensure!(
                section_mut.blocks.iter().all(|&b| b < block_count),
                "Invalid block id in column {pos}"
            );
            section_mut.count = section_mut.blocks.iter().filter(|&&b| b != 0).count() as u16;
            if section.count > 0 {
                column.sections[s] = Some(section);
            }
        }
        Ok(column)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sections_allocate_lazily_and_round_trip() {
        let mut c = Column::empty(IVec2::new(-3, 7));
        assert_eq!(c.allocated(), 0);
        c.put(0, 0, 0, 9);
        c.put(15, 127, 15, 3);
        c.put(4, 40, 9, 5);
        assert_eq!(c.allocated(), 3);
        assert_eq!(c.get(15, 127, 15), 3);
        assert_eq!(c.top(4, 9), Some(40));
        c.put(4, 40, 9, 0);
        assert_eq!(c.allocated(), 2, "emptied section is freed");
        let bytes = c.encode();
        let back = Column::decode(c.pos, &bytes, 10).unwrap();
        assert_eq!(back.get(0, 0, 0), 9);
        assert_eq!(back.get(15, 127, 15), 3);
        assert_eq!(back.allocated(), 2);
        assert_eq!(back.encode(), bytes);
        assert!(
            Column::decode(c.pos, &bytes, 5).is_err(),
            "unknown block id"
        );
        assert!(Column::decode(c.pos, &bytes[..bytes.len() - 1], 10).is_err());
        let mut wrong = bytes.clone();
        wrong[0] = 99;
        assert!(Column::decode(c.pos, &wrong, 10).is_err());
    }
}
