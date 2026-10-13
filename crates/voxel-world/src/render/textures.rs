//! Procedural 16×16 pixel-art tiles for every [`Tex`].
//!
//! Everything is generated from palettes, wrapping noise, cellular patterns
//! and small ASCII sprites, so tiles repeat seamlessly and no external image
//! is needed. Output is deterministic.
use crate::world::block::{TEX_COUNT, Tex};

pub const TILE: usize = 16;
pub type Rgba = [u8; 4];

#[derive(Clone)]
pub struct Img(pub [Rgba; TILE * TILE]);

type Rgb = [u8; 3];

fn hash(seed: u32, x: i32, y: i32) -> f32 {
    let mut v = seed
        .wrapping_mul(0x9e37_79b9)
        .wrapping_add((x as u32).wrapping_mul(0x85eb_ca6b))
        .wrapping_add((y as u32).wrapping_mul(0xc2b2_ae35));
    v ^= v >> 15;
    v = v.wrapping_mul(0x2c1b_3c6d);
    v ^= v >> 12;
    v = v.wrapping_mul(0x297a_2d39);
    v ^= v >> 15;
    (v >> 8) as f32 / 16_777_216.0
}

/// Value noise that wraps every 16 pixels; `cells` must divide 16.
fn vnoise(seed: u32, x: f32, y: f32, cells: i32) -> f32 {
    vnoise2(seed, x, y, cells, cells)
}
/// Like `vnoise` with separate cell counts per axis, for streaks.
fn vnoise2(seed: u32, x: f32, y: f32, cx: i32, cy: i32) -> f32 {
    let (fx, fy) = (x * cx as f32 / TILE as f32, y * cy as f32 / TILE as f32);
    let (ix, iy) = (fx.floor() as i32, fy.floor() as i32);
    let (tx, ty) = (fx - ix as f32, fy - iy as f32);
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let h = |x: i32, y: i32| hash(seed, x.rem_euclid(cx), y.rem_euclid(cy));
    let a = h(ix, iy) + (h(ix + 1, iy) - h(ix, iy)) * s(tx);
    let b = h(ix, iy + 1) + (h(ix + 1, iy + 1) - h(ix, iy + 1)) * s(tx);
    a + (b - a) * s(ty)
}

fn tone(c: Rgb, f: f32) -> Rgba {
    let ch = |v: u8| (f32::from(v) * f).round().clamp(0.0, 255.0) as u8;
    [ch(c[0]), ch(c[1]), ch(c[2]), 255]
}
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}
fn alpha(mut c: Rgba, a: u8) -> Rgba {
    c[3] = a;
    c
}

impl Img {
    fn clear() -> Self {
        Self([[0; 4]; TILE * TILE])
    }
    fn from_fn(mut f: impl FnMut(i32, i32) -> Rgba) -> Self {
        let mut img = Self::clear();
        for y in 0..16 {
            for x in 0..16 {
                img.0[(y * 16 + x) as usize] = f(x, y);
            }
        }
        img
    }
    pub fn get(&self, x: i32, y: i32) -> Rgba {
        self.0[(y.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize]
    }
    fn set(&mut self, x: i32, y: i32, c: Rgba) {
        if (0..16).contains(&x) && (0..16).contains(&y) {
            self.0[(y * 16 + x) as usize] = c;
        }
    }
    fn map(&mut self, mut f: impl FnMut(i32, i32, Rgba) -> Rgba) {
        for y in 0..16 {
            for x in 0..16 {
                let c = self.get(x, y);
                self.set(x, y, f(x, y, c));
            }
        }
    }
    /// Draws an ASCII sprite; '.' and ' ' are transparent.
    fn sprite(&mut self, rows: &[&str], palette: &[(char, Rgba)]) {
        for (y, row) in rows.iter().enumerate() {
            for (x, ch) in row.chars().enumerate() {
                if let Some((_, c)) = palette.iter().find(|(k, _)| *k == ch) {
                    self.set(x as i32, y as i32, *c);
                }
            }
        }
    }
    fn bevel(&mut self, light: f32, dark: f32) {
        self.map(|x, y, c| {
            let f = if x == 0 || y == 0 {
                light
            } else if x == 15 || y == 15 {
                dark
            } else {
                1.0
            };
            [
                (f32::from(c[0]) * f).min(255.0) as u8,
                (f32::from(c[1]) * f).min(255.0) as u8,
                (f32::from(c[2]) * f).min(255.0) as u8,
                c[3],
            ]
        });
    }
}

/// Per-pixel brightness jitter around a base colour.
fn speckle(base: Rgb, seed: u32, amount: f32) -> Img {
    Img::from_fn(|x, y| tone(base, 1.0 + amount * (hash(seed, x, y) * 2.0 - 1.0)))
}

/// Wrapping cellular noise: (nearest cell, distance, second distance).
struct Cells(Vec<(f32, f32)>);
impl Cells {
    fn new(seed: u32, n: usize) -> Self {
        Self(
            (0..n)
                .map(|i| {
                    (
                        hash(seed, i as i32, 1) * 16.0,
                        hash(seed, i as i32, 2) * 16.0,
                    )
                })
                .collect(),
        )
    }
    fn at(&self, x: i32, y: i32) -> (usize, f32, f32) {
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let mut best = (0, f32::MAX, f32::MAX);
        for (i, &(cx, cy)) in self.0.iter().enumerate() {
            let dx = (px - cx).abs().min(16.0 - (px - cx).abs());
            let dy = (py - cy).abs().min(16.0 - (py - cy).abs());
            let d = (dx * dx + dy * dy).sqrt();
            if d < best.1 {
                best = (i, d, best.1);
            } else if d < best.2 {
                best.2 = d;
            }
        }
        best
    }
}

const DIRT: Rgb = [134, 96, 67];
const GRASS: Rgb = [104, 168, 62];
const STONE: Rgb = [128, 128, 128];
const SAND: Rgb = [219, 207, 163];
const SNOW: Rgb = [241, 249, 252];

fn dirt(seed: u32, base: Rgb) -> Img {
    Img::from_fn(|x, y| {
        let mut f = 0.9 + 0.2 * hash(seed, x, y);
        if hash(seed + 1, x, y) > 0.86 {
            f *= 0.78;
        }
        if hash(seed + 2, x, y) > 0.94 {
            return tone(mix(base, [170, 128, 92], 0.6), 1.0);
        }
        tone(base, f)
    })
}

fn grass_top() -> Img {
    Img::from_fn(|x, y| {
        let h = hash(11, x, y);
        let blob = vnoise(12, x as f32, y as f32, 4);
        let c = if h > 0.78 {
            [128, 190, 78]
        } else if h < 0.16 {
            [78, 130, 46]
        } else {
            GRASS
        };
        tone(c, 0.9 + 0.2 * blob)
    })
}

/// Dirt with an uneven fringe of `top` hanging from the upper edge.
fn fringe(top: Rgb, seed: u32) -> Img {
    let mut img = dirt(21, DIRT);
    for x in 0..16 {
        let len = 3 + i32::from(hash(seed, x, 0) > 0.45) + 2 * i32::from(hash(seed, x, 1) > 0.8);
        for y in 0..len {
            let f = 0.88 + 0.22 * hash(seed + 5, x, y);
            let edge = if y == len - 1 { 0.82 } else { 1.0 };
            img.set(x, y, tone(top, f * edge));
        }
    }
    img
}

fn stone() -> Img {
    Img::from_fn(|x, y| {
        let n = vnoise(31, x as f32, y as f32, 4);
        let streak = vnoise2(32, x as f32, y as f32, 8, 2);
        let mut f = 0.88 + 0.2 * n + 0.08 * (hash(33, x, y) - 0.5);
        if (0.46..0.52).contains(&streak) {
            f *= 0.8;
        }
        tone(STONE, f)
    })
}

fn cobble(seed: u32) -> Img {
    let cells = Cells::new(seed, 9);
    Img::from_fn(|x, y| {
        let (i, d1, d2) = cells.at(x, y);
        if d2 - d1 < 1.1 {
            return tone([74, 74, 74], 0.9 + 0.2 * hash(seed, x, y));
        }
        let f = 0.78 + 0.32 * hash(seed + 9, i as i32, 0);
        let (cx, cy) = cells.0[i];
        let lit = if (y as f32 + 0.5) < cy - 1.0 && (x as f32 + 0.5) < cx {
            1.1
        } else {
            1.0
        };
        tone(
            [122, 122, 122],
            f * lit * (0.94 + 0.12 * hash(seed + 3, x, y)),
        )
    })
}

fn bedrock() -> Img {
    let cells = Cells::new(71, 14);
    Img::from_fn(|x, y| {
        let (i, d1, d2) = cells.at(x, y);
        let shade = 0.25 + 0.6 * hash(72, i as i32, 0);
        let edge = if d2 - d1 < 0.9 { 0.6 } else { 1.0 };
        tone([120, 120, 124], shade * edge * (0.9 + 0.2 * hash(73, x, y)))
    })
}

fn sand(seed: u32, base: Rgb) -> Img {
    Img::from_fn(|x, y| {
        if hash(seed, x, y) > 0.9 {
            tone(base, 0.86)
        } else {
            tone(base, 0.95 + 0.1 * hash(seed + 1, x, y))
        }
    })
}

fn sandstone(kind: u8) -> Img {
    let base = [216, 201, 152];
    Img::from_fn(|x, y| {
        let n = hash(41, x, y);
        match kind {
            0 => tone(base, 0.96 + 0.08 * n),
            2 => tone(base, 0.84 + 0.2 * n),
            _ => {
                let mut f = 0.95 + 0.07 * n;
                if y < 3 {
                    f *= 1.06;
                } else if y > 12 {
                    f *= 0.86;
                } else if (y + (vnoise(42, x as f32, 0.0, 4) * 2.0) as i32) % 3 == 0 {
                    f *= 0.92;
                }
                if y == 3 || y == 12 {
                    f *= 0.85;
                }
                tone(base, f)
            }
        }
    })
}

fn gravel() -> Img {
    let cells = Cells::new(51, 20);
    let colors = [
        [143, 136, 131],
        [112, 106, 102],
        [163, 152, 141],
        [124, 112, 104],
        [92, 90, 88],
    ];
    Img::from_fn(|x, y| {
        let (i, d1, d2) = cells.at(x, y);
        let c = colors[(hash(52, i as i32, 0) * 5.0) as usize % 5];
        let edge = if d2 - d1 < 0.8 { 0.72 } else { 1.0 };
        tone(c, edge * (0.92 + 0.14 * hash(53, x, y)))
    })
}

fn smooth(base: Rgb, seed: u32, amount: f32) -> Img {
    Img::from_fn(|x, y| {
        let n = vnoise(seed, x as f32, y as f32, 4);
        tone(
            base,
            1.0 - amount + 2.0 * amount * n + 0.03 * (hash(seed, x, y) - 0.5),
        )
    })
}

fn ice() -> Img {
    let mut img = Img::from_fn(|x, y| {
        let n = vnoise(61, x as f32, y as f32, 4);
        alpha(tone([150, 190, 250], 0.95 + 0.1 * n), 185)
    });
    for (x, y) in [
        (2, 3),
        (3, 4),
        (4, 5),
        (5, 6),
        (10, 9),
        (11, 10),
        (12, 11),
        (9, 2),
        (10, 3),
    ] {
        img.set(x, y, [235, 245, 255, 215]);
    }
    img
}

fn water() -> Img {
    Img::from_fn(|x, y| {
        let phase = std::f32::consts::TAU * (x as f32 + 2.0 * y as f32) / 16.0;
        let wave = (phase.sin() * 0.5 + 0.5) * vnoise(81, x as f32, y as f32, 4);
        alpha(tone([48, 92, 214], 0.88 + 0.3 * wave), 175)
    })
}

fn lava() -> Img {
    let cells = Cells::new(91, 8);
    Img::from_fn(|x, y| {
        let (_, d1, d2) = cells.at(x, y);
        let t = (d1 / 4.0).clamp(0.0, 1.0);
        let c = if d2 - d1 < 0.9 {
            [150, 38, 8]
        } else if t < 0.3 {
            [255, 214, 90]
        } else {
            mix([248, 150, 40], [214, 82, 18], (t - 0.3) / 0.7)
        };
        tone(c, 0.95 + 0.1 * hash(92, x, y))
    })
}

fn obsidian() -> Img {
    Img::from_fn(|x, y| {
        let streak = vnoise2(101, x as f32, y as f32, 4, 8);
        if (0.55..0.63).contains(&streak) {
            tone([62, 42, 96], 0.9 + 0.2 * hash(102, x, y))
        } else {
            tone([22, 17, 34], 0.8 + 0.4 * hash(103, x, y))
        }
    })
}

/// Stone with clusters of mineral pixels.
fn ore(seed: u32, color: Rgb) -> Img {
    let mut img = stone();
    let shapes: [&[(i32, i32)]; 3] = [
        &[(0, 0), (1, 0), (0, 1), (1, 1)],
        &[(0, 0), (1, 0), (-1, 1), (0, 1)],
        &[(0, 0), (1, 1), (0, 1), (2, 1), (1, 2)],
    ];
    for k in 0..5 {
        let cx = 1 + (hash(seed, k, 0) * 13.0) as i32;
        let cy = 1 + (hash(seed, k, 1) * 13.0) as i32;
        let shape = shapes[(hash(seed, k, 2) * 3.0) as usize % 3];
        for &(dx, dy) in shape {
            let f = if dx <= 0 && dy == 0 {
                1.15
            } else if dy > 0 && dx > 0 {
                0.78
            } else {
                1.0
            };
            img.set(cx + dx, cy + dy, tone(color, f));
        }
        img.set(cx + 1, cy + 2, tone([70, 70, 70], 1.0));
    }
    img
}

fn log_side(bark: Rgb, seed: u32) -> Img {
    Img::from_fn(|x, y| {
        // Vertical ridges per column, grooves broken up along the trunk.
        let ridge = hash(seed, x, 0);
        let along = vnoise2(seed + 2, x as f32, y as f32, 16, 4);
        let mut f = 0.86 + 0.22 * ridge;
        if ridge < 0.32 && along > 0.22 {
            f *= 0.68;
        }
        tone(
            bark,
            f * (0.92 + 0.16 * along) * (0.95 + 0.1 * hash(seed + 1, x, y)),
        )
    })
}

fn birch_side() -> Img {
    let mut img = speckle([214, 211, 202], 111, 0.05);
    for k in 0..9 {
        let y = (hash(112, k, 0) * 16.0) as i32;
        let x0 = (hash(112, k, 1) * 16.0) as i32;
        let len = 2 + (hash(112, k, 2) * 3.0) as i32;
        for dx in 0..len {
            img.set((x0 + dx) % 16, y, [44, 40, 36, 255]);
        }
    }
    img
}

fn log_top(bark: Rgb, wood: Rgb, seed: u32) -> Img {
    Img::from_fn(|x, y| {
        let d = (x as f32 - 7.5).abs().max((y as f32 - 7.5).abs());
        if d > 6.6 {
            return tone(bark, 0.9 + 0.2 * hash(seed, x, y));
        }
        let ring = (d.round() as i32) % 2 == 0;
        tone(
            wood,
            if ring { 0.86 } else { 1.0 } * (0.95 + 0.1 * hash(seed + 1, x, y)),
        )
    })
}

fn leaves(base: Rgb, seed: u32) -> Img {
    Img::from_fn(|x, y| {
        let h = hash(seed, x, y);
        let n = vnoise(seed + 1, x as f32, y as f32, 4);
        if h > 0.8 {
            // A hole; the colour shows when leaves are drawn opaque.
            return alpha(tone(base, 0.45), 0);
        }
        let f = if h < 0.18 {
            0.75
        } else if h > 0.68 {
            1.18
        } else {
            1.0
        };
        tone(base, f * (0.88 + 0.24 * n))
    })
}

fn planks(base: Rgb, seed: u32) -> Img {
    Img::from_fn(|x, y| {
        let board = y / 4;
        let joint = (board * 5 + 3 + (hash(seed, board, 9) * 4.0) as i32) % 16;
        if y % 4 == 3 || x == joint {
            return tone(base, 0.66);
        }
        let grain = vnoise2(seed + board as u32, x as f32, y as f32, 2, 8);
        tone(
            base,
            (0.9 + 0.16 * grain)
                * (0.96 + 0.06 * hash(seed + 2, x, y))
                * (0.96 + 0.08 * hash(seed + 3, board, 0)),
        )
    })
}

const OAK: Rgb = [162, 130, 78];

fn crafting(kind: u8) -> Img {
    let mut img = planks(OAK, 131);
    let dark = tone([92, 64, 36], 1.0);
    let metal = tone([170, 170, 172], 1.0);
    match kind {
        0 => {
            img.map(|x, y, c| {
                if x == 0 || y == 0 || x == 15 || y == 15 || x == 5 || x == 10 || y == 5 || y == 10
                {
                    dark
                } else {
                    c
                }
            });
        }
        1 | 2 => {
            for x in 0..16 {
                img.set(x, 0, dark);
                img.set(x, 1, tone([120, 86, 50], 1.0));
                img.set(x, 2, dark);
            }
            let tool: [&str; 9] = if kind == 1 {
                // A saw and a hammer hanging on the side.
                [
                    "................",
                    "..mm.......hhh..",
                    "..mmm......hhh..",
                    "..mmmm......w...",
                    "..mmmm......w...",
                    "...mmm......w...",
                    "....ww......w...",
                    "....ww......w...",
                    "................",
                ]
            } else {
                [
                    "................",
                    "...mmmmmm.......",
                    "...m....m..ww...",
                    "...mmmmmm..ww...",
                    "...m....m..ww...",
                    "...mmmmmm..ww...",
                    ".........wwww...",
                    "................",
                    "................",
                ]
            };
            let rows: Vec<&str> = std::iter::repeat_n("", 4).chain(tool).collect();
            img.sprite(
                &rows,
                &[('m', metal), ('h', tone([90, 90, 94], 1.0)), ('w', dark)],
            );
        }
        _ => {}
    }
    img
}

fn furnace(kind: u8) -> Img {
    let mut img = smooth([118, 118, 118], 141, 0.06);
    match kind {
        0 | 1 => {
            img.bevel(1.18, 0.7);
            for y in 8..14 {
                for x in 4..12 {
                    let arch = y == 8 && (x == 4 || x == 11);
                    if arch {
                        continue;
                    }
                    let c = if kind == 1 {
                        let t = (y - 8) as f32 / 5.0;
                        let flicker = hash(142, x, y);
                        if flicker > 0.75 - t * 0.3 {
                            tone([255, 220, 90], 1.0)
                        } else {
                            tone(mix([255, 150, 40], [200, 60, 20], 1.0 - t), 1.0)
                        }
                    } else {
                        tone([32, 30, 30], 1.0 + 0.2 * hash(143, x, y))
                    };
                    img.set(x, y, c);
                }
            }
            for x in 3..13 {
                img.set(x, 14, tone([80, 80, 80], 1.0));
                img.set(x, 4, tone([86, 86, 86], 1.0));
            }
        }
        2 => img.bevel(1.12, 0.74),
        _ => {
            img.bevel(1.12, 0.74);
            for i in 3..13 {
                img.set(i, 3, tone([96, 96, 96], 1.0));
                img.set(i, 12, tone([96, 96, 96], 1.0));
                img.set(3, i, tone([96, 96, 96], 1.0));
                img.set(12, i, tone([96, 96, 96], 1.0));
            }
        }
    }
    img
}

fn chest(kind: u8) -> Img {
    let wood = [164, 112, 50];
    let mut img = Img::from_fn(|x, y| {
        let grain = vnoise2(151, x as f32, y as f32, 2, 8);
        tone(wood, 0.9 + 0.18 * grain)
    });
    let rim = tone([86, 54, 22], 1.0);
    img.map(|x, y, c| {
        if x == 0 || y == 0 || x == 15 || y == 15 || (kind > 0 && y == 5) {
            rim
        } else {
            c
        }
    });
    if kind == 2 {
        img.sprite(
            &[
                "",
                "",
                "",
                "",
                "......kkkk......",
                "......kmmk......",
                "......kmmk......",
                "......kkkk......",
            ],
            &[
                ('k', tone([40, 36, 30], 1.0)),
                ('m', tone([214, 214, 210], 1.0)),
            ],
        );
    }
    img
}

fn glass() -> Img {
    let mut img = Img::clear();
    let frame = [214, 232, 236, 255];
    for i in 0..16 {
        img.set(i, 0, frame);
        img.set(i, 15, alpha(tone([214, 232, 236], 0.85), 255));
        img.set(0, i, frame);
        img.set(15, i, alpha(tone([214, 232, 236], 0.85), 255));
    }
    for (x, y) in [(3, 2), (4, 3), (5, 4), (3, 3), (11, 9), (12, 10), (11, 10)] {
        img.set(x, y, [236, 246, 250, 150]);
    }
    img
}

fn bricks() -> Img {
    Img::from_fn(|x, y| {
        let row = y / 4;
        let offset = if row % 2 == 0 { 0 } else { 4 };
        if y % 4 == 3 || (x + offset) % 8 == 7 {
            return tone([176, 166, 154], 0.9 + 0.2 * hash(161, x, y));
        }
        let brick = (x + offset) / 8 + row * 4;
        let f = 0.86 + 0.24 * hash(162, brick, 0);
        let edge = if y % 4 == 0 { 1.08 } else { 1.0 };
        tone([152, 72, 56], f * edge * (0.95 + 0.1 * hash(163, x, y)))
    })
}

fn stone_bricks() -> Img {
    Img::from_fn(|x, y| {
        let row = y / 8;
        let offset = if row == 0 { 0 } else { 8 };
        let lx = (x + offset) % 16;
        if y % 8 == 7 || lx == 15 {
            return tone([82, 82, 84], 1.0);
        }
        let lit = if y % 8 == 0 || lx == 0 { 1.12 } else { 1.0 };
        tone(
            [124, 123, 124],
            lit * (0.9 + 0.16 * vnoise(171, x as f32, y as f32, 8)),
        )
    })
}

fn wool(base: Rgb, seed: u32) -> Img {
    Img::from_fn(|x, y| {
        let tau = std::f32::consts::TAU;
        let wobble = (tau * y as f32 * 2.0 / 16.0).sin() * 2.0;
        let curl = ((tau * x as f32 * 3.0 / 16.0 + wobble).sin() * 0.5 + 0.5) * 0.5
            + vnoise(seed, x as f32, y as f32, 8) * 0.5;
        tone(base, 0.84 + 0.24 * curl + 0.04 * (hash(seed, x, y) - 0.5))
    })
}

fn glowstone() -> Img {
    let cells = Cells::new(181, 7);
    let colors = [
        [252, 222, 128],
        [240, 186, 80],
        [214, 152, 64],
        [255, 236, 170],
    ];
    Img::from_fn(|x, y| {
        let (i, d1, d2) = cells.at(x, y);
        if d2 - d1 < 0.9 {
            return tone([142, 96, 44], 1.0);
        }
        let c = colors[(hash(182, i as i32, 0) * 4.0) as usize % 4];
        tone(c, if d1 < 1.2 { 1.12 } else { 1.0 })
    })
}

fn sapling(kind: u8) -> Img {
    let mut img = Img::clear();
    let (dark, light) = match kind {
        0 => ([46, 112, 34], [92, 162, 56]),
        1 => ([96, 140, 60], [150, 190, 92]),
        _ => ([30, 72, 44], [58, 110, 66]),
    };
    let rows: &[&str] = if kind == 2 {
        &[
            "",
            "",
            "",
            "........",
            ".......g........",
            "......gGg.......",
            ".....gGGGg......",
            "......gGg.......",
            ".....gGGGg......",
            "....gGGGGGg.....",
            "......gbg.......",
            ".......b........",
            ".......b........",
            "......bb........",
            ".......b........",
            ".......b........",
        ]
    } else {
        &[
            "",
            "",
            "",
            ".....gg.........",
            "....gGGg..g.....",
            "...gGGGGggGg....",
            "....gGGGGGGg....",
            "...ggGGgGGg.....",
            ".....gGGGg......",
            "......gbg.......",
            ".......b........",
            "......bb........",
            ".......b........",
            ".......b........",
            "......bb........",
            ".......b........",
        ]
    };
    img.sprite(
        rows,
        &[
            ('g', tone(dark, 1.0)),
            ('G', tone(light, 1.0)),
            ('b', tone([104, 76, 40], 1.0)),
        ],
    );
    img
}

fn tall_grass() -> Img {
    let mut img = Img::clear();
    img.sprite(
        &[
            "",
            "",
            "",
            "",
            ".........g......",
            "....g....g......",
            "....g...gG...g..",
            ".g..gg..g.G..g..",
            ".g...g.gG..g.gg.",
            "..g..gGg...gGg..",
            "..gg.Gg.g..Gg...",
            "...gGg..gGgG....",
            "....gG...gG.....",
            "....Gg..gGg.....",
            ".....gGgGg......",
            "......gGG.......",
        ],
        &[
            ('g', tone([70, 132, 44], 1.0)),
            ('G', tone([112, 176, 70], 1.0)),
        ],
    );
    img
}

fn flower(petal: Rgb, centre: Rgb) -> Img {
    let mut img = Img::clear();
    img.sprite(
        &[
            "",
            "",
            "",
            "",
            "......pPp.......",
            ".....pPcPp......",
            ".....PcccP......",
            ".....pPcPp......",
            "......pPp.......",
            ".......s........",
            ".......s.ll.....",
            "..ll...sll......",
            "...lll.s........",
            ".....lss........",
            ".......s........",
            ".......s........",
        ],
        &[
            ('p', tone(petal, 0.8)),
            ('P', tone(petal, 1.0)),
            ('c', tone(centre, 1.0)),
            ('s', tone([60, 120, 40], 1.0)),
            ('l', tone([84, 150, 56], 1.0)),
        ],
    );
    img
}

fn torch() -> Img {
    let mut img = Img::clear();
    img.sprite(
        &[
            "",
            "",
            "",
            "",
            "",
            ".......yy.......",
            "......yWWy......",
            "......oyyo......",
            ".......sS.......",
            ".......sS.......",
            ".......sS.......",
            ".......sS.......",
            ".......sS.......",
            ".......sS.......",
            ".......sS.......",
            ".......sS.......",
        ],
        &[
            ('y', [255, 216, 96, 255]),
            ('W', [255, 250, 210, 255]),
            ('o', [236, 132, 40, 255]),
            ('s', [124, 92, 52, 255]),
            ('S', [96, 70, 40, 255]),
        ],
    );
    img
}

fn cactus(kind: u8) -> Img {
    let base = [86, 138, 50];
    Img::from_fn(|x, y| match kind {
        1 => {
            if x == 0 || x == 15 {
                return [0, 0, 0, 0];
            }
            let stripe = matches!(x, 3 | 7 | 11);
            if stripe && y % 4 == 1 {
                return tone([214, 226, 176], 1.0);
            }
            tone(
                base,
                if stripe { 0.72 } else { 1.0 } * (0.92 + 0.12 * hash(191, x, y)),
            )
        }
        _ => {
            let d = (x as f32 - 7.5).abs().max((y as f32 - 7.5).abs());
            if d > 6.6 {
                return [0, 0, 0, 0];
            }
            let f = if kind == 0 { 1.1 } else { 0.8 };
            tone(base, f * if (d as i32) % 3 == 0 { 0.82 } else { 1.0 })
        }
    })
}

fn sugar_cane() -> Img {
    let mut img = Img::clear();
    for (x0, phase) in [(3, 0), (8, 2), (12, 1)] {
        for y in 0..16 {
            let node = (y + phase) % 5 == 0;
            let c = if node {
                [176, 210, 120]
            } else {
                [112, 168, 72]
            };
            img.set(x0, y, tone(c, 1.0));
            img.set(x0 + 1, y, tone(c, 0.82));
        }
        img.set(x0 + 2, (phase * 3 + 4) % 16, tone([96, 156, 60], 1.0));
        img.set(x0 - 1, (phase * 5 + 9) % 16, tone([96, 156, 60], 1.0));
    }
    img
}

fn farmland(wet: bool) -> Img {
    let base = if wet { [78, 52, 34] } else { [118, 82, 54] };
    Img::from_fn(|x, y| {
        let f = if y % 4 == 0 {
            0.74
        } else {
            0.92 + 0.16 * hash(201, x, y)
        };
        tone(base, f)
    })
}

fn wheat(stage: u8) -> Img {
    let mut img = Img::clear();
    let height = [4, 7, 11, 14][usize::from(stage)];
    let (stem, head) = if stage == 3 {
        ([188, 164, 72], [220, 190, 84])
    } else {
        ([88, 148, 50], [118, 178, 64])
    };
    for (i, x) in [1, 4, 7, 10, 13].into_iter().enumerate() {
        let h = height - (i as i32 % 2);
        for y in 16 - h..16 {
            let sway = if y < 16 - h / 2 { i as i32 % 2 } else { 0 };
            img.set(x + sway, y, tone(stem, 0.9 + 0.2 * hash(211, x, y)));
        }
        if stage >= 2 {
            for y in 16 - h..16 - h + 3 {
                img.set(x + 1, y, tone(head, 1.0));
            }
        }
    }
    img
}

fn ladder() -> Img {
    let mut img = Img::clear();
    let rail = [126, 92, 52];
    for y in 0..16 {
        for x in [1, 2, 13, 14] {
            img.set(x, y, tone(rail, if x == 2 || x == 14 { 0.8 } else { 1.0 }));
        }
        if y % 4 == 2 {
            for x in 3..13 {
                img.set(x, y, tone(rail, 1.05));
            }
            for x in 3..13 {
                img.set(x, y + 1, tone(rail, 0.7));
            }
        }
    }
    img
}

fn door(upper: bool) -> Img {
    let mut img = planks(OAK, 221);
    let frame = tone([108, 78, 42], 1.0);
    img.map(|x, y, c| {
        if x == 0 || x == 15 || (upper && y == 0) || (!upper && y == 15) {
            frame
        } else {
            c
        }
    });
    if upper {
        for y in 3..9 {
            for x in 3..13 {
                if x == 7 || x == 8 {
                    img.set(x, y, frame);
                } else {
                    img.set(x, y, [0, 0, 0, 0]);
                }
            }
        }
    } else {
        img.set(12, 1, tone([60, 60, 60], 1.0));
        img.set(12, 2, tone([60, 60, 60], 1.0));
    }
    img
}

fn bed(kind: u8) -> Img {
    let red = [168, 38, 38];
    Img::from_fn(|x, y| match kind {
        0 => {
            if y < 6 && (1..15).contains(&x) && y > 0 {
                tone([236, 236, 230], 0.95 + 0.1 * hash(231, x, y))
            } else {
                tone(red, 0.9 + 0.14 * hash(232, x, y))
            }
        }
        1 => tone(
            red,
            if y == 12 {
                0.8
            } else {
                0.92 + 0.14 * hash(233, x, y)
            },
        ),
        _ => {
            if y < 7 {
                tone(red, 0.9 + 0.14 * hash(234, x, y))
            } else if y < 9 {
                tone([226, 226, 220], 1.0)
            } else if y >= 13 && !(3..=12).contains(&x) {
                tone([110, 80, 44], 1.0)
            } else if (9..13).contains(&y) {
                tone([140, 104, 60], 1.0)
            } else {
                [0, 0, 0, 0]
            }
        }
    })
}

fn metal(base: Rgb, seed: u32) -> Img {
    let mut img = smooth(base, seed, 0.04);
    img.bevel(1.2, 0.72);
    for i in 2..14 {
        let c = tone(base, 0.9);
        img.set(i, 2, c);
        img.set(2, i, c);
    }
    img
}

fn bookshelf() -> Img {
    let mut img = planks(OAK, 241);
    let palette = [
        [150, 40, 40],
        [50, 80, 150],
        [60, 120, 60],
        [170, 140, 60],
        [110, 60, 120],
        [200, 190, 170],
    ];
    for shelf in [2, 9] {
        let mut x = 1;
        let mut k = 0;
        while x < 15 {
            let w = 1 + (hash(242, k, shelf) * 2.0) as i32;
            let top = shelf + (hash(243, k, shelf) * 2.0) as i32;
            let c = palette[(hash(244, k, shelf) * 6.0) as usize % 6];
            for dx in 0..w.min(15 - x) {
                for y in top..shelf + 5 {
                    img.set(x + dx, y, tone(c, if dx == 0 { 1.0 } else { 0.82 }));
                }
            }
            x += w + i32::from(hash(245, k, shelf) > 0.7);
            k += 1;
        }
    }
    img
}

pub fn tile(t: Tex) -> Img {
    match t {
        Tex::GrassTop => grass_top(),
        Tex::GrassSide => fringe(GRASS, 14),
        Tex::SnowyGrassSide => fringe(SNOW, 15),
        Tex::Dirt => dirt(21, DIRT),
        Tex::Stone => stone(),
        Tex::Cobblestone => cobble(201),
        Tex::MossyCobblestone => {
            let mut img = cobble(201);
            img.map(|x, y, c| {
                if vnoise(205, x as f32, y as f32, 4) > 0.58 {
                    tone([86, 122, 58], 0.85 + 0.3 * hash(206, x, y))
                } else {
                    c
                }
            });
            img
        }
        Tex::Bedrock => bedrock(),
        Tex::Sand => sand(41, SAND),
        Tex::SandstoneTop => sandstone(0),
        Tex::SandstoneSide => sandstone(1),
        Tex::SandstoneBottom => sandstone(2),
        Tex::Gravel => gravel(),
        Tex::Clay => smooth([160, 165, 178], 57, 0.05),
        Tex::Snow => speckle(SNOW, 58, 0.03),
        Tex::Ice => ice(),
        Tex::Water => water(),
        Tex::Lava => lava(),
        Tex::Obsidian => obsidian(),
        Tex::CoalOre => ore(301, [36, 36, 36]),
        Tex::IronOre => ore(302, [214, 172, 146]),
        Tex::GoldOre => ore(303, [250, 234, 76]),
        Tex::DiamondOre => ore(304, [96, 232, 240]),
        Tex::OakLog => log_side([104, 82, 50], 311),
        Tex::OakLogTop => log_top([96, 76, 46], [178, 144, 90], 312),
        Tex::BirchLog => birch_side(),
        Tex::BirchLogTop => log_top([214, 211, 202], [204, 184, 132], 313),
        Tex::SpruceLog => log_side([60, 40, 22], 314),
        Tex::SpruceLogTop => log_top([56, 38, 20], [118, 88, 52], 315),
        Tex::OakLeaves => leaves([58, 136, 38], 321),
        Tex::BirchLeaves => leaves([118, 160, 78], 322),
        Tex::SpruceLeaves => leaves([40, 86, 54], 323),
        Tex::OakPlanks => planks(OAK, 331),
        Tex::BirchPlanks => planks([198, 180, 124], 332),
        Tex::SprucePlanks => planks([112, 82, 48], 333),
        Tex::CraftingTop => crafting(0),
        Tex::CraftingSide => crafting(1),
        Tex::CraftingFront => crafting(2),
        Tex::FurnaceFront => furnace(0),
        Tex::FurnaceFrontLit => furnace(1),
        Tex::FurnaceSide => furnace(2),
        Tex::FurnaceTop => furnace(3),
        Tex::ChestTop => chest(0),
        Tex::ChestSide => chest(1),
        Tex::ChestFront => chest(2),
        Tex::Glass => glass(),
        Tex::Bricks => bricks(),
        Tex::StoneBricks => stone_bricks(),
        Tex::WoolWhite => wool([234, 234, 230], 341),
        Tex::WoolRed => wool([176, 44, 40], 342),
        Tex::WoolYellow => wool([236, 196, 52], 343),
        Tex::WoolBlue => wool([52, 72, 168], 344),
        Tex::WoolGreen => wool([86, 136, 40], 345),
        Tex::WoolBlack => wool([50, 47, 56], 346),
        Tex::Glowstone => glowstone(),
        Tex::Torch => torch(),
        Tex::OakSapling => sapling(0),
        Tex::BirchSapling => sapling(1),
        Tex::SpruceSapling => sapling(2),
        Tex::TallGrass => tall_grass(),
        Tex::Dandelion => flower([246, 214, 40], [214, 150, 20]),
        Tex::Poppy => flower([206, 36, 30], [40, 30, 20]),
        Tex::CactusTop => cactus(0),
        Tex::CactusSide => cactus(1),
        Tex::CactusBottom => cactus(2),
        Tex::SugarCane => sugar_cane(),
        Tex::Farmland => farmland(false),
        Tex::FarmlandWet => farmland(true),
        Tex::Wheat0 => wheat(0),
        Tex::Wheat1 => wheat(1),
        Tex::Wheat2 => wheat(2),
        Tex::Wheat3 => wheat(3),
        Tex::Ladder => ladder(),
        Tex::DoorLower => door(false),
        Tex::DoorUpper => door(true),
        Tex::BedHead => bed(0),
        Tex::BedFoot => bed(1),
        Tex::BedSide => bed(2),
        Tex::IronBlock => metal([216, 216, 216], 351),
        Tex::GoldBlock => metal([246, 208, 62], 352),
        Tex::DiamondBlock => metal([104, 226, 220], 353),
        Tex::Bookshelf => bookshelf(),
    }
}

/// Every tile in layer order.
pub fn all() -> Vec<Img> {
    Tex::all().map(tile).collect()
}

/// RGBA bytes of all layers followed by successively halved mip levels:
/// returns one byte vector per level (16, 8, 4, 2, 1 pixels).
pub fn mip_chain(tiles: &[Img]) -> Vec<Vec<u8>> {
    let mut levels = vec![
        tiles
            .iter()
            .flat_map(|t| t.0.iter().flatten().copied())
            .collect::<Vec<u8>>(),
    ];
    let mut size = TILE;
    while size > 1 {
        let half = size / 2;
        let prev = levels.last().unwrap();
        let mut next = Vec::with_capacity(tiles.len() * half * half * 4);
        for layer in 0..tiles.len() {
            let base = layer * size * size * 4;
            for y in 0..half {
                for x in 0..half {
                    for c in 0..4 {
                        let at = |dx: usize, dy: usize| {
                            u32::from(prev[base + ((y * 2 + dy) * size + x * 2 + dx) * 4 + c])
                        };
                        next.push(((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8);
                    }
                }
            }
        }
        levels.push(next);
        size = half;
    }
    levels
}

/// Writes a preview sheet of all tiles (scaled ×4, 12 per row) as PNG.
pub fn write_sheet(path: &std::path::Path, tiles: &[Img]) -> anyhow::Result<()> {
    const SCALE: usize = 4;
    const COLUMNS: usize = 12;
    let cell = TILE * SCALE + 4;
    let rows = tiles.len().div_ceil(COLUMNS);
    let (w, h) = (COLUMNS * cell, rows * cell);
    let mut rgba = vec![0u8; w * h * 4];
    for (i, tile) in tiles.iter().enumerate() {
        let (ox, oy) = ((i % COLUMNS) * cell + 2, (i / COLUMNS) * cell + 2);
        for y in 0..TILE * SCALE {
            for x in 0..TILE * SCALE {
                // Checkerboard behind transparent pixels.
                let checker = if ((x / 8) + (y / 8)) % 2 == 0 {
                    70
                } else {
                    100
                };
                let p = tile.get((x / SCALE) as i32, (y / SCALE) as i32);
                let a = u32::from(p[3]);
                let o = ((oy + y) * w + ox + x) * 4;
                for c in 0..3 {
                    rgba[o + c] = ((u32::from(p[c]) * a + checker * (255 - a)) / 255) as u8;
                }
                rgba[o + 3] = 255;
            }
        }
    }
    write_png(path, w as u32, h as u32, &rgba)
}

pub fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut encoder = png::Encoder::new(file, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(rgba)?;
    Ok(())
}

const _: () = assert!(TEX_COUNT <= 256);

#[cfg(test)]
mod tests {
    use super::*;

    fn variance(img: &Img) -> f32 {
        let lum: Vec<f32> = img
            .0
            .iter()
            .filter(|p| p[3] > 0)
            .map(|p| 0.3 * f32::from(p[0]) + 0.59 * f32::from(p[1]) + 0.11 * f32::from(p[2]))
            .collect();
        let mean = lum.iter().sum::<f32>() / lum.len().max(1) as f32;
        lum.iter().map(|l| (l - mean).powi(2)).sum::<f32>() / lum.len().max(1) as f32
    }

    #[test]
    fn tiles_are_deterministic_textured_and_have_expected_alpha() {
        let a = all();
        let b = all();
        assert_eq!(a.len(), TEX_COUNT);
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            assert_eq!(x.0, y.0, "tile {i} not deterministic");
            assert!(variance(x) > 4.0, "tile {:?} is flat", Tex::all().nth(i));
        }
        let opaque = |t: Tex| tile(t).0.iter().all(|p| p[3] == 255);
        let holes = |t: Tex| tile(t).0.iter().filter(|p| p[3] == 0).count();
        for t in [
            Tex::Stone,
            Tex::GrassSide,
            Tex::OakPlanks,
            Tex::FurnaceFront,
            Tex::Bricks,
        ] {
            assert!(opaque(t), "{t:?}");
        }
        assert!(holes(Tex::OakLeaves) > 20 && holes(Tex::OakLeaves) < 100);
        assert!(holes(Tex::Glass) > 150);
        assert!(holes(Tex::Torch) > 200 && holes(Tex::Torch) < 240);
        assert!(tile(Tex::Water).0.iter().all(|p| p[3] > 100 && p[3] < 255));
        // Distinct tiles look distinct.
        assert_ne!(tile(Tex::CoalOre).0, tile(Tex::IronOre).0);
        assert_ne!(tile(Tex::OakPlanks).0, tile(Tex::BirchPlanks).0);
    }

    #[test]
    fn noise_wraps_so_tiles_repeat_seamlessly() {
        for (cx, cy) in [(4, 4), (8, 2), (2, 8)] {
            for i in 0..16 {
                let t = i as f32;
                assert!((vnoise2(5, 0.0, t, cx, cy) - vnoise2(5, 16.0, t, cx, cy)).abs() < 1e-5);
                assert!((vnoise2(5, t, 0.0, cx, cy) - vnoise2(5, t, 16.0, cx, cy)).abs() < 1e-5);
            }
        }
        // Opposite edges of natural tiles differ about as much as
        // neighbouring rows inside the tile, i.e. there is no visible seam.
        for t in [
            Tex::OakLog,
            Tex::SpruceLog,
            Tex::Stone,
            Tex::OakPlanks,
            Tex::Water,
        ] {
            let img = tile(t);
            let lum = |x: i32, y: i32| {
                let p = img.get(x, y);
                i32::from(p[0]) + i32::from(p[1]) + i32::from(p[2])
            };
            let seam: i32 = (0..16).map(|x| (lum(x, 15) - lum(x, 0)).abs()).sum();
            let inner: i32 = (1..15)
                .map(|y| {
                    (0..16)
                        .map(|x| (lum(x, y) - lum(x, y - 1)).abs())
                        .sum::<i32>()
                })
                .sum::<i32>()
                / 14;
            assert!(
                seam < inner * 3 + 120,
                "{t:?}: seam {seam} vs inner {inner}"
            );
        }
    }

    #[test]
    fn mip_chain_halves_down_to_one_pixel() {
        let tiles = vec![tile(Tex::Stone), tile(Tex::Snow)];
        let levels = mip_chain(&tiles);
        assert_eq!(levels.len(), 5);
        for (i, level) in levels.iter().enumerate() {
            let size = 16 >> i;
            assert_eq!(level.len(), 2 * size * size * 4);
        }
        let snow = &levels[4][4..8];
        assert!(snow[0] > 230 && snow[3] == 255, "{snow:?}");
    }
}
