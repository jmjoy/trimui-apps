//! Inventory icons rendered on the CPU from the block tiles.
//!
//! Cubes become isometric icons (top lit, left and right faces shaded);
//! plants, torches, ladders and doors use their flat sprite.
use super::textures::{Img, Rgba};
use crate::world::block::{self, Face, Render};

pub const ICON: usize = 48;
pub const COLUMNS: usize = 16;

pub struct Atlas {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}
impl Atlas {
    /// Normalised UV rectangle of a block's icon: (min x, min y, max x, max y).
    pub fn uv(&self, id: u8) -> [f32; 4] {
        let i = usize::from(id);
        let (x, y) = ((i % COLUMNS * ICON) as f32, (i / COLUMNS * ICON) as f32);
        [
            x / self.width as f32,
            y / self.height as f32,
            (x + ICON as f32) / self.width as f32,
            (y + ICON as f32) / self.height as f32,
        ]
    }
    #[cfg(test)]
    pub fn pixel(&self, id: u8, x: usize, y: usize) -> Rgba {
        let i = usize::from(id);
        let (ox, oy) = (i % COLUMNS * ICON, i / COLUMNS * ICON);
        let o = ((oy + y) * self.width + ox + x) * 4;
        [
            self.rgba[o],
            self.rgba[o + 1],
            self.rgba[o + 2],
            self.rgba[o + 3],
        ]
    }
}

type Icon = [Rgba; ICON * ICON];

fn shade(c: Rgba, f: f32) -> Rgba {
    let s = |v: u8| (f32::from(v) * f).min(255.0) as u8;
    [s(c[0]), s(c[1]), s(c[2]), c[3]]
}

/// Draws a parallelogram face: `origin` plus `u`, `v` edge vectors, sampling
/// `tile` (texture x along u, y along v); `rows` limits the texture rows.
fn face(
    icon: &mut Icon,
    tile: &Img,
    origin: [f32; 2],
    u: [f32; 2],
    v: [f32; 2],
    light: f32,
    rows: f32,
) {
    let det = u[0] * v[1] - u[1] * v[0];
    for py in 0..ICON {
        for px in 0..ICON {
            let (dx, dy) = (px as f32 + 0.5 - origin[0], py as f32 + 0.5 - origin[1]);
            let a = (dx * v[1] - dy * v[0]) / det;
            let b = (u[0] * dy - u[1] * dx) / det;
            if !(0.0..1.0).contains(&a) || !(0.0..1.0).contains(&b) {
                continue;
            }
            let texel = tile.get((a * 16.0) as i32, (16.0 - rows + b * rows) as i32);
            if texel[3] > 0 {
                icon[py * ICON + px] = shade(texel, light);
            }
        }
    }
}

/// Isometric box `height` blocks tall (1.0 = full cube).
fn cube(tiles: &[Img], id: u8, height: f32) -> Icon {
    let mut icon = [[0; 4]; ICON * ICON];
    let tex = |f: Face| &tiles[block::face_texture(id, 2, f) as usize];
    let (w, top_h, side_h) = (21.0, 11.0, 22.0 * height);
    let y0 = 2.0 + 22.0 * (1.0 - height);
    let centre = 24.0;
    // Top: from the back corner towards the right and left corners.
    face(
        &mut icon,
        tex(Face::Up),
        [centre, y0],
        [w, top_h],
        [-w, top_h],
        1.0,
        16.0,
    );
    // Left (south, the front of oriented blocks) and right (east) faces.
    let left = [centre - w, y0 + top_h];
    face(
        &mut icon,
        tex(Face::South),
        left,
        [w, top_h],
        [0.0, side_h],
        0.8,
        16.0 * height,
    );
    let right = [centre, y0 + 2.0 * top_h];
    face(
        &mut icon,
        tex(Face::East),
        right,
        [w, -top_h],
        [0.0, side_h],
        0.62,
        16.0 * height,
    );
    icon
}

/// The flat sprite scaled ×3 (doors stack both halves).
fn sprite(tiles: &[Img], id: u8) -> Icon {
    let mut icon = [[0; 4]; ICON * ICON];
    let mut blit = |tile: &Img, x0: usize, y0: usize, scale: f32| {
        let size = (16.0 * scale) as usize;
        for y in 0..size {
            for x in 0..size {
                let p = tile.get((x as f32 / scale) as i32, (y as f32 / scale) as i32);
                if p[3] > 0 && x0 + x < ICON && y0 + y < ICON {
                    icon[(y0 + y) * ICON + x0 + x] = p;
                }
            }
        }
    };
    if id == block::DOOR {
        blit(&tiles[block::Tex::DoorUpper as usize], 12, 0, 1.5);
        blit(&tiles[block::Tex::DoorLower as usize], 12, 24, 1.5);
    } else {
        blit(
            &tiles[block::face_texture(id, 0, Face::North) as usize],
            0,
            0,
            3.0,
        );
    }
    icon
}

pub fn build(tiles: &[Img]) -> Atlas {
    let rows = block::COUNT.div_ceil(COLUMNS);
    let (width, height) = (COLUMNS * ICON, rows * ICON);
    let mut rgba = vec![0u8; width * height * 4];
    for id in 1..block::COUNT as u8 {
        let icon = match block::def(id).render {
            Render::Cross | Render::Torch | Render::Ladder | Render::Door | Render::Crop => {
                sprite(tiles, id)
            }
            Render::Bed => cube(tiles, id, 9.0 / 16.0),
            Render::Farmland => cube(tiles, id, 15.0 / 16.0),
            Render::Layer => cube(tiles, id, 2.0 / 16.0),
            _ => cube(tiles, id, 1.0),
        };
        let (ox, oy) = (
            usize::from(id) % COLUMNS * ICON,
            usize::from(id) / COLUMNS * ICON,
        );
        for y in 0..ICON {
            let o = ((oy + y) * width + ox) * 4;
            rgba[o..o + ICON * 4].copy_from_slice(icon[y * ICON..(y + 1) * ICON].as_flattened());
        }
    }
    Atlas {
        width,
        height,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::textures;

    #[test]
    fn cube_icons_have_shape_and_shading() {
        let atlas = build(&textures::all());
        assert_eq!(atlas.width % ICON, 0);
        let stone = block::STONE;
        let a = |x, y| atlas.pixel(stone, x, y)[3];
        assert_eq!(a(0, 0), 0, "corners are transparent");
        assert_eq!(a(47, 47), 0);
        assert_eq!(a(24, 1), 0, "padding above the top corner");
        assert_eq!(a(24, 12), 255, "top face");
        assert_eq!(a(10, 28), 255, "left face");
        assert_eq!(a(38, 28), 255, "right face");
        let lum = |p: Rgba| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
        let avg = |x0: usize, x1: usize, y0: usize, y1: usize| {
            let mut sum = 0;
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += lum(atlas.pixel(stone, x, y));
                }
            }
            sum / ((x1 - x0) * (y1 - y0)) as u32
        };
        let (top, left, right) = (avg(20, 28, 9, 15), avg(6, 14, 24, 32), avg(34, 42, 24, 32));
        assert!(top > left && left > right, "{top} {left} {right}");
        // Sprites keep their transparency; air has no icon.
        let flower = (0..ICON * ICON)
            .filter(|i| atlas.pixel(block::DANDELION, i % ICON, i / ICON)[3] == 0)
            .count();
        assert!(flower > ICON * ICON / 2);
        assert!((0..ICON).all(|i| atlas.pixel(block::AIR, i, i)[3] == 0));
        let uv = atlas.uv(17);
        assert!(uv[0] > 0.0 && uv[2] <= 1.0 && uv[1] > 0.0);
    }
}
