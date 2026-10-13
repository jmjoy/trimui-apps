//! Section meshing: greedy-merged cube faces with smooth light and ambient
//! occlusion baked into compact vertices.
//!
//! A job snapshots the 3×3 neighbouring columns (shared, copy-on-write) on
//! the game thread and does all the work on a worker.
use crate::world::{
    BEDROCK, CHUNK, HEIGHT, World,
    block::{self, Face, Render},
    chunk::{FULL_SKY, Section, light_index, local_index},
};
use bytemuck::{Pod, Zeroable};
use glam::{IVec2, IVec3};
use std::sync::Arc;

/// 16 bytes. Positions are in 1/16 block relative to the section origin,
/// texture coordinates in 1/16 tile.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
pub struct Vertex {
    pub pos: [u16; 4],
    pub uv: [i16; 2],
    /// Texture layer, sky light ×4 (0–60), block light ×4, face | ao << 3.
    pub info: [u8; 4],
}

pub enum Indices {
    U16(Vec<u16>),
    U32(Vec<u32>),
}
impl Indices {
    pub fn len(&self) -> usize {
        match self {
            Indices::U16(v) => v.len(),
            Indices::U32(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn bytes(&self) -> &[u8] {
        match self {
            Indices::U16(v) => bytemuck::cast_slice(v),
            Indices::U32(v) => bytemuck::cast_slice(v),
        }
    }
    pub fn format(&self) -> wgpu::IndexFormat {
        match self {
            Indices::U16(_) => wgpu::IndexFormat::Uint16,
            Indices::U32(_) => wgpu::IndexFormat::Uint32,
        }
    }
    #[cfg(test)]
    pub fn to_vec(&self) -> Vec<u32> {
        match self {
            Indices::U16(v) => v.iter().map(|&i| u32::from(i)).collect(),
            Indices::U32(v) => v.clone(),
        }
    }
}

pub struct Mesh {
    pub key: IVec3,
    pub revision: u64,
    pub epoch: u64,
    pub style: u32,
    pub vertices: Vec<Vertex>,
    pub indices: Indices,
}

struct ColumnSnapshot {
    /// Sections key.y − 1, key.y and key.y + 1.
    sections: [Option<Arc<Section>>; 3],
    light: Arc<Vec<u8>>,
}

pub struct Job {
    /// Section key: column x, section index, column z.
    pub key: IVec3,
    pub revision: u64,
    pub epoch: u64,
    pub style: u32,
    smooth: bool,
    columns: Vec<ColumnSnapshot>,
}

const H: i32 = 18;
fn cell_index(p: IVec3) -> usize {
    (p.x + H * (p.y + H * p.z)) as usize
}

/// Whether the face of `id` towards `neighbour` must be drawn.
fn visible_face(id: u8, neighbour: u8) -> bool {
    if id == 0 || block::def(id).render == Render::Air {
        return false;
    }
    if block::occludes(neighbour) {
        return false;
    }
    // Faces between two identical see-through cubes are hidden.
    !(neighbour == id && block::is_cube(id))
}

/// Halo copy of blocks and light around one section.
struct Cells {
    blocks: Vec<u8>,
    light: Vec<u8>,
    states: Vec<u8>,
}
impl Cells {
    #[inline]
    fn block(&self, p: IVec3) -> u8 {
        self.blocks[cell_index(p + IVec3::ONE)]
    }
    #[inline]
    fn light(&self, p: IVec3) -> u8 {
        self.light[cell_index(p + IVec3::ONE)]
    }
}

/// Packs one vertex's light and occlusion into 14 bits.
fn code(sky: u8, block: u8, ao: u8) -> u64 {
    u64::from(sky) | u64::from(block) << 6 | u64::from(ao) << 12
}
fn decode(code: u64) -> (u8, u8, u8) {
    (
        (code & 63) as u8,
        (code >> 6 & 63) as u8,
        (code >> 12 & 3) as u8,
    )
}

impl Job {
    /// Snapshots the section's surroundings; `None` until the 3×3
    /// neighbouring columns are loaded and lit.
    pub fn new(world: &World, key: IVec3, epoch: u64, style: u32, smooth: bool) -> Option<Self> {
        let center = IVec2::new(key.x, key.z);
        let mut columns = Vec::with_capacity(9);
        for i in 0..9 {
            let column = world.column(center + IVec2::new(i % 3 - 1, i / 3 - 1))?;
            if !column.lit {
                return None;
            }
            let section = |s: i32| usize::try_from(s).ok().and_then(|s| column.section_arc(s));
            columns.push(ColumnSnapshot {
                sections: [section(key.y - 1), section(key.y), section(key.y + 1)],
                light: column.light_arc(),
            });
        }
        Some(Self {
            key,
            revision: world.section_revision(key)?,
            epoch,
            style,
            smooth,
            columns,
        })
    }
    fn cells(&self) -> Cells {
        let n = (H * H * H) as usize;
        let mut blocks = vec![0; n];
        let mut light = vec![0; n];
        let y0 = self.key.y * CHUNK - 1;
        for z in 0..H {
            let (cz, lz) = ((z + 15) / 16, (z + 15) % 16);
            for x in 0..H {
                let (cx, lx) = ((x + 15) / 16, (x + 15) % 16);
                let column = &self.columns[(cx + cz * 3) as usize];
                for y in 0..H {
                    let wy = y0 + y;
                    let i = cell_index(IVec3::new(x, y, z));
                    // Below the world counts as solid, above it as open sky.
                    if wy < 0 {
                        blocks[i] = BEDROCK;
                    } else if wy >= HEIGHT {
                        light[i] = FULL_SKY;
                    } else {
                        let s = (wy >> 4) - (self.key.y - 1);
                        if let Some(section) = &column.sections[s as usize] {
                            blocks[i] = section.blocks[local_index(lx, wy & 15, lz)];
                        }
                        light[i] = column.light[light_index(lx, wy, lz)];
                    }
                }
            }
        }
        let states = self.columns[4].sections[1]
            .as_ref()
            .map_or_else(|| vec![0; 4096], |s| s.state.to_vec());
        Cells {
            blocks,
            light,
            states,
        }
    }
    /// Light and occlusion of the four corners of the face of the block at
    /// `p` whose outside neighbour is `q`.
    fn corners(&self, cells: &Cells, q: IVec3, u: usize, v: usize) -> [u64; 4] {
        if !self.smooth {
            let l = cells.light(q);
            let c = code((l >> 4) * 4, (l & 15) * 4, 3);
            return [c; 4];
        }
        let mut du = IVec3::ZERO;
        du[u] = 1;
        let mut dv = IVec3::ZERO;
        dv[v] = 1;
        let occludes = |p: IVec3| block::occludes(cells.block(p));
        [(-1, -1), (1, -1), (1, 1), (-1, 1)].map(|(su, sv)| {
            let side1 = q + du * su;
            let side2 = q + dv * sv;
            let corner = side1 + dv * sv;
            let (o1, o2, oc) = (occludes(side1), occludes(side2), occludes(corner));
            let ao = if o1 && o2 {
                0
            } else {
                3 - u8::from(o1) - u8::from(o2) - u8::from(oc)
            };
            let mut sky = 0u32;
            let mut blk = 0u32;
            let mut n = 0u32;
            for (cell, open) in [
                (q, true),
                (side1, !o1),
                (side2, !o2),
                (corner, !oc && !(o1 && o2)),
            ] {
                if open {
                    let l = cells.light(cell);
                    sky += u32::from(l >> 4);
                    blk += u32::from(l & 15);
                    n += 1;
                }
            }
            let avg = |sum: u32| ((sum * 4 + n / 2) / n) as u8;
            code(avg(sky), avg(blk), ao)
        })
    }
    pub fn build(self) -> Mesh {
        let cells = self.cells();
        let mut vertices = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for axis in 0..3 {
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            for sign in [-1, 1] {
                let face = Face::from_axis(axis, sign > 0);
                for slice in 0..CHUNK {
                    // Per cell: texture layer + 1 and the four corner codes.
                    let mut mask = [0u64; 256];
                    for j in 0..CHUNK {
                        for i in 0..CHUNK {
                            let mut p = IVec3::ZERO;
                            p[axis] = slice;
                            p[u] = i;
                            p[v] = j;
                            let id = cells.block(p);
                            let mut q = p;
                            q[axis] += sign;
                            if !visible_face(id, cells.block(q)) {
                                continue;
                            }
                            let state = cells.states[local_index(p.x, p.y, p.z)];
                            let layer = block::face_texture(id, state, face) as u64 + 1;
                            let c = self.corners(&cells, q, u, v);
                            mask[(i + 16 * j) as usize] =
                                layer | c[0] << 8 | c[1] << 22 | c[2] << 36 | c[3] << 50;
                        }
                    }
                    greedy(&mut mask, |i, j, w, h, key| {
                        emit(
                            &mut vertices,
                            &mut indices,
                            Quad {
                                axis,
                                u,
                                v,
                                sign,
                                face,
                                slice,
                                i,
                                j,
                                w,
                                h,
                                key,
                            },
                        )
                    });
                }
            }
        }
        let indices = if vertices.len() <= usize::from(u16::MAX) + 1 {
            Indices::U16(indices.iter().map(|&i| i as u16).collect())
        } else {
            Indices::U32(indices)
        };
        Mesh {
            key: self.key,
            revision: self.revision,
            epoch: self.epoch,
            style: self.style,
            vertices,
            indices,
        }
    }
}

fn corner(key: u64, k: usize) -> u64 {
    key >> (8 + 14 * k) & 0x3fff
}
/// Faces may only merge along a direction their light does not vary in,
/// otherwise the merged quad would interpolate over the wrong distance.
fn uniform_u(key: u64) -> bool {
    corner(key, 0) == corner(key, 1) && corner(key, 3) == corner(key, 2)
}
fn uniform_v(key: u64) -> bool {
    corner(key, 0) == corner(key, 3) && corner(key, 1) == corner(key, 2)
}

fn greedy(mask: &mut [u64; 256], mut emit: impl FnMut(usize, usize, usize, usize, u64)) {
    for j in 0..16 {
        let mut i = 0;
        while i < 16 {
            let key = mask[i + 16 * j];
            if key == 0 {
                i += 1;
                continue;
            }
            let mut width = 1;
            if uniform_u(key) {
                while i + width < 16 && mask[i + width + 16 * j] == key {
                    width += 1;
                }
            }
            let mut height = 1;
            if uniform_v(key) {
                'grow: while j + height < 16 {
                    for dx in 0..width {
                        if mask[i + dx + 16 * (j + height)] != key {
                            break 'grow;
                        }
                    }
                    height += 1;
                }
            }
            for dy in 0..height {
                for dx in 0..width {
                    mask[i + dx + 16 * (j + dy)] = 0;
                }
            }
            emit(i, j, width, height, key);
            i += width;
        }
    }
}

struct Quad {
    axis: usize,
    u: usize,
    v: usize,
    sign: i32,
    face: Face,
    slice: i32,
    i: usize,
    j: usize,
    w: usize,
    h: usize,
    key: u64,
}

fn emit(vertices: &mut Vec<Vertex>, indices: &mut Vec<u32>, q: Quad) {
    let layer = (q.key & 0xff) as u8 - 1;
    let mut base = [0i32; 3];
    base[q.axis] = (q.slice + i32::from(q.sign > 0)) * 16;
    base[q.u] = q.i as i32 * 16;
    base[q.v] = q.j as i32 * 16;
    let (w, h) = (q.w as i32 * 16, q.h as i32 * 16);
    // East and north faces would show textures mirrored; negating u (with
    // repeat addressing) makes every side read left to right.
    let flip = matches!(q.face, Face::East | Face::North);
    let face = q.face as u8;
    let first = vertices.len() as u32;
    let mut brightness = [0u32; 4];
    for (k, (cu, cv)) in [(0, 0), (w, 0), (w, h), (0, h)].into_iter().enumerate() {
        let mut p = base;
        p[q.u] += cu;
        p[q.v] += cv;
        let mut uv = if q.axis == 0 { [cv, cu] } else { [cu, cv] };
        if flip {
            uv[0] = -uv[0];
        }
        let (sky, blk, ao) = decode(corner(q.key, k));
        brightness[k] = u32::from(sky.max(blk)) + u32::from(ao) * 20;
        vertices.push(Vertex {
            pos: [p[0] as u16, p[1] as u16, p[2] as u16, 0],
            uv: [uv[0] as i16, uv[1] as i16],
            info: [layer, sky, blk, face | ao << 3],
        });
    }
    // Split along the diagonal that keeps occlusion gradients symmetric.
    let flipped = brightness[0] + brightness[2] > brightness[1] + brightness[3];
    let order: [u32; 6] = match (q.sign > 0, flipped) {
        (true, false) => [0, 1, 2, 0, 2, 3],
        (true, true) => [1, 2, 3, 1, 3, 0],
        (false, false) => [0, 2, 1, 0, 3, 2],
        (false, true) => [1, 3, 2, 1, 0, 3],
    };
    indices.extend(order.map(|n| first + n));
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn mesh(w: &World, key: IVec3, smooth: bool) -> Mesh {
        Job::new(w, key, 1, 0, smooth).unwrap().build()
    }
    fn world_pos(m: &Mesh, v: &Vertex) -> Vec3 {
        (m.key * 16).as_vec3() + Vec3::new(v.pos[0] as f32, v.pos[1] as f32, v.pos[2] as f32) / 16.0
    }
    fn quads(m: &Mesh) -> Vec<Vec<Vec3>> {
        m.vertices
            .chunks(4)
            .map(|q| q.iter().map(|v| world_pos(m, v)).collect())
            .collect()
    }

    #[test]
    fn greedy_faces_and_neighbor_occlusion() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 3);
        // A full section on the world floor: its bottom face is hidden and
        // the other five are evenly lit, so each merges into one quad.
        let key = IVec3::new(0, 0, 0);
        for z in 0..16 {
            for y in 0..16 {
                for x in 0..16 {
                    w.set(IVec3::new(x, y, z), 3);
                }
            }
        }
        let m = mesh(&w, key, false);
        assert_eq!(m.indices.len(), 30);
        assert_eq!(m.vertices.len(), 20);
        assert!(matches!(m.indices, Indices::U16(_)));
        // A solid wall in the next column hides the +X face.
        for z in 0..16 {
            for y in 0..16 {
                w.set(IVec3::new(16, y, z), 3);
            }
        }
        let m = mesh(&w, key, false);
        assert_eq!(m.indices.len(), 24);
        let idx = m.indices.to_vec();
        for face in idx.as_chunks::<6>().0 {
            let p = |i: u32| world_pos(&m, &m.vertices[i as usize]);
            let (a, b, c) = (p(face[0]), p(face[1]), p(face[2]));
            assert!((b - a).cross(c - a).length() > 0.0);
        }
        // The bottom of the world never shows faces; negative columns work.
        w.set(IVec3::new(-5, 0, -5), 3);
        let m = mesh(&w, IVec3::new(-1, 0, -1), false);
        assert_eq!(m.indices.len(), 30);
        assert!(
            quads(&m)
                .iter()
                .flatten()
                .all(|p| p.x >= -5.0 && p.x <= -4.0)
        );
        // Shadow under an overhang breaks merging where the light differs.
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 2);
        for z in 0..8 {
            for x in 0..8 {
                w.set(IVec3::new(x, 40, z), 3);
            }
        }
        let m = mesh(&w, IVec3::new(0, 2, 0), false);
        let bottoms = quads(&m)
            .iter()
            .filter(|q| q.iter().all(|p| p.y == 40.0))
            .count();
        assert!(bottoms > 4, "graded light under the slab: {bottoms} quads");
        assert!(
            Job::new(&w, IVec3::new(2, 0, 2), 1, 0, true).is_none(),
            "neighbours unlit"
        );
    }

    #[test]
    fn faces_use_registry_textures_and_read_left_to_right() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 2);
        w.set(IVec3::new(4, 20, 4), block::GRASS);
        let m = mesh(&w, IVec3::new(0, 1, 0), true);
        assert_eq!(m.indices.len(), 36);
        let layer_where = |axis: usize, value: f32| -> Vec<u8> {
            m.vertices
                .chunks(4)
                .filter(|q| q.iter().all(|v| world_pos(&m, v)[axis] == value))
                .map(|q| q[0].info[0])
                .collect()
        };
        assert_eq!(layer_where(1, 21.0), [block::Tex::GrassTop as u8]);
        assert_eq!(layer_where(1, 20.0), [block::Tex::Dirt as u8]);
        assert_eq!(layer_where(0, 5.0), [block::Tex::GrassSide as u8]);
        let us = |axis: usize, value: f32| -> Vec<i16> {
            m.vertices
                .chunks(4)
                .find(|q| q.iter().all(|v| world_pos(&m, v)[axis] == value))
                .unwrap()
                .iter()
                .map(|v| v.uv[0])
                .collect()
        };
        assert!(us(0, 5.0).iter().all(|u| *u <= 0));
        assert!(us(0, 4.0).iter().all(|u| *u >= 0));
        assert!(us(2, 4.0).iter().all(|u| *u <= 0));
        assert!(us(2, 5.0).iter().all(|u| *u >= 0));
        // Oriented blocks put their front on the facing side.
        w.set_with_state(IVec3::new(8, 20, 8), block::FURNACE, 1);
        let m = mesh(&w, IVec3::new(0, 1, 0), true);
        let front = block::Tex::FurnaceFront as u8;
        let fronts: Vec<_> = m
            .vertices
            .chunks(4)
            .filter(|q| q[0].info[0] == front)
            .collect();
        assert_eq!(fronts.len(), 1);
        assert!(
            fronts[0].iter().all(|v| world_pos(&m, v).x == 9.0),
            "front faces east"
        );
        // Leaves hide each other (fast leaves), glass shows what is behind.
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 2);
        w.set(IVec3::new(4, 20, 4), block::OAK_LEAVES);
        w.set(IVec3::new(5, 20, 4), block::OAK_LEAVES);
        w.set(IVec3::new(4, 22, 4), block::STONE);
        w.set(IVec3::new(5, 22, 4), block::GLASS);
        let m = mesh(&w, IVec3::new(0, 1, 0), false);
        // The two leaves share their sides (6 quads) except the top, which is
        // shaded under the stone but not under the glass (+1); stone 6,
        // glass 5.
        assert_eq!(m.indices.len() / 6, 7 + 6 + 5);
    }

    #[test]
    fn corners_darken_next_to_walls_and_carry_light() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 2);
        // Floor of 3×3 stone with a wall block on one side of the centre.
        for z in 3..6 {
            for x in 3..6 {
                w.set(IVec3::new(x, 20, z), block::STONE);
            }
        }
        w.set(IVec3::new(5, 21, 4), block::STONE);
        let m = mesh(&w, IVec3::new(0, 1, 0), true);
        // The top face of the centre floor block (4, 20, 4).
        let top = m
            .vertices
            .chunks(4)
            .find(|q| {
                let ps: Vec<_> = q.iter().map(|v| world_pos(&m, v)).collect();
                ps.iter().all(|p| p.y == 21.0)
                    && ps.iter().any(|p| p.x == 4.0 && p.z == 4.0)
                    && ps.iter().any(|p| p.x == 5.0 && p.z == 5.0)
            })
            .expect("centre top face");
        for v in top {
            let p = world_pos(&m, v);
            let ao = v.info[3] >> 3;
            if p.x == 5.0 {
                assert!(ao < 3, "corner next to the wall is occluded: {p}");
            } else {
                assert_eq!(ao, 3, "open corner: {p}");
            }
            assert_eq!(v.info[1], 60, "full sky light");
        }
        // Glowstone light reaches neighbouring faces.
        w.set(IVec3::new(8, 21, 8), block::GLOWSTONE);
        let m = mesh(&w, IVec3::new(0, 1, 0), false);
        assert!(m.vertices.iter().any(|v| v.info[2] == 15 * 4 - 4));
        assert_eq!(std::mem::size_of::<Vertex>(), 16);
    }

    #[test]
    fn merging_respects_light_gradients() {
        let mut mask = [0u64; 256];
        let gradient = 1
            | code(60, 0, 3) << 8
            | code(60, 0, 3) << 22
            | code(40, 0, 3) << 36
            | code(40, 0, 3) << 50;
        assert!(uniform_u(gradient) && !uniform_v(gradient));
        for i in 0..4 {
            mask[i] = gradient;
            mask[16 + i] = gradient;
        }
        let mut rects = Vec::new();
        greedy(&mut mask, |i, j, w, h, _| rects.push((i, j, w, h)));
        assert_eq!(rects, [(0, 0, 4, 1), (0, 1, 4, 1)]);
        assert!(mask.iter().all(|&k| k == 0));
    }
}
