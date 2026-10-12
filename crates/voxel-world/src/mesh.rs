use crate::world::{self, CHUNK, World};
use bytemuck::{Pod, Zeroable};
use glam::IVec3;
use std::sync::mpsc::{self, Receiver, SyncSender};
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub layer: u32,
    pub shade: f32,
}
pub struct Job {
    pub id: usize,
    pub revision: u64,
    pub epoch: u64,
    cells: Vec<u8>,
}
pub struct Mesh {
    pub id: usize,
    pub revision: u64,
    pub epoch: u64,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}
fn cell_index(p: IVec3) -> usize {
    (p.x + 18 * (p.y + 18 * p.z)) as usize
}
impl Job {
    pub fn new(world: &World, id: usize, epoch: u64) -> Self {
        let origin = world::chunk_pos(id) * CHUNK - IVec3::ONE;
        let mut cells = vec![0; 18 * 18 * 18];
        for z in 0..18 {
            for y in 0..18 {
                for x in 0..18 {
                    let p = IVec3::new(x, y, z);
                    cells[cell_index(p)] = world.get(origin + p);
                }
            }
        }
        Self {
            id,
            revision: world.revisions[id],
            epoch,
            cells,
        }
    }
    fn get(&self, p: IVec3) -> u8 {
        self.cells[cell_index(p + IVec3::ONE)]
    }
    pub fn build(self) -> Mesh {
        let mut mesh = Mesh {
            id: self.id,
            revision: self.revision,
            epoch: self.epoch,
            vertices: Vec::new(),
            indices: Vec::new(),
        };
        let origin = (world::chunk_pos(self.id) * CHUNK).as_vec3();
        for axis in 0..3 {
            let u = (axis + 1) % 3;
            let v = (axis + 2) % 3;
            for sign in [-1, 1] {
                for slice in 0..CHUNK {
                    let mut mask = [0u8; 256];
                    for j in 0..CHUNK {
                        for i in 0..CHUNK {
                            let mut p = IVec3::ZERO;
                            p[axis] = slice;
                            p[u] = i;
                            p[v] = j;
                            let block = self.get(p);
                            p[axis] += sign;
                            if block != 0 && self.get(p) == 0 {
                                mask[(i + 16 * j) as usize] = match block {
                                    1 if axis == 1 && sign > 0 => 1,
                                    1 if axis == 1 => 2,
                                    1 => 10,
                                    5 if axis == 1 => 11,
                                    other => other,
                                };
                            }
                        }
                    }
                    let mut j = 0;
                    while j < 16 {
                        let mut i = 0;
                        while i < 16 {
                            let layer = mask[i + 16 * j];
                            if layer == 0 {
                                i += 1;
                                continue;
                            }
                            let mut width = 1;
                            while i + width < 16 && mask[i + width + 16 * j] == layer {
                                width += 1;
                            }
                            let mut height = 1;
                            'grow: while j + height < 16 {
                                for dx in 0..width {
                                    if mask[i + dx + 16 * (j + height)] != layer {
                                        break 'grow;
                                    }
                                }
                                height += 1;
                            }
                            let mut base = origin;
                            base[axis] += slice as f32 + if sign > 0 { 1.0 } else { 0.0 };
                            base[u] += i as f32;
                            base[v] += j as f32;
                            let mut du = glam::Vec3::ZERO;
                            du[u] = width as f32;
                            let mut dv = glam::Vec3::ZERO;
                            dv[v] = height as f32;
                            let first = mesh.vertices.len() as u32;
                            let shade = match axis {
                                1 if sign > 0 => 1.0,
                                1 => 0.52,
                                0 => 0.75,
                                _ => 0.88,
                            };
                            for (position, uv) in [
                                (base, [0.0, 0.0]),
                                (base + du, [width as f32, 0.0]),
                                (base + du + dv, [width as f32, height as f32]),
                                (base + dv, [0.0, height as f32]),
                            ] {
                                mesh.vertices.push(Vertex {
                                    position: position.to_array(),
                                    uv: if axis == 0 { [uv[1], uv[0]] } else { uv },
                                    layer: u32::from(layer),
                                    shade,
                                });
                            }
                            for n in if sign > 0 {
                                [0, 1, 2, 0, 2, 3]
                            } else {
                                [0, 2, 1, 0, 3, 2]
                            } {
                                mesh.indices.push(first + n);
                            }
                            for dy in 0..height {
                                for dx in 0..width {
                                    mask[i + dx + 16 * (j + dy)] = 0;
                                }
                            }
                            i += width;
                        }
                        j += 1;
                    }
                }
            }
        }
        mesh
    }
}
pub struct Worker {
    pub sender: SyncSender<Job>,
    pub receiver: Receiver<Mesh>,
}
impl Worker {
    pub fn new() -> Self {
        let (sender, jobs) = mpsc::sync_channel::<Job>(4);
        let (results, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                if results.send(job.build()).is_err() {
                    break;
                }
            }
        });
        Self { sender, receiver }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{CHUNKS, COUNT};
    #[test]
    fn greedy_faces_and_neighbor_occlusion() {
        let mut w = World {
            seed: 0,
            blocks: vec![0; COUNT],
            revisions: vec![1; CHUNKS],
        };
        for z in 0..16 {
            for y in 0..16 {
                for x in 0..16 {
                    w.set(IVec3::new(x, y, z), 3);
                }
            }
        }
        let m = Job::new(&w, 0, 1).build();
        assert_eq!(m.indices.len(), 36);
        assert_eq!(m.vertices.len(), 24);
        for z in 0..16 {
            for y in 0..16 {
                w.set(IVec3::new(16, y, z), 3);
            }
        }
        let m = Job::new(&w, 0, 1).build();
        assert_eq!(m.indices.len(), 30);
        for face in m.indices.as_chunks::<6>().0 {
            let a = glam::Vec3::from(m.vertices[face[0] as usize].position);
            let b = glam::Vec3::from(m.vertices[face[1] as usize].position);
            let c = glam::Vec3::from(m.vertices[face[2] as usize].position);
            assert!((b - a).cross(c - a).length() > 0.0);
        }
    }
}
