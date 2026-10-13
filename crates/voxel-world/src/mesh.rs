//! Section meshing.
//!
//! Cubes become greedy-merged faces with smooth light and ambient
//! occlusion baked into compact vertices. Liquids get sloped surfaces and
//! non-cube blocks (plants, torches, doors, …) small box or cross models.
//! Faces are sorted into the opaque, alpha-tested and translucent passes.
//!
//! A job snapshots the 3×3 neighbouring columns (shared, copy-on-write) on
//! the game thread and does all the work on a worker.
use crate::world::{
    BEDROCK, CHUNK, HEIGHT, World,
    block::{self, Face, Pass, Render, Tex},
    chunk::{FULL_SKY, Section, light_index, local_index},
};
use bytemuck::{Pod, Zeroable};
use glam::{IVec2, IVec3};
use std::sync::Arc;

/// 16 bytes. Positions are in 1/16 block relative to the section origin
/// (the fourth component holds flags), texture coordinates in 1/16 tile.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug, PartialEq, Eq)]
pub struct Vertex {
    pub pos: [u16; 4],
    pub uv: [i16; 2],
    /// Texture layer, sky light ×4 (0–60), block light ×4, face | ao << 3.
    pub info: [u8; 4],
}
/// Animated water and lava surfaces.
pub const FLAG_WATER: u16 = 1;
pub const FLAG_LAVA: u16 = 2;
/// Shading slot of faces without an axis direction (crossed plants).
const FACE_CROSS: u8 = 6;

pub const STYLE_SMOOTH: u32 = 1;
pub const STYLE_FANCY_LEAVES: u32 = 2;

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
    /// (first index, index count) per pass (opaque, cutout, translucent)
    /// and face group (east, west, up, down, south, north, other) inside
    /// `indices`; the groups of a pass are contiguous.
    pub ranges: [[(u32, u32); GROUPS]; 3],
    /// Which pairs of section faces are connected through see-through
    /// cells (see [`connected`]); used to cull hidden caves.
    pub visibility: u64,
}

/// Face groups per pass: six axis-aligned directions and everything else.
pub const GROUPS: usize = 7;
/// Storage order of the groups: east, up, south, other, north, down, west.
/// Opposite directions sit at opposite ends, so the groups facing a camera
/// outside the section mostly form one or two contiguous ranges.
pub const GROUP_ORDER: [usize; GROUPS] = [0, 2, 4, 6, 5, 3, 1];
impl Mesh {
    /// The whole index range of a pass.
    #[cfg(test)]
    pub fn pass(&self, p: usize) -> (u32, u32) {
        let r = &self.ranges[p];
        (r[0].0, r.iter().map(|(_, n)| n).sum())
    }
}
/// Every pair of faces connected (empty sections).
pub const VISIBLE_ALL: u64 = (1 << 36) - 1;
/// Whether light (and the eye) can pass from face `a` to face `b`.
pub fn connected(visibility: u64, a: Face, b: Face) -> bool {
    visibility & (1 << (a as usize * 6 + b as usize)) != 0
}
/// Flood fill over the section's non-opaque cells; each region links every
/// face it touches.
fn visibility(cells: &Cells) -> u64 {
    let open = |i: usize| {
        let p = IVec3::new((i & 15) as i32, (i >> 8) as i32, (i >> 4 & 15) as i32);
        !block::occludes(cells.block(p))
    };
    let mut seen = vec![false; 4096];
    let mut stack = Vec::with_capacity(512);
    let mut result = 0u64;
    for start in 0..4096 {
        if seen[start] || !open(start) {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut faces = 0u8;
        while let Some(i) = stack.pop() {
            let (x, z, y) = (i & 15, i >> 4 & 15, i >> 8);
            for (edge, face) in [
                (x == 15, Face::East),
                (x == 0, Face::West),
                (y == 15, Face::Up),
                (y == 0, Face::Down),
                (z == 15, Face::South),
                (z == 0, Face::North),
            ] {
                if edge {
                    faces |= 1 << face as u8;
                }
            }
            let mut visit = |j: usize| {
                if !seen[j] && open(j) {
                    seen[j] = true;
                    stack.push(j);
                }
            };
            if x < 15 {
                visit(i + 1);
            }
            if x > 0 {
                visit(i - 1);
            }
            if z < 15 {
                visit(i + 16);
            }
            if z > 0 {
                visit(i - 16);
            }
            if y < 15 {
                visit(i + 256);
            }
            if y > 0 {
                visit(i - 256);
            }
        }
        for a in 0..6 {
            if faces & (1 << a) == 0 {
                continue;
            }
            for b in 0..6 {
                if faces & (1 << b) != 0 {
                    result |= 1 << (a * 6 + b);
                }
            }
        }
    }
    result
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
    columns: Vec<ColumnSnapshot>,
}

const H: i32 = 18;
fn cell_index(p: IVec3) -> usize {
    (p.x + H * (p.y + H * p.z)) as usize
}

/// Halo copy of blocks, states and light around one section.
struct Cells {
    blocks: Vec<u8>,
    states: Vec<u8>,
    light: Vec<u8>,
}
impl Cells {
    /// `p` in section coordinates, −1..=16 on every axis.
    #[inline]
    fn block(&self, p: IVec3) -> u8 {
        self.blocks[cell_index(p + IVec3::ONE)]
    }
    #[inline]
    fn state(&self, p: IVec3) -> u8 {
        self.states[cell_index(p + IVec3::ONE)]
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
fn flat_light(l: u8) -> u64 {
    code((l >> 4) * 4, (l & 15) * 4, 3)
}

/// Canonical (u, v) corner order of [`Job::corners`]: −−, +−, ++, −+.
const CANONICAL: [(i32, i32); 4] = [(0, 0), (1, 0), (1, 1), (0, 1)];
/// Geometric order of canonical corners that winds counter-clockwise seen
/// from outside the face.
fn winding(positive: bool) -> [usize; 4] {
    if positive { [0, 1, 2, 3] } else { [0, 3, 2, 1] }
}
fn axes(axis: usize) -> (usize, usize) {
    ((axis + 1) % 3, (axis + 2) % 3)
}
fn face_axis(face: Face) -> (usize, bool) {
    match face {
        Face::East => (0, true),
        Face::West => (0, false),
        Face::Up => (1, true),
        Face::Down => (1, false),
        Face::South => (2, true),
        Face::North => (2, false),
    }
}
/// Texture coordinates (1/16) of a point inside a block, so that every side
/// reads left to right and upright from outside.
fn face_uv(face: Face, p: [i32; 3]) -> [i32; 2] {
    match face {
        Face::Up | Face::Down => [p[2], p[0]],
        Face::East => [16 - p[2], p[1]],
        Face::West => [p[2], p[1]],
        Face::South => [p[0], p[1]],
        Face::North => [16 - p[0], p[1]],
    }
}
/// Corners of one face of a box (1/16 units), counter-clockwise from outside.
fn box_face(min: [i32; 3], max: [i32; 3], face: Face) -> [[i32; 3]; 4] {
    let (axis, positive) = face_axis(face);
    let (u, v) = axes(axis);
    winding(positive).map(|k| {
        let (su, sv) = CANONICAL[k];
        let mut p = [0; 3];
        p[axis] = if positive { max[axis] } else { min[axis] };
        p[u] = if su == 0 { min[u] } else { max[u] };
        p[v] = if sv == 0 { min[v] } else { max[v] };
        p
    })
}

/// One quad, corners counter-clockwise from the visible side, positions in
/// 1/16 relative to the section origin.
struct Quad {
    pos: [[i32; 3]; 4],
    uv: [[i32; 2]; 4],
    layer: u8,
    light: [u64; 4],
    face: u8,
    flags: u16,
}

#[derive(Default)]
struct Builder {
    vertices: Vec<Vertex>,
    groups: [[Vec<u32>; GROUPS]; 3],
}
impl Builder {
    fn push(&mut self, pass: Pass, q: &Quad) {
        let first = self.vertices.len() as u32;
        let mut brightness = [0u32; 4];
        for (k, b) in brightness.iter_mut().enumerate() {
            let (sky, blk, ao) = decode(q.light[k]);
            *b = u32::from(sky.max(blk)) + u32::from(ao) * 20;
            let p = q.pos[k];
            self.vertices.push(Vertex {
                pos: [p[0] as u16, p[1] as u16, p[2] as u16, q.flags],
                uv: [q.uv[k][0] as i16, q.uv[k][1] as i16],
                info: [q.layer, sky, blk, q.face | ao << 3],
            });
        }
        // Split along the diagonal that keeps occlusion gradients symmetric.
        let order: [u32; 6] = if brightness[0] + brightness[2] > brightness[1] + brightness[3] {
            [1, 2, 3, 1, 3, 0]
        } else {
            [0, 1, 2, 0, 2, 3]
        };
        // Flat faces along an axis can be skipped when the camera is behind
        // them; blended faces are seen from both sides.
        let (axis, _) = match q.face {
            0 => (0, ()),
            1 => (0, ()),
            2 | 3 => (1, ()),
            4 | 5 => (2, ()),
            _ => (3, ()),
        };
        let flat = axis < 3 && q.pos.iter().all(|p| p[axis] == q.pos[0][axis]);
        let group = if flat && pass != Pass::Translucent {
            usize::from(q.face)
        } else {
            GROUPS - 1
        };
        self.groups[pass as usize][group].extend(order.map(|n| first + n));
    }
    /// A thin quad seen from both sides (plants, ladders): cutout faces are
    /// drawn without back-face culling, so one copy is enough.
    fn push_double(&mut self, pass: Pass, q: &Quad) {
        let face = q.face;
        self.push(
            pass,
            &Quad {
                face: FACE_CROSS,
                ..*q
            },
        );
        // Keep the face's own shading for axis-aligned planes.
        if face != FACE_CROSS
            && let Some(v) = self.vertices.len().checked_sub(4)
        {
            for vertex in &mut self.vertices[v..] {
                vertex.info[3] = vertex.info[3] & !7 | face;
            }
        }
    }
}
impl Clone for Quad {
    fn clone(&self) -> Self {
        *self
    }
}
impl Copy for Quad {}

impl Job {
    /// Snapshots the section's surroundings; `None` until the 3×3
    /// neighbouring columns are loaded and lit.
    pub fn new(world: &World, key: IVec3, epoch: u64, style: u32) -> Option<Self> {
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
            columns,
        })
    }
    fn smooth(&self) -> bool {
        self.style & STYLE_SMOOTH != 0
    }
    fn fancy(&self) -> bool {
        self.style & STYLE_FANCY_LEAVES != 0
    }
    fn cells(&self) -> Cells {
        let n = (H * H * H) as usize;
        let mut blocks = vec![0; n];
        let mut states = vec![0; n];
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
                            let j = local_index(lx, wy & 15, lz);
                            blocks[i] = section.blocks[j];
                            states[i] = section.state[j];
                        }
                        light[i] = column.light[light_index(lx, wy, lz)];
                    }
                }
            }
        }
        Cells {
            blocks,
            states,
            light,
        }
    }
    /// Light and occlusion of the four canonical corners of a face whose
    /// outside neighbour is `q`; `u`, `v` are the face's tangent axes.
    fn corners(&self, cells: &Cells, q: IVec3, u: usize, v: usize) -> [u64; 4] {
        if !self.smooth() {
            return [flat_light(cells.light(q)); 4];
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
        let mut out = Builder::default();
        self.cubes(&cells, &mut out);
        self.liquids(&cells, &mut out);
        self.models(&cells, &mut out);
        let mut ranges = [[(0, 0); GROUPS]; 3];
        let mut all = Vec::with_capacity(out.groups.iter().flatten().map(Vec::len).sum());
        for (pass_ranges, pass) in ranges.iter_mut().zip(&out.groups) {
            for g in GROUP_ORDER {
                pass_ranges[g] = (all.len() as u32, pass[g].len() as u32);
                all.extend_from_slice(&pass[g]);
            }
        }
        let indices = if out.vertices.len() <= usize::from(u16::MAX) + 1 {
            Indices::U16(all.iter().map(|&i| i as u16).collect())
        } else {
            Indices::U32(all)
        };
        Mesh {
            key: self.key,
            revision: self.revision,
            epoch: self.epoch,
            style: self.style,
            vertices: out.vertices,
            indices,
            ranges,
            visibility: visibility(&cells),
        }
    }

    fn cubes(&self, cells: &Cells, out: &mut Builder) {
        let fancy = self.fancy();
        for axis in 0..3 {
            let (u, v) = axes(axis);
            for positive in [false, true] {
                let face = Face::from_axis(axis, positive);
                let sign = if positive { 1 } else { -1 };
                for slice in 0..CHUNK {
                    let mut mask = [0u128; 256];
                    for j in 0..CHUNK {
                        for i in 0..CHUNK {
                            let mut p = IVec3::ZERO;
                            p[axis] = slice;
                            p[u] = i;
                            p[v] = j;
                            let id = cells.block(p);
                            if !block::is_cube(id) {
                                continue;
                            }
                            let mut q = p;
                            q[axis] += sign;
                            let neighbour = cells.block(q);
                            if block::hides(neighbour, fancy)
                                || (neighbour == id
                                    && !(fancy && block::def(id).render == Render::Leaves))
                            {
                                continue;
                            }
                            let layer = block::face_texture(id, cells.state(p), face) as u128 + 1;
                            // Leaves are noisy anyway; flat light lets their
                            // faces merge into far fewer quads.
                            let c = if block::def(id).render == Render::Leaves {
                                [flat_light(cells.light(q)); 4]
                            } else {
                                self.corners(cells, q, u, v)
                            };
                            let pass = block::pass(id, fancy) as u128;
                            mask[(i + 16 * j) as usize] = layer
                                | u128::from(c[0]) << 8
                                | u128::from(c[1]) << 22
                                | u128::from(c[2]) << 36
                                | u128::from(c[3]) << 50
                                | pass << 64;
                        }
                    }
                    greedy(&mut mask, |i, j, w, h, key| {
                        let base = {
                            let mut b = [0; 3];
                            b[axis] = (slice + i32::from(positive)) * 16;
                            b[u] = i as i32 * 16;
                            b[v] = j as i32 * 16;
                            b
                        };
                        out.push(pass_of(key), &cube_quad(axis, positive, base, w, h, key, 0));
                    });
                }
            }
        }
    }

    fn liquids(&self, cells: &Cells, out: &mut Builder) {
        let fancy = self.fancy();
        let (u, v) = axes(1);
        for y in 0..CHUNK {
            // Flat surfaces merge like cube faces; sloped ones do not.
            let mut tops = [0u128; 256];
            for z in 0..CHUNK {
                for x in 0..CHUNK {
                    let p = IVec3::new(x, y, z);
                    let id = cells.block(p);
                    if !block::liquid(id) {
                        continue;
                    }
                    let pass = block::pass(id, fancy);
                    let flags = if id == block::WATER {
                        FLAG_WATER
                    } else {
                        FLAG_LAVA
                    };
                    let layer = block::face_texture(id, 0, Face::Up) as u8;
                    let above = cells.block(p + IVec3::Y) == id;
                    // Corner heights in canonical (u = z, v = x) order.
                    let heights = CANONICAL.map(|(cz, cx)| corner_height(cells, p, id, cx, cz));
                    if !above {
                        let light = self.corners(cells, p + IVec3::Y, u, v);
                        if heights.iter().all(|&h| h == heights[0]) {
                            // Mask rows follow the face axes: i = z (u), j = x (v).
                            tops[(z + 16 * x) as usize] = (u128::from(layer) + 1)
                                | u128::from(light[0]) << 8
                                | u128::from(light[1]) << 22
                                | u128::from(light[2]) << 36
                                | u128::from(light[3]) << 50
                                | (pass as u128) << 64
                                | (heights[0] as u128) << 66
                                | u128::from(flags) << 72;
                        } else {
                            let base = [x * 16, y * 16, z * 16];
                            let pos = winding(true).map(|k| {
                                let (cz, cx) = CANONICAL[k];
                                [base[0] + cx * 16, base[1] + heights[k], base[2] + cz * 16]
                            });
                            out.push(
                                pass,
                                &Quad {
                                    pos,
                                    uv: winding(true)
                                        .map(|k| [CANONICAL[k].0 * 16, CANONICAL[k].1 * 16]),
                                    layer,
                                    light: winding(true).map(|k| light[k]),
                                    face: Face::Up as u8,
                                    flags,
                                },
                            );
                        }
                    }
                    // Sides and bottom against anything that does not hide them.
                    for face in [Face::East, Face::West, Face::South, Face::North, Face::Down] {
                        let n = p + face.normal();
                        let nb = cells.block(n);
                        if nb == id || block::hides(nb, fancy) {
                            continue;
                        }
                        let (axis, positive) = face_axis(face);
                        let (fu, fv) = axes(axis);
                        let light = self.corners(cells, n, fu, fv);
                        let mut corners = box_face([0, 0, 0], [16, 16, 16], face);
                        for c in &mut corners {
                            if c[1] == 16 {
                                c[1] = if above {
                                    16
                                } else {
                                    let k = CANONICAL
                                        .iter()
                                        .position(|&(cz, cx)| cx * 16 == c[0] && cz * 16 == c[2])
                                        .unwrap();
                                    heights[k]
                                };
                            }
                        }
                        let order = winding(positive);
                        out.push(
                            pass,
                            &Quad {
                                pos: corners.map(|c| [c[0] + x * 16, c[1] + y * 16, c[2] + z * 16]),
                                uv: corners.map(|c| face_uv(face, c)),
                                layer,
                                light: order.map(|k| light[k]),
                                face: face as u8,
                                flags,
                            },
                        );
                    }
                }
            }
            greedy(&mut tops, |i, j, w, h, key| {
                let height = (key >> 66 & 31) as i32;
                let flags = (key >> 72 & 0xffff) as u16;
                let mut base = [0; 3];
                base[1] = y * 16 + height;
                base[u] = i as i32 * 16;
                base[v] = j as i32 * 16;
                out.push(pass_of(key), &cube_quad(1, true, base, w, h, key, flags));
            });
        }
    }

    fn models(&self, cells: &Cells, out: &mut Builder) {
        let fancy = self.fancy();
        for y in 0..CHUNK {
            for z in 0..CHUNK {
                for x in 0..CHUNK {
                    let p = IVec3::new(x, y, z);
                    let id = cells.block(p);
                    let render = block::def(id).render;
                    if block::is_cube(id) || matches!(render, Render::Air | Render::Liquid) {
                        continue;
                    }
                    let model = Model {
                        cells,
                        p,
                        id,
                        state: cells.state(p),
                        light: flat_light(cells.light(p)),
                        pass: block::pass(id, fancy),
                        fancy,
                    };
                    model.emit(out);
                }
            }
        }
    }
}

fn pass_of(key: u128) -> Pass {
    match key >> 64 & 3 {
        0 => Pass::Opaque,
        1 => Pass::Cutout,
        _ => Pass::Translucent,
    }
}

/// Quad of a greedy rectangle `w`×`h` cells on a face along `axis`.
fn cube_quad(
    axis: usize,
    positive: bool,
    base: [i32; 3],
    w: usize,
    h: usize,
    key: u128,
    flags: u16,
) -> Quad {
    let (u, v) = axes(axis);
    let (wu, wv) = (w as i32 * 16, h as i32 * 16);
    let face = Face::from_axis(axis, positive);
    // East and north faces would show textures mirrored; negating u (with
    // repeat addressing) makes every side read left to right.
    let flip = matches!(face, Face::East | Face::North);
    let order = winding(positive);
    Quad {
        pos: order.map(|k| {
            let (su, sv) = CANONICAL[k];
            let mut p = base;
            p[u] += su * wu;
            p[v] += sv * wv;
            p
        }),
        uv: order.map(|k| {
            let (cu, cv) = (CANONICAL[k].0 * wu, CANONICAL[k].1 * wv);
            let mut uv = if axis == 0 { [cv, cu] } else { [cu, cv] };
            if flip {
                uv[0] = -uv[0];
            }
            uv
        }),
        layer: ((key & 0xff) - 1) as u8,
        light: order.map(|k| corner(key, k)),
        face: face as u8,
        flags,
    }
}

/// Height (1/16) of a liquid surface corner shared by up to four cells.
fn corner_height(cells: &Cells, p: IVec3, id: u8, cx: i32, cz: i32) -> i32 {
    let mut sum = 0;
    let mut n = 0;
    for dz in [cz - 1, cz] {
        for dx in [cx - 1, cx] {
            let q = p + IVec3::new(dx, 0, dz);
            if cells.block(q) != id {
                continue;
            }
            if cells.block(q + IVec3::Y) == id {
                return 16;
            }
            sum += block::liquid_height(cells.state(q));
            n += 1;
        }
    }
    (sum + n / 2) / n.max(1)
}

fn corner(key: u128, k: usize) -> u64 {
    (key >> (8 + 14 * k) & 0x3fff) as u64
}
/// Faces may only merge along a direction their light does not vary in,
/// otherwise the merged quad would interpolate over the wrong distance.
fn uniform_u(key: u128) -> bool {
    corner(key, 0) == corner(key, 1) && corner(key, 3) == corner(key, 2)
}
fn uniform_v(key: u128) -> bool {
    corner(key, 0) == corner(key, 3) && corner(key, 1) == corner(key, 2)
}

fn greedy(mask: &mut [u128; 256], mut emit: impl FnMut(usize, usize, usize, usize, u128)) {
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

/// One non-cube block being turned into quads.
struct Model<'a> {
    cells: &'a Cells,
    p: IVec3,
    id: u8,
    state: u8,
    light: u64,
    pass: Pass,
    fancy: bool,
}

/// Per-face textures of a box; `None` skips the face.
type BoxTextures = [Option<Tex>; 6];
const FACES: [Face; 6] = [
    Face::East,
    Face::West,
    Face::Up,
    Face::Down,
    Face::South,
    Face::North,
];

impl Model<'_> {
    fn origin(&self) -> [i32; 3] {
        [self.p.x * 16, self.p.y * 16, self.p.z * 16]
    }
    fn quad(&self, pos: [[i32; 3]; 4], uv: [[i32; 2]; 4], tex: Tex, face: u8) -> Quad {
        let o = self.origin();
        Quad {
            pos: pos.map(|c| [c[0] + o[0], c[1] + o[1], c[2] + o[2]]),
            uv,
            layer: tex as u8,
            light: [self.light; 4],
            face,
            flags: 0,
        }
    }
    /// A box in the north-facing frame, turned to `facing`; `shear` moves
    /// the top corners along −z (leaning torches).
    #[allow(clippy::too_many_arguments)]
    fn cuboid(
        &self,
        out: &mut Builder,
        min: [i32; 3],
        max: [i32; 3],
        textures: BoxTextures,
        uv_override: &[(Face, [i32; 4])],
        facing: u8,
        shear: i32,
    ) {
        for (i, face) in FACES.into_iter().enumerate() {
            let Some(tex) = textures[i] else { continue };
            let local = box_face(min, max, face);
            let uv = match uv_override.iter().find(|(f, _)| *f == face) {
                // Only used for top and bottom faces, whose texture axes
                // match the canonical corner axes.
                Some((_, [u0, v0, u1, v1])) => {
                    let (_, positive) = face_axis(face);
                    winding(positive).map(|k| {
                        let (su, sv) = CANONICAL[k];
                        [
                            if su == 0 { *u0 } else { *u1 },
                            if sv == 0 { *v0 } else { *v1 },
                        ]
                    })
                }
                None => local.map(|c| face_uv(face, c)),
            };
            let pos = local.map(|c| {
                let z = if c[1] == max[1] { c[2] - shear } else { c[2] };
                let (x, z) = block::rotate_xz(c[0], z, facing);
                [x, c[1], z]
            });
            let world_face = face.rotated(facing);
            // Faces flush with the cell boundary hide behind full blocks.
            let (axis, positive) = face_axis(world_face);
            let edge = if positive { 16 } else { 0 };
            if pos.iter().all(|c| c[axis] == edge)
                && block::hides(self.cells.block(self.p + world_face.normal()), self.fancy)
            {
                continue;
            }
            out.push(self.pass, &self.quad(pos, uv, tex, world_face as u8));
        }
    }
    /// A vertical double-sided quad from (x0, z0) to (x1, z1).
    #[allow(clippy::too_many_arguments)]
    fn plane(
        &self,
        out: &mut Builder,
        a: [i32; 2],
        b: [i32; 2],
        top: i32,
        tex: Tex,
        facing: u8,
        face: u8,
    ) {
        let corners = [
            [a[0], 0, a[1]],
            [b[0], 0, b[1]],
            [b[0], top, b[1]],
            [a[0], top, a[1]],
        ];
        let pos = corners.map(|c| {
            let (x, z) = block::rotate_xz(c[0], c[2], facing);
            [x, c[1], z]
        });
        let uv = [[0, 0], [16, 0], [16, top], [0, top]];
        out.push_double(self.pass, &self.quad(pos, uv, tex, face));
    }
    fn emit(&self, out: &mut Builder) {
        let all = |t: Tex| [Some(t); 6];
        let facing = self.state & 3;
        match block::def(self.id).render {
            Render::Cross => {
                let tex = block::face_texture(self.id, 0, Face::North);
                self.plane(out, [2, 2], [14, 14], 16, tex, 0, FACE_CROSS);
                self.plane(out, [2, 14], [14, 2], 16, tex, 0, FACE_CROSS);
            }
            Render::Crop => {
                let tex = [Tex::Wheat0, Tex::Wheat1, Tex::Wheat2, Tex::Wheat3]
                    [usize::from(self.state.min(7) / 2)];
                for x in [4, 12] {
                    self.plane(out, [x, 0], [x, 16], 16, tex, 0, FACE_CROSS);
                }
                for z in [4, 12] {
                    self.plane(out, [0, z], [16, z], 16, tex, 0, FACE_CROSS);
                }
            }
            Render::Torch => {
                let top = (Face::Up, [7, 9, 9, 11]);
                let bottom = (Face::Down, [7, 0, 9, 2]);
                match block::torch_pointing(self.state) {
                    None => self.cuboid(
                        out,
                        [7, 0, 7],
                        [9, 10, 9],
                        all(Tex::Torch),
                        &[top, bottom],
                        0,
                        0,
                    ),
                    // Leaning away from the wall it hangs on.
                    Some(f) => self.cuboid(
                        out,
                        [7, 3, 13],
                        [9, 13, 15],
                        all(Tex::Torch),
                        &[top, bottom],
                        f,
                        4,
                    ),
                }
            }
            Render::Cactus => {
                let below = self.cells.block(self.p - IVec3::Y);
                let above = self.cells.block(self.p + IVec3::Y);
                let mut tex = [Some(Tex::CactusSide); 6];
                tex[2] = (above != self.id).then_some(Tex::CactusTop);
                tex[3] = (below != self.id && !block::hides(below, self.fancy))
                    .then_some(Tex::CactusBottom);
                self.cuboid(out, [1, 0, 1], [15, 16, 15], tex, &[], 0, 0);
            }
            Render::Ladder => {
                self.plane(
                    out,
                    [0, 15],
                    [16, 15],
                    16,
                    Tex::Ladder,
                    facing,
                    Face::South.rotated(facing) as u8,
                );
            }
            Render::Door => {
                let tex = if self.state & block::DOOR_UPPER != 0 {
                    Tex::DoorUpper
                } else {
                    Tex::DoorLower
                };
                let (min, max) = if self.state & block::DOOR_OPEN != 0 {
                    ([0, 0, 0], [3, 16, 16])
                } else {
                    ([0, 0, 13], [16, 16, 16])
                };
                self.cuboid(out, min, max, all(tex), &[], facing, 0);
            }
            Render::Bed => {
                let top = if self.state & block::BED_HEAD != 0 {
                    Tex::BedHead
                } else {
                    Tex::BedFoot
                };
                let mut tex = all(Tex::BedSide);
                tex[2] = Some(top);
                tex[3] = Some(Tex::OakPlanks);
                self.cuboid(out, [0, 0, 0], [16, 9, 16], tex, &[], facing, 0);
            }
            Render::Layer => self.cuboid(out, [0, 0, 0], [16, 2, 16], all(Tex::Snow), &[], 0, 0),
            Render::Farmland => {
                let mut tex = all(Tex::Dirt);
                tex[2] = Some(if self.state & 1 != 0 {
                    Tex::FarmlandWet
                } else {
                    Tex::Farmland
                });
                self.cuboid(out, [0, 0, 0], [16, 15, 16], tex, &[], 0, 0);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn mesh_style(w: &World, key: IVec3, style: u32) -> Mesh {
        Job::new(w, key, 1, style).unwrap().build()
    }
    fn mesh(w: &World, key: IVec3, smooth: bool) -> Mesh {
        mesh_style(w, key, if smooth { STYLE_SMOOTH } else { 0 })
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
    /// Quads per pass (opaque, cutout, translucent).
    fn counts(m: &Mesh) -> [u32; 3] {
        [0, 1, 2].map(|p| m.pass(p).1 / 6)
    }
    fn front_facing_normals_point_out(m: &Mesh) {
        let idx = m.indices.to_vec();
        for t in idx.as_chunks::<3>().0 {
            let p = |i: u32| world_pos(m, &m.vertices[i as usize]);
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            assert!((b - a).cross(c - a).length() > 0.0, "degenerate triangle");
        }
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
        assert_eq!(counts(&m), [5, 0, 0]);
        assert!(matches!(m.indices, Indices::U16(_)));
        // A solid wall in the next column hides the +X face.
        for z in 0..16 {
            for y in 0..16 {
                w.set(IVec3::new(16, y, z), 3);
            }
        }
        let m = mesh(&w, key, false);
        assert_eq!(m.indices.len(), 24);
        front_facing_normals_point_out(&m);
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
            Job::new(&w, IVec3::new(2, 0, 2), 1, 0).is_none(),
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
    }

    #[test]
    fn passes_follow_leaf_style_and_transparency() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 2);
        w.set(IVec3::new(4, 20, 4), block::OAK_LEAVES);
        w.set(IVec3::new(5, 20, 4), block::OAK_LEAVES);
        w.set(IVec3::new(4, 22, 4), block::STONE);
        w.set(IVec3::new(5, 22, 4), block::GLASS);
        w.set(IVec3::new(9, 22, 9), block::ICE);
        let key = IVec3::new(0, 1, 0);
        // Fast leaves: opaque, hiding each other (leaves merge into 7 quads,
        // one top is shaded under the stone); glass is cut out, ice blended.
        let fast = mesh(&w, key, false);
        assert_eq!(counts(&fast), [7 + 6, 5, 6]);
        // Fancy leaves move to the cutout pass and also show the two faces
        // between them: 7 outer quads + 2 inner = 9.
        let fancy = mesh_style(&w, key, STYLE_FANCY_LEAVES);
        assert_eq!(counts(&fancy), [6, 5 + 9, 6]);
        front_facing_normals_point_out(&fancy);
    }

    #[test]
    fn liquids_slope_merge_and_animate() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 2);
        for z in 2..6 {
            for x in 2..6 {
                w.set(IVec3::new(x, 18, z), block::STONE);
                w.set(IVec3::new(x, 19, z), block::WATER);
            }
        }
        let m = mesh(&w, IVec3::new(0, 1, 0), false);
        let translucent: Vec<_> = {
            let (start, n) = m.pass(2);
            m.indices.to_vec()[start as usize..(start + n) as usize].to_vec()
        };
        let top: Vec<_> = translucent
            .iter()
            .map(|&i| &m.vertices[i as usize])
            .filter(|v| v.info[3] & 7 == Face::Up as u8)
            .collect();
        // The 4×4 pool surface is one quad at 14/16 of the block.
        assert_eq!(top.len(), 6);
        assert!(top.iter().all(|v| world_pos(&m, v).y == 19.0 + 14.0 / 16.0));
        assert!(top.iter().all(|v| v.pos[3] == FLAG_WATER));
        // Water above water: the lower cell has no top face and full sides.
        w.set(IVec3::new(3, 20, 3), block::WATER);
        let m = mesh(&w, IVec3::new(0, 1, 0), false);
        let corners: Vec<f32> = m
            .vertices
            .iter()
            .filter(|v| v.info[3] & 7 == Face::Up as u8)
            .map(|v| world_pos(&m, v).y)
            .collect();
        assert!(corners.contains(&20.875), "upper cell surface");
        assert!(
            corners.contains(&20.0),
            "corners next to the column rise to full height"
        );
        // Flowing water slopes down to its level.
        w.set_with_state(IVec3::new(6, 19, 3), block::WATER, 5);
        let m = mesh(&w, IVec3::new(0, 1, 0), false);
        let heights: Vec<f32> = m
            .vertices
            .iter()
            .filter(|v| v.info[3] & 7 == Face::Up as u8 && world_pos(&m, v).x >= 6.0)
            .map(|v| world_pos(&m, v).y - 19.0)
            .collect();
        assert!(
            heights.iter().any(|h| (*h - 6.0 / 16.0).abs() < 1e-4),
            "{heights:?}"
        );
        // Lava is opaque and animated slowly.
        w.set(IVec3::new(10, 19, 10), block::LAVA);
        let m = mesh(&w, IVec3::new(0, 1, 0), false);
        assert!(m.vertices.iter().any(|v| v.pos[3] == FLAG_LAVA));
        let (start, n) = m.pass(0);
        let idx = m.indices.to_vec();
        assert!(
            idx[start as usize..(start + n) as usize]
                .iter()
                .any(|&i| m.vertices[i as usize].pos[3] == FLAG_LAVA)
        );
    }

    #[test]
    fn models_for_plants_torches_ladders_doors_and_layers() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 2);
        let key = IVec3::new(0, 1, 0);
        let floor = |w: &mut World| {
            for z in 0..16 {
                for x in 0..16 {
                    w.set(IVec3::new(x, 16, z), block::STONE);
                }
            }
        };
        floor(&mut w);
        let base = counts(&mesh(&w, key, false));
        // Crossed plants: two diagonals (drawn without culling).
        w.set(IVec3::new(3, 17, 3), block::TALL_GRASS);
        let m = mesh(&w, key, false);
        assert_eq!(counts(&m)[1] - base[1], 2);
        assert!(m.vertices.iter().any(|v| v.info[3] & 7 == FACE_CROSS));
        // A standing torch: four sides, top and bottom; the bottom touches
        // the floor at the cell boundary and is hidden.
        w.set(IVec3::new(3, 17, 3), 0);
        w.set(IVec3::new(8, 17, 8), block::TORCH);
        let m = mesh(&w, key, false);
        assert_eq!(counts(&m)[1] - base[1], 5);
        let torch: Vec<Vec3> = quads(&m)
            .into_iter()
            .flatten()
            .filter(|p| p.y > 17.0)
            .collect();
        assert!(
            torch
                .iter()
                .all(|p| p.x >= 8.4 && p.x <= 8.6 && p.y <= 17.0 + 10.0 / 16.0)
        );
        // A wall torch leans away from its wall.
        w.set(IVec3::new(8, 17, 8), 0);
        w.set(IVec3::new(8, 18, 9), block::STONE);
        w.set_with_state(
            IVec3::new(8, 18, 8),
            block::TORCH,
            block::torch_state(Face::North).unwrap(),
        );
        let m = mesh(&w, key, false);
        let torch: Vec<Vec3> = quads(&m)
            .into_iter()
            .flatten()
            .filter(|p| p.y > 18.0 && p.y < 19.0 && p.z > 8.0 && p.z < 9.0)
            .collect();
        let low = torch
            .iter()
            .filter(|p| p.y < 18.3)
            .map(|p| p.z)
            .fold(0.0, f32::max);
        let high = torch
            .iter()
            .filter(|p| p.y > 18.7)
            .map(|p| p.z)
            .fold(0.0, f32::max);
        assert!(low > high, "top leans north: base z {low}, top z {high}");
        // Ladders are thin double-sided planes next to their wall.
        w.set(IVec3::new(8, 18, 8), 0);
        w.set_with_state(IVec3::new(8, 18, 8), block::LADDER, 0);
        let m = mesh(&w, key, false);
        let ladder: Vec<Vec3> = quads(&m)
            .into_iter()
            .flatten()
            .filter(|p| p.y >= 18.0 && p.x >= 8.0 && p.x <= 9.0 && p.z > 8.5 && p.z < 9.0)
            .collect();
        assert_eq!(ladder.len(), 4);
        // Snow layers are two pixels thick.
        w.set(IVec3::new(8, 18, 8), 0);
        w.set(IVec3::new(2, 17, 2), block::SNOW_LAYER);
        let m = mesh(&w, key, false);
        assert!(
            quads(&m)
                .iter()
                .any(|q| q.iter().all(|p| (p.y - (17.0 + 2.0 / 16.0)).abs() < 1e-4))
        );
        // An open door turns into the hinge side.
        w.set(IVec3::new(2, 17, 2), 0);
        w.set_with_state(IVec3::new(5, 17, 5), block::DOOR, block::DOOR_OPEN);
        let m = mesh(&w, key, false);
        let door: Vec<Vec3> = quads(&m)
            .into_iter()
            .flatten()
            .filter(|p| p.y > 17.0 && p.x < 7.0)
            .collect();
        assert!(!door.is_empty());
        assert!(
            door.iter()
                .all(|p| p.x >= 5.0 && p.x <= 5.0 + 3.0 / 16.0 + 1e-4)
        );
        front_facing_normals_point_out(&m);
    }

    #[test]
    fn corners_darken_next_to_walls_and_carry_light() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 2);
        for z in 3..6 {
            for x in 3..6 {
                w.set(IVec3::new(x, 20, z), block::STONE);
            }
        }
        w.set(IVec3::new(5, 21, 4), block::STONE);
        let m = mesh(&w, IVec3::new(0, 1, 0), true);
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
        w.set(IVec3::new(8, 21, 8), block::GLOWSTONE);
        let m = mesh(&w, IVec3::new(0, 1, 0), false);
        assert!(m.vertices.iter().any(|v| v.info[2] == 15 * 4 - 4));
        assert_eq!(std::mem::size_of::<Vertex>(), 16);
    }

    #[test]
    fn merging_respects_light_gradients() {
        let mut mask = [0u128; 256];
        let gradient = 1
            | u128::from(code(60, 0, 3)) << 8
            | u128::from(code(60, 0, 3)) << 22
            | u128::from(code(40, 0, 3)) << 36
            | u128::from(code(40, 0, 3)) << 50;
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

    #[test]
    fn visibility_links_faces_through_open_space_only() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 2);
        let key = IVec3::new(0, 1, 0);
        // Fill the section with stone, then dig a tunnel from west to east.
        for z in 0..16 {
            for y in 16..32 {
                for x in 0..16 {
                    w.set(IVec3::new(x, y, z), block::STONE);
                }
            }
        }
        let solid = mesh(&w, key, false);
        assert_eq!(solid.visibility, 0);
        for x in 0..16 {
            w.set(IVec3::new(x, 24, 8), 0);
        }
        let tunnel = mesh(&w, key, false);
        assert!(connected(tunnel.visibility, Face::West, Face::East));
        assert!(connected(tunnel.visibility, Face::East, Face::West));
        assert!(!connected(tunnel.visibility, Face::West, Face::Up));
        assert!(!connected(tunnel.visibility, Face::North, Face::South));
        // Glass and water do not block the view; an empty section links all.
        w.set(IVec3::new(5, 24, 8), block::GLASS);
        w.set(IVec3::new(6, 24, 8), block::WATER);
        assert!(connected(
            mesh(&w, key, false).visibility,
            Face::West,
            Face::East
        ));
        let open = mesh(&w, IVec3::new(0, 4, 0), false);
        assert_eq!(open.visibility, VISIBLE_ALL);
    }

    #[test]
    #[ignore = "statistics probe"]
    fn triangle_budget_probe() {
        let mut w = World::new(42);
        let spawn = w.spawn().floor().as_ivec3();
        let c = crate::world::column_of(spawn);
        w.generate_area(c, 6);
        let mut by_y = [0usize; 8];
        let (mut dark, mut lit, mut leaves) = (0usize, 0usize, 0usize);
        let mut flat_total = 0usize;
        for dz in -4..=4 {
            for dx in -4..=4 {
                for y in 0..8 {
                    let key = IVec3::new(c.x + dx, y, c.y + dz);
                    let m = Job::new(&w, key, 1, STYLE_SMOOTH).unwrap().build();
                    by_y[y as usize] += m.indices.len() / 3;
                    let far = dx.abs().max(dz.abs()) > 2;
                    flat_total += Job::new(&w, key, 1, if far { 0 } else { STYLE_SMOOTH })
                        .unwrap()
                        .build()
                        .indices
                        .len()
                        / 3;
                    for q in m.vertices.chunks(4) {
                        if q.iter().all(|v| v.info[1] == 0) {
                            dark += 2
                        } else {
                            lit += 2
                        }
                        if matches!(q[0].info[0], l if l == block::Tex::OakLeaves as u8 || l == block::Tex::BirchLeaves as u8 || l == block::Tex::SpruceLeaves as u8)
                        {
                            leaves += 2
                        }
                    }
                }
            }
        }
        eprintln!(
            "by section y: {by_y:?}; sky-dark {dark}, lit {lit}, leaves {leaves}; near-smooth/far-flat total {flat_total}"
        );
    }

    #[test]
    fn faces_are_grouped_by_direction() {
        let mut w = World::new(0);
        w.empty_area(IVec2::ZERO, 2);
        w.set(IVec3::new(4, 20, 4), block::STONE);
        w.set(IVec3::new(8, 20, 8), block::TALL_GRASS);
        let m = mesh(&w, IVec3::new(0, 1, 0), false);
        // One cube: one quad in each of the six opaque direction groups.
        for g in 0..6 {
            assert_eq!(m.ranges[0][g].1, 6, "group {g}");
        }
        assert_eq!(m.ranges[0][6].1, 0);
        // Crossed plants go to the cutout "other" group.
        assert_eq!(m.ranges[1][6].1, 12);
        // Groups are contiguous and in storage order.
        let mut next = 0;
        for p in 0..3 {
            for g in GROUP_ORDER {
                assert_eq!(m.ranges[p][g].0, next);
                next += m.ranges[p][g].1;
            }
        }
        assert_eq!(next as usize, m.indices.len());
        // The east group really holds east faces.
        let (start, n) = m.ranges[0][Face::East as usize];
        let idx = m.indices.to_vec();
        assert!(
            idx[start as usize..(start + n) as usize]
                .iter()
                .all(|&i| world_pos(&m, &m.vertices[i as usize]).x == 5.0)
        );
    }
}
