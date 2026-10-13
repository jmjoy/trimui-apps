//! Terrain generation: biomes, mountains, caves, ores and decorations.
//!
//! Every column is a pure function of the seed and its position. Height
//! comes from four climate noises (continentalness, erosion, temperature,
//! humidity) through continuous functions, so biome borders blend without
//! cliffs; the biome only picks surface blocks and decorations. Mountains
//! add a coarse 3D density field (overhangs), caves are carved from two
//! more 3D fields, both sampled on a grid and interpolated.
//!
//! Features that cross column borders (trees, ore veins) are evaluated
//! from every column they touch and only written into the column being
//! generated, so neighbours agree no matter which is generated first.
use super::{
    block::*,
    chunk::{Column, HEIGHT},
    noise::{self, Perlin},
};
use glam::{IVec2, IVec3, Vec3};
use std::sync::{Arc, Mutex, OnceLock};

pub const SEA_LEVEL: i32 = 62;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Biome {
    Ocean,
    Beach,
    Plains,
    Forest,
    BirchForest,
    Taiga,
    SnowyPlains,
    Desert,
    Mountains,
}
impl Biome {
    #[cfg(test)]
    pub const ALL: [Biome; 9] = [
        Biome::Ocean,
        Biome::Beach,
        Biome::Plains,
        Biome::Forest,
        Biome::BirchForest,
        Biome::Taiga,
        Biome::SnowyPlains,
        Biome::Desert,
        Biome::Mountains,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Biome::Ocean => "海洋",
            Biome::Beach => "沙滩",
            Biome::Plains => "平原",
            Biome::Forest => "森林",
            Biome::BirchForest => "白桦林",
            Biome::Taiga => "针叶林",
            Biome::SnowyPlains => "雪原",
            Biome::Desert => "沙漠",
            Biome::Mountains => "山地",
        }
    }
    /// Map colour.
    pub fn color(self) -> [u8; 3] {
        match self {
            Biome::Ocean => [40, 72, 160],
            Biome::Beach => [222, 210, 150],
            Biome::Plains => [126, 182, 82],
            Biome::Forest => [56, 120, 48],
            Biome::BirchForest => [104, 156, 80],
            Biome::Taiga => [52, 96, 80],
            Biome::SnowyPlains => [236, 242, 246],
            Biome::Desert => [232, 200, 120],
            Biome::Mountains => [130, 130, 136],
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Climate {
    pub continent: f32,
    pub erosion: f32,
    pub temperature: f32,
    pub humidity: f32,
}

fn smooth(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
/// Piecewise linear interpolation through sorted points.
fn spline(x: f32, points: &[(f32, f32)]) -> f32 {
    if x <= points[0].0 {
        return points[0].1;
    }
    for w in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if x <= x1 {
            return y0 + (y1 - y0) * (x - x0) / (x1 - x0);
        }
    }
    points[points.len() - 1].1
}
/// Base height by continentalness: deep ocean, shelf, coast, inland.
const BASE: [(f32, f32); 9] = [
    (-1.0, 30.0),
    (-0.55, 38.0),
    (-0.3, 48.0),
    (-0.12, 57.0),
    (-0.04, 61.5),
    (0.04, 63.5),
    (0.25, 67.0),
    (0.6, 74.0),
    (1.0, 80.0),
];
fn mountainous(cl: &Climate) -> f32 {
    smooth(-0.22, -0.5, cl.erosion) * smooth(0.02, 0.2, cl.continent)
}

/// Trilinear interpolation; `c[i]` is the corner at dx | dy << 1 | dz << 2.
fn trilerp(c: &[f32; 8], tx: f32, ty: f32, tz: f32) -> f32 {
    let x00 = c[0] + (c[1] - c[0]) * tx;
    let x10 = c[2] + (c[3] - c[2]) * tx;
    let x01 = c[4] + (c[5] - c[4]) * tx;
    let x11 = c[6] + (c[7] - c[6]) * tx;
    let y0 = x00 + (x10 - x00) * ty;
    let y1 = x01 + (x11 - x01) * ty;
    y0 + (y1 - y0) * tz
}
/// Value at block (x, y, z) of a field sampled every (sx, sy, sz) blocks.
/// Column generation and point queries share this code so their results
/// are bit-identical.
fn sample(grid: impl Fn(i32, i32, i32) -> f32, p: IVec3, s: IVec3) -> f32 {
    let g = p.div_euclid(s);
    let t = (p - g * s).as_vec3() / s.as_vec3();
    let c: [f32; 8] = std::array::from_fn(|i| {
        grid(
            g.x + (i & 1) as i32,
            g.y + (i >> 1 & 1) as i32,
            g.z + (i >> 2) as i32,
        )
    });
    trilerp(&c, t.x, t.y, t.z)
}
const MOUNTAIN_GRID: IVec3 = IVec3::new(4, 8, 4);
const CAVE_GRID: IVec3 = IVec3::new(4, 4, 4);
/// Largest height the mountain field adds or removes.
const MOUNTAIN_AMPLITUDE: f32 = 18.0;

/// Surface layers of one stretch of ground.
#[derive(Clone, Copy, Debug)]
struct Surface {
    top: u8,
    filler: u8,
    depth: i32,
    base: u8,
    base_depth: i32,
}
impl Surface {
    fn block(&self, depth: i32) -> u8 {
        if depth == 0 {
            self.top
        } else if depth < self.depth {
            self.filler
        } else if depth < self.depth + self.base_depth {
            self.base
        } else {
            STONE
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TreeKind {
    Oak,
    BigOak,
    Birch,
    Spruce,
}
#[derive(Clone, Copy, Debug)]
struct Tree {
    kind: TreeKind,
    x: i32,
    z: i32,
    /// Ground block under the trunk.
    ground: i32,
    trunk: i32,
}
/// Trees are placed at most once per TREE_CELL² blocks.
const TREE_CELL: i32 = 5;
/// Furthest a canopy reaches from its trunk.
const CANOPY: i32 = 3;

/// (block, lowest centre y, highest centre y, attempts per column, cells).
const ORES: [(u8, i32, i32, u32, u32); 6] = [
    (GRAVEL, 5, 100, 3, 18),
    (DIRT, 5, 100, 3, 18),
    (COAL_ORE, 5, 120, 16, 10),
    (IRON_ORE, 5, 64, 10, 7),
    (GOLD_ORE, 5, 32, 3, 6),
    (DIAMOND_ORE, 5, 16, 1, 5),
];
/// Ore veins stay within this distance of their centre.
const VEIN: i32 = 3;

struct Noises {
    continent: Perlin,
    erosion: Perlin,
    temperature: Perlin,
    humidity: Perlin,
    warp: Perlin,
    hills: Perlin,
    detail: Perlin,
    ridges: Perlin,
    mountain: Perlin,
    cave_a: Perlin,
    cave_b: Perlin,
    cheese: Perlin,
    patches: Perlin,
}

pub struct Generator {
    seed: u64,
    n: Noises,
    spawn: OnceLock<IVec3>,
}

/// Shared generator of a seed (permutation tables are built once).
pub fn generator(seed: u64) -> Arc<Generator> {
    static CACHE: Mutex<Vec<Arc<Generator>>> = Mutex::new(Vec::new());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(g) = cache.iter().find(|g| g.seed == seed) {
        return g.clone();
    }
    let g = Arc::new(Generator::new(seed));
    cache.insert(0, g.clone());
    cache.truncate(3);
    g
}
pub fn generate(seed: u64, pos: IVec2) -> Column {
    generator(seed).column(pos)
}
/// Player spawn: feet position on land near the origin.
pub fn spawn(seed: u64) -> Vec3 {
    let p = generator(seed).spawn();
    Vec3::new(p.x as f32 + 0.5, p.y as f32 + 0.001, p.z as f32 + 0.5)
}
pub fn biome_at(seed: u64, x: i32, z: i32) -> Biome {
    generator(seed).biome_at(x, z)
}

impl Generator {
    fn new(seed: u64) -> Self {
        let p = |k: i32| Perlin::new(noise::hash(seed, k, 0x7e44));
        Self {
            seed,
            n: Noises {
                continent: p(1),
                erosion: p(2),
                temperature: p(3),
                humidity: p(4),
                warp: p(5),
                hills: p(6),
                detail: p(7),
                ridges: p(8),
                mountain: p(9),
                cave_a: p(10),
                cave_b: p(11),
                cheese: p(12),
                patches: p(13),
            },
            spawn: OnceLock::new(),
        }
    }
    pub fn climate(&self, x: i32, z: i32) -> Climate {
        let (fx, fz) = (x as f32, z as f32);
        // Domain warping breaks up straight coastlines and climate bands.
        let wx = fx + self.n.warp.fbm2(fx / 320.0, fz / 320.0, 2) * 70.0;
        let wz = fz + self.n.warp.fbm2(fx / 320.0 + 31.7, fz / 320.0 + 47.3, 2) * 70.0;
        let s = 2.6;
        Climate {
            continent: self.n.continent.fbm2(wx / 900.0, wz / 900.0, 4) * s + 0.15,
            erosion: self.n.erosion.fbm2(wx / 520.0, wz / 520.0, 3) * s,
            temperature: self.n.temperature.fbm2(wx / 1100.0, wz / 1100.0, 3) * s,
            humidity: self.n.humidity.fbm2(wx / 800.0, wz / 800.0, 3) * s,
        }
    }
    fn height(&self, x: i32, z: i32, cl: &Climate) -> f32 {
        let (fx, fz) = (x as f32, z as f32);
        let base = spline(cl.continent, &BASE);
        let inland = smooth(-0.08, 0.12, cl.continent);
        let rough = smooth(0.45, -0.35, cl.erosion);
        let hills = self.n.hills.fbm2(fx / 120.0, fz / 120.0, 3) * (3.0 + 15.0 * rough) * inland;
        let detail = self.n.detail.fbm2(fx / 26.0, fz / 26.0, 2) * 1.6;
        let ridge = 1.0
            - (self.n.ridges.fbm2(fx / 170.0, fz / 170.0, 3) * 2.2)
                .abs()
                .min(1.0);
        let peaks = mountainous(cl) * (14.0 + 34.0 * ridge * ridge);
        (base + hills + detail + peaks).clamp(4.0, 116.0)
    }
    pub fn biome(&self, cl: &Climate, h: f32) -> Biome {
        let sea = SEA_LEVEL as f32;
        if h < sea - 1.0 {
            return Biome::Ocean;
        }
        if mountainous(cl) > 0.4 {
            return Biome::Mountains;
        }
        let t = cl.temperature;
        if h < sea + 2.5 && cl.continent < 0.1 {
            return if t < -0.5 {
                Biome::SnowyPlains
            } else if t > 0.42 && cl.humidity < 0.05 {
                Biome::Desert
            } else {
                Biome::Beach
            };
        }
        if t < -0.5 {
            Biome::SnowyPlains
        } else if t < -0.2 {
            Biome::Taiga
        } else if t > 0.42 && cl.humidity < 0.05 {
            Biome::Desert
        } else if cl.humidity > 0.18 {
            if t < 0.12 {
                Biome::BirchForest
            } else {
                Biome::Forest
            }
        } else {
            Biome::Plains
        }
    }
    pub fn biome_at(&self, x: i32, z: i32) -> Biome {
        let cl = self.climate(x, z);
        self.biome(&cl, self.height(x, z, &cl))
    }
    fn surface(&self, biome: Biome, x: i32, z: i32, y: i32) -> Surface {
        let patch = self.n.patches.noise2(x as f32 / 9.0, z as f32 / 9.0);
        let layers = |top, filler, depth, base, base_depth| Surface {
            top,
            filler,
            depth,
            base,
            base_depth,
        };
        let soil = |top| layers(top, DIRT, 4, DIRT, 0);
        let floor = if patch > 0.35 {
            GRAVEL
        } else if patch < -0.4 {
            CLAY
        } else {
            SAND
        };
        match biome {
            Biome::Ocean => layers(floor, floor, 3, SAND, 1),
            Biome::Beach => layers(SAND, SAND, 4, SANDSTONE, 2),
            Biome::Desert => layers(SAND, SAND, 4, SANDSTONE, 4),
            Biome::Mountains if y >= 104 => layers(SNOW, STONE, 1, STONE, 0),
            Biome::Mountains if y >= 90 || patch > 0.4 => {
                layers(if patch > 0.6 { GRAVEL } else { STONE }, STONE, 1, STONE, 0)
            }
            // Lake beds under water.
            _ if y < SEA_LEVEL - 1 => {
                layers(if patch > 0.1 { GRAVEL } else { DIRT }, DIRT, 3, DIRT, 0)
            }
            Biome::SnowyPlains => soil(SNOWY_GRASS),
            _ => soil(GRASS),
        }
    }
    fn mountain_corner(&self, gx: i32, gz: i32) -> f32 {
        mountainous(&self.climate(gx * MOUNTAIN_GRID.x, gz * MOUNTAIN_GRID.z))
    }
    fn mountain_value(&self, mf: f32, gx: i32, gy: i32, gz: i32) -> f32 {
        if mf <= 0.0 {
            return 0.0;
        }
        let p = (IVec3::new(gx, gy, gz) * MOUNTAIN_GRID).as_vec3();
        mf * self.n.mountain.fbm3(p.x / 28.0, p.y / 18.0, p.z / 28.0, 2) * MOUNTAIN_AMPLITUDE
    }
    fn cave_value(&self, channel: usize, gx: i32, gy: i32, gz: i32) -> f32 {
        let p = (IVec3::new(gx, gy, gz) * CAVE_GRID).as_vec3();
        match channel {
            0 => self.n.cave_a.noise3(p.x / 40.0, p.y / 24.0, p.z / 40.0),
            1 => self.n.cave_b.noise3(p.x / 40.0, p.y / 24.0, p.z / 40.0),
            _ => self.n.cheese.fbm3(p.x / 60.0, p.y / 34.0, p.z / 60.0, 2),
        }
    }
    /// Whether caves may open to the surface at (x, z): only in scattered
    /// entrance areas, so most caves stay sealed and invisible from above
    /// (which lets the renderer skip them).
    fn entrance(&self, x: i32, z: i32) -> bool {
        self.n
            .patches
            .noise2(x as f32 / 47.0 + 91.7, z as f32 / 47.0 - 13.1)
            > 0.3
    }
    /// Whether a cave removes a block, given the interpolated cave fields.
    fn carve(&self, v: [f32; 3], y: i32, top: i32, h: f32, entrance: bool) -> bool {
        // The sea floor stays sealed: no cave within five blocks of it.
        if y < 1 || (h < SEA_LEVEL as f32 + 1.0 && y > top - 5) {
            return false;
        }
        if y > top - 7 && !entrance {
            return false;
        }
        let noodle = v[0] * v[0] + v[1] * v[1] < 0.0032;
        let cavern = v[2] > 0.36 + 0.3 * smooth(30.0, 58.0, y as f32) && y < top - 6;
        noodle || cavern
    }
    /// Cave test at one block, computed directly.
    fn carved_at(&self, p: IVec3, top: i32, h: f32) -> bool {
        let v =
            std::array::from_fn(|k| sample(|x, y, z| self.cave_value(k, x, y, z), p, CAVE_GRID));
        self.carve(v, p.y, top, h, self.entrance(p.x, p.z))
    }
    /// Highest terrain block at (x, z) before caves and decorations.
    fn top_at(&self, x: i32, z: i32, h: f32) -> i32 {
        let g = IVec2::new(x, z).div_euclid(IVec2::new(MOUNTAIN_GRID.x, MOUNTAIN_GRID.z));
        let mf = [
            self.mountain_corner(g.x, g.y),
            self.mountain_corner(g.x + 1, g.y),
            self.mountain_corner(g.x, g.y + 1),
            self.mountain_corner(g.x + 1, g.y + 1),
        ];
        if mf.iter().all(|&m| m <= 0.0) {
            return (h.ceil() as i32 - 1).min(HEIGHT - 1);
        }
        let corner = |gx: i32, gz: i32| mf[((gx - g.x) + 2 * (gz - g.y)) as usize];
        let top = ((h + MOUNTAIN_AMPLITUDE).ceil() as i32).min(HEIGHT - 1);
        (0..=top)
            .rev()
            .find(|&y| {
                let p = sample(
                    |gx, gy, gz| self.mountain_value(corner(gx, gz), gx, gy, gz),
                    IVec3::new(x, y, z),
                    MOUNTAIN_GRID,
                );
                (y as f32) < h + p
            })
            .unwrap_or(0)
    }

    pub fn column(&self, pos: IVec2) -> Column {
        let mut c = Column::empty(pos);
        let (x0, z0) = (pos.x * 16, pos.y * 16);
        // Per cell: climate, height, biome.
        let cells: Vec<(Climate, f32, Biome)> = (0..256)
            .map(|i| {
                let (x, z) = (x0 + i % 16, z0 + i / 16);
                let cl = self.climate(x, z);
                let h = self.height(x, z, &cl);
                (cl, h, self.biome(&cl, h))
            })
            .collect();
        // Mountain field on a 5×17×5 grid, only where mountains are.
        let (gx0, gz0) = (x0 / MOUNTAIN_GRID.x, z0 / MOUNTAIN_GRID.z);
        let mf: Vec<f32> = (0..25)
            .map(|i| self.mountain_corner(gx0 + i % 5, gz0 + i / 5))
            .collect();
        let mountains = mf.iter().any(|&m| m > 0.0);
        let gy_count = HEIGHT / MOUNTAIN_GRID.y + 1;
        let mgrid: Vec<f32> = if mountains {
            (0..25 * gy_count)
                .map(|i| {
                    let (gx, gz, gy) = (i % 5, i / 5 % 5, i / 25);
                    self.mountain_value(mf[(gx + gz * 5) as usize], gx0 + gx, gy, gz0 + gz)
                })
                .collect()
        } else {
            Vec::new()
        };
        let mount =
            |gx: i32, gy: i32, gz: i32| mgrid[((gx - gx0) + (gz - gz0) * 5 + gy * 25) as usize];
        // Terrain and surface layers.
        let mut tops = [0i32; 256];
        for (i, (_, h, biome)) in cells.iter().enumerate() {
            let (lx, lz) = (i as i32 % 16, i as i32 / 16);
            let (x, z) = (x0 + lx, z0 + lz);
            let solid = |y: i32| {
                let p = if mountains {
                    sample(mount, IVec3::new(x, y, z), MOUNTAIN_GRID)
                } else {
                    0.0
                };
                (y as f32) < h + p
            };
            let ymax = if mountains {
                ((h + MOUNTAIN_AMPLITUDE).ceil() as i32).min(HEIGHT - 1)
            } else {
                (h.ceil() as i32 - 1).min(HEIGHT - 1)
            };
            let mut top = 0;
            let mut layers: Option<Surface> = None;
            let mut depth = 0;
            let mut above_air = true;
            for y in (1..=ymax).rev() {
                if !solid(y) {
                    above_air = true;
                    continue;
                }
                if above_air {
                    if top == 0 {
                        top = y;
                    }
                    layers = Some(self.surface(*biome, x, z, y));
                    depth = 0;
                    above_air = false;
                }
                c.put(lx, y, lz, layers.map_or(STONE, |s| s.block(depth)));
                depth += 1;
            }
            tops[i] = top;
            c.put(lx, 0, lz, BEDROCK);
            let r = noise::hash(self.seed ^ 0xbed, x, z);
            if r.is_multiple_of(2) {
                c.put(lx, 1, lz, BEDROCK);
            }
            if r.is_multiple_of(5) {
                c.put(lx, 2, lz, BEDROCK);
            }
        }
        self.carve_caves(&mut c, pos, &cells, &tops);
        // Water up to sea level above low ground, frozen in the cold.
        for (i, (cl, _, _)) in cells.iter().enumerate() {
            let (lx, lz) = (i as i32 % 16, i as i32 / 16);
            for y in tops[i] + 1..=SEA_LEVEL {
                let ice = y == SEA_LEVEL && cl.temperature < -0.5;
                c.put(lx, y, lz, if ice { ICE } else { WATER });
            }
        }
        self.place_ores(&mut c, pos);
        self.place_plants(&mut c, pos, &cells, &tops);
        self.place_trees(&mut c, pos);
        c
    }

    fn carve_caves(
        &self,
        c: &mut Column,
        pos: IVec2,
        cells: &[(Climate, f32, Biome)],
        tops: &[i32; 256],
    ) {
        let (x0, z0) = (pos.x * 16, pos.y * 16);
        let top_max = tops.iter().copied().max().unwrap_or(0);
        let gy_count = top_max / CAVE_GRID.y + 2;
        let (gx0, gz0) = (x0 / CAVE_GRID.x, z0 / CAVE_GRID.z);
        let grids: [Vec<f32>; 3] = std::array::from_fn(|k| {
            (0..25 * gy_count)
                .map(|i| self.cave_value(k, gx0 + i % 5, i / 25, gz0 + i / 5 % 5))
                .collect()
        });
        for (i, (_, h, _)) in cells.iter().enumerate() {
            let (lx, lz) = (i as i32 % 16, i as i32 / 16);
            let top = tops[i];
            let entrance = self.entrance(x0 + lx, z0 + lz);
            for y in 1..=top {
                let id = c.get(lx, y, lz);
                if id == 0 || id == BEDROCK {
                    continue;
                }
                let p = IVec3::new(x0 + lx, y, z0 + lz);
                let v = std::array::from_fn(|k| {
                    let g = &grids[k];
                    sample(
                        |gx, gy, gz| g[((gx - gx0) + (gz - gz0) * 5 + gy * 25) as usize],
                        p,
                        CAVE_GRID,
                    )
                });
                if self.carve(v, y, top, *h, entrance) {
                    // Deep caves hold lava lakes.
                    c.put(lx, y, lz, if y <= 10 { LAVA } else { 0 });
                }
            }
        }
    }

    fn place_ores(&self, c: &mut Column, pos: IVec2) {
        let (x0, z0) = (pos.x * 16, pos.y * 16);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let from = pos + IVec2::new(dx, dz);
                for (k, &(ore, min, max, attempts, size)) in ORES.iter().enumerate() {
                    for i in 0..attempts {
                        let mut h = noise::hash3(
                            self.seed ^ 0x0e5,
                            from.x,
                            from.y,
                            (k * 64) as i32 + i as i32,
                        );
                        let centre = IVec3::new(
                            from.x * 16 + (h & 15) as i32,
                            min + ((h >> 8) % (max - min + 1) as u64) as i32,
                            from.y * 16 + (h >> 4 & 15) as i32,
                        );
                        // Skip veins that cannot reach this column.
                        if centre.x + VEIN < x0
                            || centre.x - VEIN >= x0 + 16
                            || centre.z + VEIN < z0
                            || centre.z - VEIN >= z0 + 16
                        {
                            continue;
                        }
                        let mut p = centre;
                        for _ in 0..size {
                            let (lx, lz) = (p.x - x0, p.z - z0);
                            if (0..16).contains(&lx)
                                && (0..16).contains(&lz)
                                && (1..HEIGHT).contains(&p.y)
                                && c.get(lx, p.y, lz) == STONE
                            {
                                c.put(lx, p.y, lz, ore);
                            }
                            h = noise::hash(h, 0x51, 0x7);
                            let axis = (h % 3) as usize;
                            let step = if h >> 8 & 1 == 0 { 1 } else { -1 };
                            p[axis] += step;
                            if (p[axis] - centre[axis]).abs() > VEIN {
                                p[axis] -= 2 * step;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Grass, flowers, cactus, sugar cane and snow inside this column.
    fn place_plants(
        &self,
        c: &mut Column,
        pos: IVec2,
        cells: &[(Climate, f32, Biome)],
        tops: &[i32; 256],
    ) {
        let (x0, z0) = (pos.x * 16, pos.y * 16);
        for (i, (_, _, biome)) in cells.iter().enumerate() {
            let (lx, lz) = (i as i32 % 16, i as i32 / 16);
            let (x, z) = (x0 + lx, z0 + lz);
            let top = tops[i];
            if top + 4 >= HEIGHT || c.get(lx, top + 1, lz) != 0 {
                continue;
            }
            let ground = c.get(lx, top, lz);
            let h = noise::hash(self.seed ^ 0x91a, x, z);
            let roll = noise::unit(h);
            let flower = if h >> 8 & 1 == 0 { DANDELION } else { POPPY };
            match biome {
                Biome::Desert if ground == SAND && roll < 0.006 => {
                    for dy in 1..=1 + (h >> 12) as i32 % 3 {
                        c.put(lx, top + dy, lz, CACTUS);
                    }
                    continue;
                }
                Biome::SnowyPlains if ground == SNOWY_GRASS => {
                    c.put(lx, top + 1, lz, SNOW_LAYER);
                    continue;
                }
                _ => {}
            }
            // Sugar cane on the shore, next to water.
            if top == SEA_LEVEL && matches!(ground, GRASS | SAND | DIRT) && roll > 0.82 {
                let wet = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dz)| {
                    let (nx, nz) = (lx + dx, lz + dz);
                    let ntop = if (0..16).contains(&nx) && (0..16).contains(&nz) {
                        tops[(nx + nz * 16) as usize]
                    } else {
                        let (x, z) = (x0 + nx, z0 + nz);
                        let cl = self.climate(x, z);
                        self.top_at(x, z, self.height(x, z, &cl))
                    };
                    ntop < SEA_LEVEL
                });
                if wet {
                    for dy in 1..=1 + (h >> 16) as i32 % 3 {
                        c.put(lx, top + dy, lz, SUGAR_CANE);
                    }
                    continue;
                }
            }
            if ground != GRASS {
                continue;
            }
            let (grass, flowers) = match biome {
                Biome::Plains => (0.12, 0.025),
                Biome::Forest | Biome::BirchForest => (0.07, 0.015),
                Biome::Taiga => (0.07, 0.0),
                Biome::Mountains => (0.06, 0.005),
                _ => (0.0, 0.0),
            };
            if roll < grass {
                c.put(lx, top + 1, lz, TALL_GRASS);
            } else if roll < grass + flowers {
                c.put(lx, top + 1, lz, flower);
            }
        }
    }

    fn tree_at(&self, cell: IVec2) -> Option<Tree> {
        let h = noise::hash(self.seed ^ 0x7ee, cell.x, cell.y);
        let roll = noise::unit(h >> 20);
        if roll >= 0.9 {
            return None;
        }
        let x = cell.x * TREE_CELL + (h % TREE_CELL as u64) as i32;
        let z = cell.y * TREE_CELL + (h >> 8 & 0xff) as i32 % TREE_CELL;
        let cl = self.climate(x, z);
        let height = self.height(x, z, &cl);
        let biome = self.biome(&cl, height);
        let pick = noise::unit(h >> 40);
        let (chance, kind) = match biome {
            Biome::Forest if pick < 0.08 => (0.9, TreeKind::BigOak),
            Biome::Forest if pick < 0.3 => (0.9, TreeKind::Birch),
            Biome::Forest => (0.9, TreeKind::Oak),
            Biome::BirchForest if pick < 0.1 => (0.8, TreeKind::Oak),
            Biome::BirchForest => (0.8, TreeKind::Birch),
            Biome::Taiga => (0.85, TreeKind::Spruce),
            Biome::Plains if pick < 0.2 => (0.05, TreeKind::BigOak),
            Biome::Plains => (0.05, TreeKind::Oak),
            Biome::SnowyPlains => (0.07, TreeKind::Spruce),
            Biome::Mountains => (0.2, TreeKind::Spruce),
            _ => return None,
        };
        if roll >= chance {
            return None;
        }
        // Keep the spawn point clear.
        let spawn = self.spawn();
        if (x - spawn.x).abs() <= CANOPY && (z - spawn.z).abs() <= CANOPY {
            return None;
        }
        let ground = self.top_at(x, z, height);
        if ground < SEA_LEVEL
            || !matches!(self.surface(biome, x, z, ground).top, GRASS | SNOWY_GRASS)
        {
            return None;
        }
        if self.carved_at(IVec3::new(x, ground, z), ground, height) {
            return None;
        }
        let trunk = match kind {
            TreeKind::Oak => 4 + (h >> 30) as i32 % 3,
            TreeKind::Birch => 5 + (h >> 30) as i32 % 3,
            TreeKind::BigOak => 6 + (h >> 30) as i32 % 3,
            TreeKind::Spruce => 6 + (h >> 30) as i32 % 4,
        };
        (ground + trunk + 4 < HEIGHT).then_some(Tree {
            kind,
            x,
            z,
            ground,
            trunk,
        })
    }

    fn leaves(&self, t: &Tree, mut put: impl FnMut(IVec3, u8)) {
        let top = t.ground + t.trunk;
        let jitter = |p: IVec3| noise::hash3(self.seed ^ 0x1ea, p.x, p.y, p.z) & 1 == 0;
        match t.kind {
            TreeKind::Oak | TreeKind::Birch => {
                let leaf = if t.kind == TreeKind::Oak {
                    OAK_LEAVES
                } else {
                    BIRCH_LEAVES
                };
                for y in top - 2..=top + 1 {
                    let r: i32 = if y < top { 2 } else { 1 };
                    for dz in -r..=r {
                        for dx in -r..=r {
                            let corner = dx.abs() == r && dz.abs() == r;
                            let p = IVec3::new(t.x + dx, y, t.z + dz);
                            if corner && (y >= top || jitter(p)) {
                                continue;
                            }
                            put(p, leaf);
                        }
                    }
                }
            }
            TreeKind::BigOak => {
                let centre = top - 1;
                for dy in -2..=3 {
                    for dz in -CANOPY..=CANOPY {
                        for dx in -CANOPY..=CANOPY {
                            let d = (dx * dx + dz * dz) as f32 + (dy as f32 * 1.5).powi(2);
                            let p = IVec3::new(t.x + dx, centre + dy, t.z + dz);
                            if d <= 8.5 || (d <= 11.0 && jitter(p)) {
                                put(p, OAK_LEAVES);
                            }
                        }
                    }
                }
            }
            TreeKind::Spruce => {
                put(IVec3::new(t.x, top + 1, t.z), SPRUCE_LEAVES);
                for (i, y) in (t.ground + 3..=top).rev().enumerate() {
                    let r = match i {
                        0 => 1,
                        i if i % 2 == 1 => 1 + i32::from(i > 4),
                        _ => 2 + i32::from(i > 5),
                    };
                    for dz in -r..=r {
                        for dx in -r..=r {
                            if dx.abs() + dz.abs() <= r + r / 2 && (dx, dz) != (0, 0) {
                                put(IVec3::new(t.x + dx, y, t.z + dz), SPRUCE_LEAVES);
                            }
                        }
                    }
                }
            }
        }
    }

    fn place_trees(&self, c: &mut Column, pos: IVec2) {
        let (x0, z0) = (pos.x * 16, pos.y * 16);
        let cells = |from: i32, to: i32| from.div_euclid(TREE_CELL)..=to.div_euclid(TREE_CELL);
        let mut trees = Vec::new();
        for cz in cells(z0 - CANOPY, z0 + 15 + CANOPY) {
            for cx in cells(x0 - CANOPY, x0 + 15 + CANOPY) {
                if let Some(t) = self.tree_at(IVec2::new(cx, cz)) {
                    trees.push(t);
                }
            }
        }
        let local = |p: IVec3| {
            let (lx, lz) = (p.x - x0, p.z - z0);
            ((0..16).contains(&lx) && (0..16).contains(&lz) && (0..HEIGHT).contains(&p.y))
                .then_some((lx, lz))
        };
        // Leaves only fill air and trunks always win, which makes the result
        // independent of the order trees are visited.
        for t in &trees {
            self.leaves(t, |p, leaf| {
                if let Some((lx, lz)) = local(p)
                    && c.get(lx, p.y, lz) == 0
                {
                    c.put(lx, p.y, lz, leaf);
                }
            });
        }
        for t in &trees {
            let log = match t.kind {
                TreeKind::Oak | TreeKind::BigOak => OAK_LOG,
                TreeKind::Birch => BIRCH_LOG,
                TreeKind::Spruce => SPRUCE_LOG,
            };
            for y in t.ground + 1..=t.ground + t.trunk {
                if let Some((lx, lz)) = local(IVec3::new(t.x, y, t.z)) {
                    c.put(lx, y, lz, log);
                }
            }
        }
    }

    /// Ground block where a player can spawn at (x, z), if any.
    fn land(&self, x: i32, z: i32) -> Option<i32> {
        let cl = self.climate(x, z);
        let h = self.height(x, z, &cl);
        let biome = self.biome(&cl, h);
        if matches!(biome, Biome::Ocean | Biome::Mountains) {
            return None;
        }
        let top = self.top_at(x, z, h);
        let ground = self.surface(biome, x, z, top).top;
        (top > SEA_LEVEL
            && matches!(ground, GRASS | SAND | SNOWY_GRASS)
            && !self.carved_at(IVec3::new(x, top, z), top, h))
        .then_some(top)
    }
    /// Feet block of the spawn: the land closest to the origin.
    pub fn spawn(&self) -> IVec3 {
        *self.spawn.get_or_init(|| {
            for r in 0..=128 {
                for i in 0..(8 * r).max(1) {
                    // Walk the square ring of radius r (steps of 8 blocks).
                    let side = 2 * r;
                    let (x, z) = if r == 0 {
                        (0, 0)
                    } else {
                        match i / side {
                            0 => (-r + i % side, -r),
                            1 => (r, -r + i % side),
                            2 => (r - i % side, r),
                            _ => (-r, r - i % side),
                        }
                    };
                    if let Some(top) = self.land(x * 8, z * 8) {
                        return IVec3::new(x * 8, top + 1, z * 8);
                    }
                }
            }
            let cl = self.climate(0, 0);
            IVec3::new(0, self.top_at(0, 0, self.height(0, 0, &cl)) + 1, 0)
        })
    }

    /// Top-down map, `scale` blocks per pixel, as RGB (host tool).
    pub fn map_rgb(&self, center: IVec2, size: usize, scale: i32) -> Vec<u8> {
        let mut out = Vec::with_capacity(size * size * 3);
        let half = size as i32 / 2;
        for py in 0..size as i32 {
            for px in 0..size as i32 {
                let (x, z) = (
                    center.x + (px - half) * scale,
                    center.y + (py - half) * scale,
                );
                let cl = self.climate(x, z);
                let h = self.height(x, z, &cl);
                let mut rgb = self.biome(&cl, h).color().map(f32::from);
                let shade = if h < SEA_LEVEL as f32 {
                    0.55 + 0.45 * (h / SEA_LEVEL as f32)
                } else {
                    0.75 + (h - SEA_LEVEL as f32) / 120.0
                };
                for v in &mut rgb {
                    *v = (*v * shade).min(255.0);
                }
                out.extend(rgb.map(|v| v as u8));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column_tops(g: &Generator, pos: IVec2) -> Vec<i32> {
        // Reproduces the terrain pass: highest block before decoration.
        (0..256)
            .map(|i| {
                let (x, z) = (pos.x * 16 + i % 16, pos.y * 16 + i / 16);
                let cl = g.climate(x, z);
                g.top_at(x, z, g.height(x, z, &cl))
            })
            .collect()
    }

    /// Finds a column whose biome is `biome` (searching outward).
    fn find(g: &Generator, biome: Biome) -> IVec2 {
        for r in 0..400 {
            for (dx, dz) in [(r, 0), (-r, 0), (0, r), (0, -r), (r, r), (-r, -r)] {
                let p = IVec2::new(dx * 16, dz * 16);
                if g.biome_at(p.x + 8, p.y + 8) == biome {
                    return p / 16;
                }
            }
        }
        panic!("no {biome:?} found");
    }

    #[test]
    fn every_biome_appears_within_4096_blocks() {
        let g = Generator::new(42);
        let mut seen = std::collections::HashMap::new();
        for z in (-2048..2048).step_by(32) {
            for x in (-2048..2048).step_by(32) {
                *seen.entry(g.biome_at(x, z)).or_insert(0usize) += 1;
            }
        }
        for b in Biome::ALL {
            assert!(
                seen.get(&b).copied().unwrap_or(0) > 0,
                "{b:?} missing: {seen:?}"
            );
        }
        let ocean = seen[&Biome::Ocean] as f32 / (128.0 * 128.0);
        assert!((0.1..0.6).contains(&ocean), "ocean share {ocean}: {seen:?}");
    }

    #[test]
    fn deterministic_and_features_continue_across_borders() {
        let g = Generator::new(42);
        let forest = find(&g, Biome::Forest);
        let a = g.column(forest);
        assert_eq!(a.encode(), generate(42, forest).encode());
        assert_ne!(a.encode(), generate(43, forest).encode());
        // A tree near the east border of a column puts leaves into the next.
        let mut checked = 0;
        'search: for cz in -200..200 {
            for cx in -200..200 {
                let Some(t) = g.tree_at(IVec2::new(cx, cz)) else {
                    continue;
                };
                if t.x.rem_euclid(16) != 15 || matches!(t.kind, TreeKind::Spruce) {
                    continue;
                }
                let own = g.column(IVec2::new(t.x.div_euclid(16), t.z.div_euclid(16)));
                let next = g.column(IVec2::new(t.x.div_euclid(16) + 1, t.z.div_euclid(16)));
                let lz = t.z.rem_euclid(16);
                let log = own.get(15, t.ground + 1, lz);
                assert!(matches!(log, OAK_LOG | BIRCH_LOG), "trunk {log}");
                let y = t.ground + t.trunk - 1;
                let leaf = next.get(0, y, lz);
                assert!(
                    matches!(leaf, OAK_LEAVES | BIRCH_LEAVES) || leaf != 0,
                    "canopy {leaf} at {t:?}"
                );
                checked += 1;
                if checked >= 3 {
                    break 'search;
                }
            }
        }
        assert!(checked >= 3, "no border trees found");
    }

    #[test]
    fn point_queries_match_column_terrain_in_mountains() {
        let g = Generator::new(42);
        let pos = find(&g, Biome::Mountains);
        // Generate the raw terrain pass through the column generator and
        // compare against the pure per-block query.
        let c = g.column(pos);
        let tops = column_tops(&g, pos);
        for (i, &top) in tops.iter().enumerate() {
            let (lx, lz) = (i as i32 % 16, i as i32 / 16);
            for y in top + 1..HEIGHT {
                let id = c.get(lx, y, lz);
                assert!(
                    !matches!(id, STONE | DIRT | GRASS | GRAVEL | SNOW),
                    "terrain above the queried top at {lx},{y},{lz}: {id}"
                );
            }
        }
        let peak = tops.iter().copied().max().unwrap();
        assert!(peak > 85, "mountains rise: {peak}");
    }

    #[test]
    fn ores_follow_depth_and_caves_spare_the_sea_floor() {
        let g = Generator::new(7);
        let mut counts = std::collections::HashMap::new();
        let mut deepest_gold = 0;
        let mut highest_diamond = 0;
        let mut caves = 0;
        let mut lava = 0;
        for cz in 0..4 {
            for cx in 0..4 {
                let c = g.column(IVec2::new(cx, cz));
                for y in 0..HEIGHT {
                    for z in 0..16 {
                        for x in 0..16 {
                            let id = c.get(x, y, z);
                            *counts.entry(id).or_insert(0usize) += 1;
                            if id == GOLD_ORE {
                                deepest_gold = deepest_gold.max(y);
                            }
                            if id == DIAMOND_ORE {
                                highest_diamond = highest_diamond.max(y);
                            }
                            if id == LAVA && y <= 10 {
                                lava += 1;
                            }
                            if id == 0 && y < 50 {
                                caves += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(
            counts.get(&COAL_ORE).copied().unwrap_or(0) > 100,
            "{counts:?}"
        );
        assert!(counts.get(&IRON_ORE).copied().unwrap_or(0) > 50);
        assert!(deepest_gold <= 32 + VEIN, "gold at {deepest_gold}");
        assert!(highest_diamond <= 16 + VEIN, "diamond at {highest_diamond}");
        assert!(caves > 500, "caves: {caves} air cells below y 50");
        assert!(lava + caves > 0);
        // Ocean floors: the five blocks under the water are never carved.
        let ocean = find(&g, Biome::Ocean);
        let c = g.column(ocean);
        let tops = column_tops(&g, ocean);
        for (i, &top) in tops.iter().enumerate() {
            if top >= SEA_LEVEL - 1 {
                continue;
            }
            let (lx, lz) = (i as i32 % 16, i as i32 / 16);
            assert!(
                matches!(c.get(lx, top + 1, lz), WATER | ICE),
                "water above the floor"
            );
            for y in (top - 4).max(1)..=top {
                assert_ne!(
                    c.get(lx, y, lz),
                    0,
                    "hole in the sea floor at {lx},{y},{lz}"
                );
            }
        }
    }

    #[test]
    fn spawn_is_on_dry_land_with_headroom() {
        for seed in [1, 42, 9_999] {
            let s = spawn(seed);
            let p = s.floor().as_ivec3();
            let c = generate(seed, IVec2::new(p.x.div_euclid(16), p.z.div_euclid(16)));
            let (lx, lz) = (p.x.rem_euclid(16), p.z.rem_euclid(16));
            let ground = c.get(lx, p.y - 1, lz);
            assert!(
                matches!(ground, GRASS | SAND | SNOWY_GRASS),
                "seed {seed}: ground {ground}"
            );
            assert!(!solid(c.get(lx, p.y, lz)) || c.get(lx, p.y, lz) == CACTUS);
            assert!(!liquid(c.get(lx, p.y, lz)));
            assert!(p.y > SEA_LEVEL);
            assert_eq!(c.get(lx, 0, lz), BEDROCK);
        }
    }

    #[test]
    #[ignore = "timing probe"]
    fn generation_timing() {
        let g = Generator::new(42);
        for biome in [Biome::Plains, Biome::Forest, Biome::Mountains, Biome::Ocean] {
            let p = find(&g, biome);
            let start = std::time::Instant::now();
            let n = 30;
            for i in 0..n {
                std::hint::black_box(g.column(p + IVec2::new(i % 5, i / 5)));
            }
            eprintln!(
                "{biome:?}: {:.2} ms/column",
                start.elapsed().as_secs_f64() * 1000.0 / n as f64
            );
        }
    }

    #[test]
    fn caves_reach_the_surface_only_at_entrances() {
        let g = Generator::new(42);
        let (mut open, mut sealed_air, mut entrances) = (0, 0, 0);
        for cz in -3..3 {
            for cx in -3..3 {
                let pos = IVec2::new(cx, cz);
                let c = g.column(pos);
                let tops = column_tops(&g, pos);
                for (i, &top) in tops.iter().enumerate() {
                    let (lx, lz) = (i as i32 % 16, i as i32 / 16);
                    let (x, z) = (pos.x * 16 + lx, pos.y * 16 + lz);
                    let entrance = g.entrance(x, z);
                    entrances += usize::from(entrance);
                    for y in (top - 6).max(1)..top {
                        if c.get(lx, y, lz) == 0 {
                            if entrance {
                                open += 1;
                            } else {
                                sealed_air += 1;
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(sealed_air, 0, "caves broke through outside entrances");
        assert!(
            entrances > 0 && entrances < 9216 / 3,
            "{entrances} entrance cells"
        );
        assert!(open + entrances > 0);
    }
}
