//! A flat layer of blocky clouds drifting east.
//!
//! The pattern is fixed for every world; the mesh covers a window of cloud
//! cells around the camera and is rebuilt only when the camera crosses into
//! another cell.
use crate::world::noise;
use glam::IVec2;

/// Just below the top of the world, above every mountain.
pub const HEIGHT: f32 = 126.0;
/// Size of one cloud cell in blocks.
pub const CELL: i32 = 12;
/// Drift in blocks per world tick (0.6 blocks per second).
pub const SPEED: f64 = 0.03;
const SEED: u64 = 0x00c1_0d5e;

pub fn is_cloud(cell: IVec2) -> bool {
    let (x, z) = (cell.x as f32, cell.y as f32);
    // Three octaves on rotated, offset grids hide the lattice of value noise.
    let shape = noise::value(SEED, x * 0.13 + z * 0.05 + 3.7, z * 0.13 - x * 0.05 + 1.3) * 0.55
        + noise::value(
            SEED ^ 1,
            x * 0.29 - z * 0.11 + 7.1,
            z * 0.29 + x * 0.11 + 2.9,
        ) * 0.3
        + noise::value(SEED ^ 2, x * 0.71 + 5.3, z * 0.71 + 8.9) * 0.15;
    shape > 0.63
}

/// Cloud cell under a world position, given the drift so far.
pub fn cell_at(x: f64, z: f64, drift: f64) -> IVec2 {
    IVec2::new(
        ((x - drift) / f64::from(CELL)).floor() as i32,
        (z / f64::from(CELL)).floor() as i32,
    )
}

/// Triangles (positions relative to the corner of cell `anchor`) of every
/// cloud within `radius` cells; runs along x are merged into one quad.
pub fn build(anchor: IVec2, radius: i32) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    let c = CELL as f32;
    for dz in -radius..=radius {
        let mut dx = -radius;
        while dx <= radius {
            if !is_cloud(anchor + IVec2::new(dx, dz)) {
                dx += 1;
                continue;
            }
            let start = dx;
            while dx <= radius && is_cloud(anchor + IVec2::new(dx, dz)) {
                dx += 1;
            }
            let (x0, x1) = (start as f32 * c, dx as f32 * c);
            let (z0, z1) = (dz as f32 * c, (dz + 1) as f32 * c);
            out.extend_from_slice(&[
                [x0, 0.0, z0],
                [x0, 0.0, z1],
                [x1, 0.0, z1],
                [x0, 0.0, z0],
                [x1, 0.0, z1],
                [x1, 0.0, z0],
            ]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clouds_cover_part_of_the_sky_and_merge_runs() {
        let mut cloudy = 0;
        for z in -50..50 {
            for x in -50..50 {
                cloudy += usize::from(is_cloud(IVec2::new(x, z)));
            }
        }
        assert!((1500..6000).contains(&cloudy), "{cloudy} of 10000 cells");
        let tris = build(IVec2::ZERO, 10);
        assert_eq!(tris.len() % 6, 0);
        let quads = tris.len() / 6;
        let cells: usize = (-10..=10)
            .flat_map(|z| (-10..=10).map(move |x| IVec2::new(x, z)))
            .filter(|&c| is_cloud(c))
            .count();
        assert!(
            quads > 0 && quads < cells,
            "{quads} quads for {cells} cells"
        );
        assert!(
            tris.iter()
                .all(|p| p[1] == 0.0 && p[0].abs() <= 11.0 * 12.0)
        );
        // The anchor follows the drift.
        assert_eq!(cell_at(30.0, 5.0, 0.0), IVec2::new(2, 0));
        assert_eq!(cell_at(30.0, 5.0, 12.0), IVec2::new(1, 0));
        assert_eq!(cell_at(-1.0, -1.0, 0.0), IVec2::new(-1, -1));
    }
}
