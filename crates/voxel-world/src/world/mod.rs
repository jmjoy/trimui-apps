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
            let id = self.get(p);
            if block::targetable(id) {
                if block::is_cube(id) {
                    return Some(Hit {
                        block: p,
                        normal,
                        min: Vec3::ZERO,
                        max: Vec3::ONE,
                    });
                }
                // Thin models are only hit where their box is.
                let (lo, hi) = block::bounds(id, self.state(p));
                let (min, max) = (
                    IVec3::from(lo).as_vec3() / 16.0,
                    IVec3::from(hi).as_vec3() / 16.0,
                );
                if let Some((t, n)) = ray_box(origin - p.as_vec3(), d, min, max)
                    && t <= reach
                {
                    return Some(Hit {
                        block: p,
                        normal: if n == IVec3::ZERO { normal } else { n },
                        min,
                        max,
                    });
                }
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
    /// Face the ray entered through; zero when it started inside.
    pub normal: IVec3,
    /// Box of the block's model inside its cell (0–1).
    pub min: Vec3,
    pub max: Vec3,
}

/// Entry distance and face normal of a ray hitting a box; (0, zero) when
/// the origin is inside.
fn ray_box(origin: Vec3, d: Vec3, min: Vec3, max: Vec3) -> Option<(f32, IVec3)> {
    let mut near = f32::NEG_INFINITY;
    let mut far = f32::INFINITY;
    let mut normal = IVec3::ZERO;
    for axis in 0..3 {
        if d[axis] == 0.0 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
            continue;
        }
        let a = (min[axis] - origin[axis]) / d[axis];
        let b = (max[axis] - origin[axis]) / d[axis];
        let (t0, t1) = if a < b { (a, b) } else { (b, a) };
        if t0 > near {
            near = t0;
            normal = IVec3::ZERO;
            normal[axis] = if d[axis] > 0.0 { -1 } else { 1 };
        }
        far = far.min(t1);
    }
    if far < near.max(0.0) {
        return None;
    }
    Some(if near < 0.0 {
        (0.0, IVec3::ZERO)
    } else {
        (near, normal)
    })
}

/// Placement rules of blocks that need something to stand on or hang from.
impl World {
    /// A full cube that plants, torches and ladders can rest on.
    fn sturdy(&self, p: IVec3) -> bool {
        let id = self.get(p);
        block::is_cube(id) && block::solid(id)
    }
    /// Whether `id` in `state` can stay at `p`.
    pub fn supported(&self, p: IVec3, id: u8, state: u8) -> bool {
        use block::*;
        let below = self.get(p - IVec3::Y);
        match def(id).render {
            Render::Cross if id == SUGAR_CANE => {
                matches!(below, GRASS | DIRT | SNOWY_GRASS | SAND | SUGAR_CANE)
            }
            Render::Cross => matches!(below, GRASS | DIRT | SNOWY_GRASS | FARMLAND),
            Render::Cactus => matches!(below, SAND | CACTUS),
            Render::Torch => match torch_pointing(state) {
                None => self.sturdy(p - IVec3::Y),
                Some(f) => self.sturdy(p - facing_face(f).normal()),
            },
            Render::Ladder => self.sturdy(p - facing_face(state).normal()),
            Render::Door if state & DOOR_UPPER != 0 => below == DOOR,
            Render::Door => self.sturdy(p - IVec3::Y) && self.get(p + IVec3::Y) == DOOR,
            Render::Layer => self.sturdy(p - IVec3::Y),
            Render::Crop => below == FARMLAND,
            _ => true,
        }
    }
    /// Removes blocks around `p` that lost their support, repeatedly (a
    /// stack of cactus falls apart); returns what was removed.
    pub fn drop_unsupported(&mut self, p: IVec3) -> Vec<(IVec3, u8)> {
        let mut removed = Vec::new();
        let mut check = vec![p];
        while let Some(c) = check.pop() {
            if removed.len() >= 256 {
                break;
            }
            for d in [
                IVec3::Y,
                IVec3::NEG_Y,
                IVec3::X,
                IVec3::NEG_X,
                IVec3::Z,
                IVec3::NEG_Z,
            ] {
                let q = c + d;
                let id = self.get(q);
                if id == 0 || block::is_cube(id) || block::liquid(id) {
                    continue;
                }
                if !self.supported(q, id, self.state(q)) && self.set(q, 0) {
                    removed.push((q, id));
                    check.push(q);
                }
            }
        }
        removed
    }
    /// Breaks the block at `p` with everything that depended on it.
    pub fn break_block(&mut self, p: IVec3) -> Option<Vec<(IVec3, u8)>> {
        let id = self.get(p);
        let state = self.state(p);
        if !self.set(p, 0) {
            return None;
        }
        let mut removed = vec![(p, id)];
        if id == block::DOOR {
            let other = if state & block::DOOR_UPPER != 0 {
                p - IVec3::Y
            } else {
                p + IVec3::Y
            };
            if self.get(other) == block::DOOR && self.set(other, 0) {
                removed.push((other, id));
            }
        }
        let cells: Vec<_> = removed.iter().map(|(c, _)| *c).collect();
        for c in cells {
            removed.extend(self.drop_unsupported(c));
        }
        Some(removed)
    }
    /// Where and in which state `id` goes when placed against `hit` by a
    /// player looking along `yaw`.
    pub fn placement(&self, hit: &Hit, id: u8, yaw: f32) -> Result<(IVec3, u8), &'static str> {
        use block::*;
        let refuse = "此处无法放置方块";
        let hit_id = self.get(hit.block);
        let target = if replaceable(hit_id) {
            hit.block
        } else if hit.normal == IVec3::ZERO {
            return Err(refuse);
        } else {
            hit.block + hit.normal
        };
        if !inside(target) || !replaceable(self.get(target)) {
            return Err(refuse);
        }
        let face = Face::from_normal(hit.normal);
        let state = match def(id).render {
            Render::Torch => match face {
                Some(Face::Up) => 0,
                Some(f) if facing_index(f).is_some() => torch_state(f).unwrap_or(0),
                _ => return Err("火把只能放在方块的顶面或侧面"),
            },
            Render::Ladder => match face.and_then(facing_index) {
                Some(i) => i,
                None => return Err("梯子只能挂在方块侧面"),
            },
            Render::Door => {
                let up = target + IVec3::Y;
                if !inside(up) || !replaceable(self.get(up)) {
                    return Err("门需要两格高的空间");
                }
                if !self.sturdy(target - IVec3::Y) {
                    return Err("门需要放在完整方块上");
                }
                return Ok((target, player_facing(yaw)));
            }
            _ if matches!(def(id).faces, Faces::Front(..)) => facing_towards_player(yaw),
            _ => 0,
        };
        if !self.supported(target, id, state) {
            return Err(match def(id).render {
                Render::Cross if id == SUGAR_CANE => "甘蔗需要种在草地、泥土或沙子上",
                Render::Cross => "植物需要种在草地或泥土上",
                Render::Cactus => "仙人掌需要种在沙子上",
                _ => "需要贴着完整的方块放置",
            });
        }
        Ok((target, state))
    }
    /// Places a block computed by `placement` (doors take two cells).
    pub fn place_block(&mut self, p: IVec3, id: u8, state: u8) -> bool {
        if id == block::DOOR {
            let lower = state & 3;
            return self.set_with_state(p + IVec3::Y, id, lower | block::DOOR_UPPER)
                && self.set_with_state(p, id, lower);
        }
        self.set_with_state(p, id, state)
    }
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

    #[test]
    fn rays_hit_model_boxes_only() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 1);
        w.set(IVec3::new(4, 39, 4), block::STONE);
        w.set(IVec3::new(4, 40, 4), block::TORCH);
        // Through the empty corner of the torch cell onto the stone below.
        let h = w
            .raycast(Vec3::new(4.1, 41.5, 4.1), Vec3::new(0.0, -1.0, 0.0), 6.0)
            .unwrap();
        assert_eq!(h.block, IVec3::new(4, 39, 4));
        // Straight at the torch stick.
        let h = w.raycast(Vec3::new(4.5, 40.3, 2.0), Vec3::Z, 6.0).unwrap();
        assert_eq!(h.block, IVec3::new(4, 40, 4));
        assert_eq!(h.normal, IVec3::NEG_Z);
        assert!((h.max.y - 10.0 / 16.0).abs() < 1e-6);
        assert_eq!(
            ray_box(Vec3::splat(0.5), Vec3::X, Vec3::ZERO, Vec3::ONE),
            Some((0.0, IVec3::ZERO))
        );
    }

    #[test]
    fn placement_rules_support_and_cascading_breaks() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 1);
        for x in 0..8 {
            w.set(IVec3::new(x, 39, 4), block::GRASS);
        }
        w.set(IVec3::new(4, 40, 5), block::STONE);
        let ground = |x: i32| Hit {
            block: IVec3::new(x, 39, 4),
            normal: IVec3::Y,
            min: Vec3::ZERO,
            max: Vec3::ONE,
        };
        // Plants need soil, cactus sand; torches and ladders a wall.
        assert_eq!(
            w.placement(&ground(1), block::POPPY, 0.0),
            Ok((IVec3::new(1, 40, 4), 0))
        );
        assert!(w.placement(&ground(1), block::CACTUS, 0.0).is_err());
        let wall = Hit {
            block: IVec3::new(4, 40, 5),
            normal: IVec3::NEG_Z,
            min: Vec3::ZERO,
            max: Vec3::ONE,
        };
        let (at, state) = w.placement(&wall, block::TORCH, 0.0).unwrap();
        assert_eq!(
            (at, block::torch_pointing(state)),
            (IVec3::new(4, 40, 4), Some(0))
        );
        let (_, ladder) = w.placement(&wall, block::LADDER, 0.0).unwrap();
        assert_eq!(block::facing_face(ladder), block::Face::North);
        let down = Hit {
            normal: IVec3::NEG_Y,
            ..wall
        };
        assert!(
            w.placement(&down, block::TORCH, 0.0).is_err(),
            "no ceiling torches"
        );
        assert!(w.place_block(at, block::TORCH, state));
        // Tall grass is replaced by what is placed on it.
        w.set(IVec3::new(2, 40, 4), block::TALL_GRASS);
        let grass = Hit {
            block: IVec3::new(2, 40, 4),
            normal: IVec3::Y,
            ..wall
        };
        assert_eq!(
            w.placement(&grass, block::STONE, 0.0).unwrap().0,
            IVec3::new(2, 40, 4)
        );
        // Doors take two cells and break together.
        let (door, facing) = w.placement(&ground(6), block::DOOR, 0.0).unwrap();
        assert!(w.place_block(door, block::DOOR, facing));
        assert_eq!(w.get(door + IVec3::Y), block::DOOR);
        assert_ne!(w.state(door + IVec3::Y) & block::DOOR_UPPER, 0);
        let removed = w.break_block(door + IVec3::Y).unwrap();
        assert_eq!(removed.len(), 2);
        assert_eq!((w.get(door), w.get(door + IVec3::Y)), (0, 0));
        // Removing the wall drops the torch; removing soil drops the flower.
        w.place_block(IVec3::new(1, 40, 4), block::POPPY, 0);
        let removed = w.break_block(IVec3::new(4, 40, 5)).unwrap();
        assert!(removed.contains(&(IVec3::new(4, 40, 4), block::TORCH)));
        assert_eq!(w.get(IVec3::new(4, 40, 4)), 0);
        w.break_block(IVec3::new(1, 39, 4)).unwrap();
        assert_eq!(w.get(IVec3::new(1, 40, 4)), 0);
        // Sugar cane stacks fall apart from the bottom.
        w.set(IVec3::new(3, 39, 4), block::SAND);
        for y in 40..43 {
            w.set(IVec3::new(3, y, 4), block::SUGAR_CANE);
        }
        let removed = w.break_block(IVec3::new(3, 40, 4)).unwrap();
        assert_eq!(removed.len(), 3);
        // Plants never collide.
        assert!(!w.solid(IVec3::new(3, 41, 4)));
    }
}
