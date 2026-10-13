//! Infinite world made of streamed 16×16×128 columns.
pub mod block;
pub mod chunk;
pub mod light;
pub mod noise;
pub mod stream;
pub mod terrain;

pub use chunk::{Column, HEIGHT};
use glam::{IVec2, IVec3, Vec3};
use std::collections::HashMap;

pub const CHUNK: i32 = chunk::SIZE;
/// Soft world border: movement and building stop at ±BORDER blocks.
pub const BORDER: i32 = 100_000;
pub use block::BEDROCK;
/// Number of block ids; stored columns must only contain ids below this.
pub const BLOCK_COUNT: u8 = block::COUNT as u8;

/// Acoustic material used for digging, placing and footstep sounds.
pub fn material(id: u8) -> crate::audio::Material {
    block::def(id)
        .material
        .unwrap_or(crate::audio::Material::Stone)
}

/// Column containing a block position.
pub fn column_of(p: IVec3) -> IVec2 {
    IVec2::new(p.x.div_euclid(CHUNK), p.z.div_euclid(CHUNK))
}
/// Column containing a world-space point.
pub fn column_at(p: Vec3) -> IVec2 {
    column_of(p.floor().as_ivec3())
}
/// Section key (column x, section y, column z) of a block position.
pub fn section_of(p: IVec3) -> IVec3 {
    p.div_euclid(IVec3::splat(CHUNK))
}
/// Whether blocks may exist at `p`: inside the height range and border.
pub fn inside(p: IVec3) -> bool {
    p.y >= 0 && p.y < HEIGHT && p.x.abs() < BORDER && p.z.abs() < BORDER
}

pub struct World {
    pub seed: u64,
    columns: HashMap<IVec2, Column>,
    next_revision: u64,
}

impl World {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            columns: HashMap::new(),
            next_revision: 1,
        }
    }
    fn bump(&mut self) -> u64 {
        self.next_revision += 1;
        self.next_revision
    }
    pub fn len(&self) -> usize {
        self.columns.len()
    }
    pub fn column(&self, c: IVec2) -> Option<&Column> {
        self.columns.get(&c)
    }
    pub fn columns(&self) -> impl Iterator<Item = &Column> {
        self.columns.values()
    }
    pub fn columns_mut(&mut self) -> impl Iterator<Item = &mut Column> {
        self.columns.values_mut()
    }
    pub fn loaded(&self, c: IVec2) -> bool {
        self.columns.contains_key(&c)
    }
    /// Whether every column within Chebyshev radius `r` of `c` is loaded.
    pub fn area_loaded(&self, c: IVec2, r: i32) -> bool {
        (-r..=r).all(|dz| (-r..=r).all(|dx| self.loaded(c + IVec2::new(dx, dz))))
    }
    /// Adds a column; every section receives a fresh mesh revision so that
    /// meshes built from an earlier copy are rejected.
    pub fn insert(&mut self, mut column: Column) {
        for s in 0..chunk::SECTIONS {
            column.revisions[s] = self.bump();
        }
        self.columns.insert(column.pos, column);
    }
    pub fn remove(&mut self, c: IVec2) -> Option<Column> {
        self.columns.remove(&c)
    }
    /// Block at `p`; air outside the height range or in unloaded columns.
    pub fn get(&self, p: IVec3) -> u8 {
        if p.y < 0 || p.y >= HEIGHT {
            return 0;
        }
        self.columns.get(&column_of(p)).map_or(0, |c| {
            c.get(p.x.rem_euclid(CHUNK), p.y, p.z.rem_euclid(CHUNK))
        })
    }
    /// Collision test: unloaded columns, the border and the void below the
    /// world are solid so nothing can fall out of loaded terrain.
    pub fn solid(&self, p: IVec3) -> bool {
        if p.y < 0 || p.x.abs() >= BORDER || p.z.abs() >= BORDER {
            return true;
        }
        if p.y >= HEIGHT {
            return false;
        }
        self.columns
            .get(&column_of(p))
            .is_none_or(|c| block::solid(c.get(p.x.rem_euclid(CHUNK), p.y, p.z.rem_euclid(CHUNK))))
    }
    /// Places or removes a block. Fails outside the world, in unloaded
    /// columns, on bedrock and when nothing changes.
    pub fn set(&mut self, p: IVec3, block: u8) -> bool {
        self.set_with_state(p, block, 0)
    }
    /// Block state at `p` (orientation, growth stage, …).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn state(&self, p: IVec3) -> u8 {
        if p.y < 0 || p.y >= HEIGHT {
            return 0;
        }
        self.columns.get(&column_of(p)).map_or(0, |c| {
            c.state(p.x.rem_euclid(CHUNK), p.y, p.z.rem_euclid(CHUNK))
        })
    }
    /// Like `set`, also storing a state byte.
    pub fn set_with_state(&mut self, p: IVec3, block: u8, state: u8) -> bool {
        if !inside(p) || block >= BLOCK_COUNT || block == BEDROCK {
            return false;
        }
        let c = column_of(p);
        let Some(column) = self.columns.get(&c) else {
            return false;
        };
        let (lx, lz) = (p.x.rem_euclid(CHUNK), p.z.rem_euclid(CHUNK));
        let old = column.get(lx, p.y, lz);
        if old == BEDROCK || (old == block && column.state(lx, p.y, lz) == state) {
            return false;
        }
        let column = self.columns.get_mut(&c).unwrap();
        column.put_with_state(lx, p.y, lz, block, state);
        column.changes += 1;
        // Meshes read a one-block halo of blocks and light, so every section
        // within one block of the edit or of a light change is rebuilt.
        let mut cells = light::block_changed(self, p);
        cells.push(p);
        for key in light::affected_sections(&cells) {
            let revision = self.bump();
            if let Some(column) = self.columns.get_mut(&IVec2::new(key.x, key.z)) {
                column.revisions[key.y as usize] = revision;
            }
        }
        true
    }
    pub fn section_revision(&self, key: IVec3) -> Option<u64> {
        let column = self.columns.get(&IVec2::new(key.x, key.z))?;
        column.revisions.get(usize::try_from(key.y).ok()?).copied()
    }
    /// (sky, block) light at `p`; `None` where light is not known yet.
    pub fn light(&self, p: IVec3) -> Option<(u8, u8)> {
        light::get(self, p).map(|v| (v >> 4, v & 15))
    }

    pub fn spawn(&self) -> Vec3 {
        terrain::spawn(self.seed)
    }
    /// Generates every missing column around `center` synchronously.
    #[cfg(test)]
    pub fn generate_area(&mut self, center: IVec2, r: i32) {
        for dz in -r..=r {
            for dx in -r..=r {
                let c = center + IVec2::new(dx, dz);
                if !self.loaded(c) {
                    self.insert(terrain::generate(self.seed, c));
                }
            }
        }
        light::Lighter::light_now(self);
    }
    /// Inserts empty columns around `center` and lights those whose
    /// neighbours exist (tests build on them).
    #[cfg(test)]
    pub fn empty_area(&mut self, center: IVec2, r: i32) {
        for dz in -r..=r {
            for dx in -r..=r {
                self.insert(Column::empty(center + IVec2::new(dx, dz)));
            }
        }
        light::Lighter::light_now(self);
    }
    pub fn raycast(&self, origin: Vec3, direction: Vec3, reach: f32) -> Option<Hit> {
        if direction.length_squared() < 0.5 || !origin.is_finite() {
            return None;
        }
        let d = direction.normalize();
        let mut p = origin.floor().as_ivec3();
        let step = d.signum().as_ivec3();
        let mut delta = Vec3::ZERO;
        let mut next = Vec3::ZERO;
        for axis in 0..3 {
            if d[axis] == 0.0 {
                delta[axis] = f32::INFINITY;
                next[axis] = f32::INFINITY;
            } else {
                delta[axis] = 1.0 / d[axis].abs();
                next[axis] =
                    ((p[axis] + i32::from(step[axis] > 0)) as f32 - origin[axis]) / d[axis];
            }
        }
        let mut distance = 0.0;
        let mut normal = IVec3::ZERO;
        for _ in 0..64 {
            if distance > reach {
                return None;
            }
            if block::targetable(self.get(p)) {
                return Some(Hit { block: p, normal });
            }
            let axis = if next.x <= next.y && next.x <= next.z {
                0
            } else if next.y <= next.z {
                1
            } else {
                2
            };
            distance = next[axis];
            next[axis] += delta[axis];
            p[axis] += step[axis];
            normal = IVec3::ZERO;
            normal[axis] = -step[axis];
        }
        None
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub block: IVec3,
    pub normal: IVec3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_column_edits_invalidate_neighbour_sections() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 1);
        // x = -1 is the last block of column -1; y = 16 starts section 1.
        let p = IVec3::new(-1, 16, 5);
        let before: Vec<_> = [
            IVec3::new(-1, 1, 0),
            IVec3::new(0, 1, 0),
            IVec3::new(-1, 0, 0),
            IVec3::new(0, 0, 0),
            IVec3::new(-1, 2, 0),
        ]
        .iter()
        .map(|k| (*k, w.section_revision(*k).unwrap()))
        .collect();
        assert!(w.set(p, 3));
        assert_eq!(w.get(p), 3);
        assert_eq!(w.column(IVec2::new(-1, 0)).unwrap().get(15, 16, 5), 3);
        for (key, revision) in &before[..4] {
            assert_ne!(w.section_revision(*key), Some(*revision), "{key}");
        }
        // Section 2 is two blocks away from y = 16 and stays valid.
        assert_eq!(w.section_revision(before[4].0), Some(before[4].1));
        assert!(w.column(IVec2::new(-1, 0)).unwrap().dirty());
        assert!(!w.column(IVec2::ZERO).unwrap().dirty());
        assert!(!w.set(p, 3), "unchanged");
        assert!(!w.set(IVec3::new(500, 20, 0), 3), "unloaded");
        assert!(!w.set(IVec3::new(0, HEIGHT, 0), 3), "above the world");
        w.column_put_for_test(IVec3::new(2, 0, 2), BEDROCK);
        assert!(!w.set(IVec3::new(2, 0, 2), 0), "bedrock");
        assert!(!w.set(IVec3::new(2, 1, 2), BEDROCK), "cannot place bedrock");
    }

    impl World {
        fn column_put_for_test(&mut self, p: IVec3, block: u8) {
            let c = self.columns.get_mut(&column_of(p)).unwrap();
            c.put(p.x.rem_euclid(16), p.y, p.z.rem_euclid(16), block);
        }
    }

    #[test]
    fn collision_treats_unloaded_and_border_as_solid() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 0);
        assert!(!w.solid(IVec3::new(3, 20, 3)));
        assert!(w.solid(IVec3::new(20, 20, 3)), "unloaded column");
        assert!(w.solid(IVec3::new(3, -1, 3)), "below the world");
        assert!(!w.solid(IVec3::new(3, HEIGHT + 4, 3)));
        assert!(w.solid(IVec3::new(BORDER, 20, 0)));
        assert!(!inside(IVec3::new(-BORDER, 20, 0)));
        assert!(inside(IVec3::new(-BORDER + 1, 0, 0)));
    }

    #[test]
    fn ray_crosses_negative_column_borders() {
        let mut w = World::new(0);
        w.empty_area(IVec2::new(-1, -1), 1);
        w.set(IVec3::new(-17, 40, -3), 3);
        let h = w
            .raycast(Vec3::new(-12.5, 40.5, -2.5), -Vec3::X, 6.0)
            .unwrap();
        assert_eq!(h.block, IVec3::new(-17, 40, -3));
        assert_eq!(h.normal, IVec3::X);
        assert!(
            w.raycast(Vec3::new(-9.5, 40.5, -2.5), -Vec3::X, 6.0)
                .is_none()
        );
        assert_eq!(column_of(IVec3::new(-1, 0, -16)), IVec2::new(-1, -1));
        assert_eq!(section_of(IVec3::new(-1, 17, 16)), IVec3::new(-1, 1, 1));
    }
}
