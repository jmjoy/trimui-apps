use glam::{IVec3, Vec3};

pub const WIDTH: i32 = 256;
pub const HEIGHT: i32 = 64;
pub const CHUNK: i32 = 16;
pub const COUNT: usize = (WIDTH * WIDTH * HEIGHT) as usize;
pub const CHUNKS: usize = 16 * 16 * 4;
pub const NAMES: [&str; 10] = [
    "空气",
    "草方块",
    "泥土",
    "石头",
    "沙",
    "原木",
    "树叶",
    "木板",
    "砖块",
    "基岩",
];
pub const COLORS: [[u8; 3]; 10] = [
    [0, 0, 0],
    [93, 153, 58],
    [128, 87, 53],
    [133, 139, 145],
    [221, 204, 137],
    [113, 79, 43],
    [61, 125, 47],
    [181, 137, 80],
    [168, 87, 64],
    [55, 59, 65],
];

#[derive(Clone)]
pub struct World {
    pub seed: u64,
    pub blocks: Vec<u8>,
    pub revisions: Vec<u64>,
}

pub fn chunk_id(p: IVec3) -> usize {
    (p.x + 16 * (p.z + 16 * p.y)) as usize
}
pub fn chunk_pos(id: usize) -> IVec3 {
    IVec3::new((id % 16) as i32, (id / 256) as i32, ((id / 16) % 16) as i32)
}
pub fn inside(p: IVec3) -> bool {
    p.x >= 0 && p.z >= 0 && p.y >= 0 && p.x < WIDTH && p.z < WIDTH && p.y < HEIGHT
}
fn index(p: IVec3) -> usize {
    (p.x + WIDTH * (p.z + WIDTH * p.y)) as usize
}
fn hash(seed: u64, x: i32, z: i32) -> u64 {
    let mut v = seed
        ^ (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
        ^ (z as u64).wrapping_mul(0xbf58476d1ce4e5b9);
    v = (v ^ (v >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94d049bb133111eb);
    v ^ (v >> 31)
}
fn noise(seed: u64, x: f32, z: f32) -> f32 {
    let ix = x.floor() as i32;
    let iz = z.floor() as i32;
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let tx = smooth(x.fract());
    let tz = smooth(z.fract());
    let h = |x, z| (hash(seed, x, z) >> 40) as f32 / 16777215.0;
    let a = h(ix, iz) * (1.0 - tx) + h(ix + 1, iz) * tx;
    let b = h(ix, iz + 1) * (1.0 - tx) + h(ix + 1, iz + 1) * tx;
    a * (1.0 - tz) + b * tz
}
impl World {
    pub fn generate(seed: u64) -> Self {
        let mut world = Self {
            seed,
            blocks: vec![0; COUNT],
            revisions: vec![1; CHUNKS],
        };
        let mut heights = vec![0; (WIDTH * WIDTH) as usize];
        for z in 0..WIDTH {
            for x in 0..WIDTH {
                let h = (17.0
                    + 22.0 * noise(seed, x as f32 / 64.0, z as f32 / 64.0)
                    + 5.0 * noise(seed ^ 99, x as f32 / 17.0, z as f32 / 17.0))
                    as i32;
                heights[(x + WIDTH * z) as usize] = h;
                let sand = noise(seed ^ 777, x as f32 / 43.0, z as f32 / 43.0) > 0.68;
                for y in 0..=h {
                    world.blocks[index(IVec3::new(x, y, z))] = if y == 0 {
                        9
                    } else if y < h - 3 {
                        3
                    } else if sand {
                        4
                    } else if y == h {
                        1
                    } else {
                        2
                    };
                }
            }
        }
        for z in 3..WIDTH - 3 {
            for x in 3..WIDTH - 3 {
                let h = heights[(x + WIDTH * z) as usize];
                // A small clear patch makes the initial controller experience predictable.
                if (x - 128).abs() < 5 && (z - 128).abs() < 5 {
                    continue;
                }
                if !hash(seed ^ 12, x, z).is_multiple_of(103) || world.get(IVec3::new(x, h, z)) != 1
                {
                    continue;
                }
                let trunk = 4 + (hash(seed, x, z) % 2) as i32;
                for dy in 1..=trunk {
                    world.blocks[index(IVec3::new(x, h + dy, z))] = 5;
                }
                for dy in trunk - 2..=trunk + 1 {
                    for dz in -2i32..=2 {
                        for dx in -2i32..=2 {
                            if dx.abs() + dz.abs() > 3
                                || (dy == trunk + 1 && dx.abs() + dz.abs() > 1)
                            {
                                continue;
                            }
                            let p = IVec3::new(x + dx, h + dy, z + dz);
                            if world.get(p) == 0 {
                                world.blocks[index(p)] = 6;
                            }
                        }
                    }
                }
            }
        }
        world
    }
    pub fn get(&self, p: IVec3) -> u8 {
        if inside(p) { self.blocks[index(p)] } else { 0 }
    }
    pub fn set(&mut self, p: IVec3, block: u8) -> bool {
        if !inside(p) || block > 8 || self.get(p) == 9 || self.get(p) == block {
            return false;
        }
        self.blocks[index(p)] = block;
        // Every mesh reads a one-cell halo, including its adjacent chunks.
        let c = p / CHUNK;
        self.revisions[chunk_id(c)] += 1;
        for axis in 0..3 {
            for step in [-1, 1] {
                let mut q = p;
                q[axis] += step;
                if inside(q) && q / CHUNK != c {
                    self.revisions[chunk_id(q / CHUNK)] += 1;
                }
            }
        }
        true
    }
    pub fn spawn(&self) -> Vec3 {
        for y in (1..HEIGHT - 2).rev() {
            let p = IVec3::new(128, y, 128);
            if self.get(p) != 0 && self.get(p + IVec3::Y) == 0 && self.get(p + IVec3::Y * 2) == 0 {
                return Vec3::new(128.5, y as f32 + 1.001, 128.5);
            }
        }
        Vec3::new(128.5, 62.0, 128.5)
    }
    pub fn raycast(&self, origin: Vec3, direction: Vec3, reach: f32) -> Option<Hit> {
        if direction.length_squared() < 0.5 {
            return None;
        }
        let d = direction.normalize();
        let mut p = origin.floor().as_ivec3();
        let step = d.signum().as_ivec3();
        let delta = Vec3::new(
            if d.x == 0.0 {
                f32::INFINITY
            } else {
                1.0 / d.x.abs()
            },
            if d.y == 0.0 {
                f32::INFINITY
            } else {
                1.0 / d.y.abs()
            },
            if d.z == 0.0 {
                f32::INFINITY
            } else {
                1.0 / d.z.abs()
            },
        );
        let mut next = Vec3::ZERO;
        for axis in 0..3 {
            next[axis] = if d[axis] == 0.0 {
                f32::INFINITY
            } else {
                ((p[axis] + i32::from(step[axis] > 0)) as f32 - origin[axis]) / d[axis]
            };
        }
        let mut distance = 0.0;
        let mut normal = IVec3::ZERO;
        for _ in 0..32 {
            if distance > reach {
                return None;
            }
            if self.get(p) != 0 {
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
    fn terrain_is_reproducible_and_has_safe_spawn() {
        let a = World::generate(42);
        let b = World::generate(42);
        assert_eq!(a.blocks, b.blocks);
        assert_ne!(a.blocks, World::generate(43).blocks);
        let p = a.spawn().floor().as_ivec3();
        assert_eq!(a.get(p), 0);
        assert_eq!(a.get(p + IVec3::Y), 0);
        assert_ne!(a.get(p - IVec3::Y), 0);
        assert!(a.blocks.contains(&5) && a.blocks.contains(&4));
    }
    #[test]
    fn boundary_edits_invalidate_neighbor_and_bedrock_is_protected() {
        let mut w = World {
            seed: 0,
            blocks: vec![0; COUNT],
            revisions: vec![1; CHUNKS],
        };
        assert!(w.set(IVec3::new(15, 17, 20), 3));
        assert_eq!(w.revisions[chunk_id(IVec3::new(0, 1, 1))], 2);
        assert_eq!(w.revisions[chunk_id(IVec3::new(1, 1, 1))], 2);
        w.blocks[0] = 9;
        assert!(!w.set(IVec3::ZERO, 0));
        assert!(!w.set(IVec3::new(-1, 0, 0), 1));
    }
    #[test]
    fn ray_returns_entry_face_and_obeys_reach() {
        let mut w = World {
            seed: 0,
            blocks: vec![0; COUNT],
            revisions: vec![1; CHUNKS],
        };
        w.set(IVec3::new(10, 10, 10), 3);
        let h = w.raycast(Vec3::new(10.5, 10.5, 7.0), Vec3::Z, 6.0).unwrap();
        assert_eq!(h.block, IVec3::splat(10));
        assert_eq!(h.normal, -IVec3::Z);
        assert!(
            w.raycast(Vec3::new(10.5, 10.5, 1.0), Vec3::Z, 6.0)
                .is_none()
        );
    }
}
