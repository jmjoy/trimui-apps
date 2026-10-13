//! Stateless per-column terrain generation.
//!
//! Each column is computed only from the seed and its position. Trees whose
//! canopy crosses a column border are evaluated from every column they touch,
//! so neighbouring columns agree no matter which is generated first.
use super::{
    BEDROCK,
    block::{DIRT, GRASS, OAK_LEAVES, OAK_LOG, SAND, STONE},
    chunk::{Column, HEIGHT},
    noise,
};
use glam::{IVec2, Vec3};

/// Surface height at a world column.
pub fn height(seed: u64, x: i32, z: i32) -> i32 {
    let h = 52.0
        + 26.0 * noise::value(seed, x as f32 / 64.0, z as f32 / 64.0)
        + 6.0 * noise::value(seed ^ 99, x as f32 / 17.0, z as f32 / 17.0);
    (h as i32).clamp(1, HEIGHT - 12)
}

fn sandy(seed: u64, x: i32, z: i32) -> bool {
    noise::value(seed ^ 777, x as f32 / 43.0, z as f32 / 43.0) > 0.68
}

/// Trunk height of a tree rooted at (x, z), if any.
fn tree(seed: u64, x: i32, z: i32) -> Option<i32> {
    // A clear patch keeps the initial spawn free of trunks and leaves.
    if x.abs() < 5 && z.abs() < 5 {
        return None;
    }
    if !noise::hash(seed ^ 12, x, z).is_multiple_of(103) || sandy(seed, x, z) {
        return None;
    }
    Some(4 + (noise::hash(seed, x, z) % 2) as i32)
}

pub fn generate(seed: u64, pos: IVec2) -> Column {
    let mut c = Column::empty(pos);
    let (x0, z0) = (pos.x * 16, pos.y * 16);
    for lz in 0..16 {
        for lx in 0..16 {
            let (x, z) = (x0 + lx, z0 + lz);
            let h = height(seed, x, z);
            let sand = sandy(seed, x, z);
            for y in 0..=h {
                let block = if y == 0 {
                    BEDROCK
                } else if y < h - 3 {
                    STONE
                } else if sand {
                    SAND
                } else if y == h {
                    GRASS
                } else {
                    DIRT
                };
                c.put(lx, y, lz, block);
            }
        }
    }
    let local = |x: i32, z: i32| {
        let (lx, lz) = (x - x0, z - z0);
        ((0..16).contains(&lx) && (0..16).contains(&lz)).then_some((lx, lz))
    };
    for z in z0 - 2..z0 + 18 {
        for x in x0 - 2..x0 + 18 {
            let Some(trunk) = tree(seed, x, z) else {
                continue;
            };
            let h = height(seed, x, z);
            // Leaves only fill air and trunks always win, which makes the
            // result independent of the order trees are visited.
            for dy in trunk - 2..=trunk + 1 {
                for dz in -2i32..=2 {
                    for dx in -2i32..=2 {
                        let d = dx.abs() + dz.abs();
                        if d > 3 || (dy == trunk + 1 && d > 1) {
                            continue;
                        }
                        if let Some((lx, lz)) = local(x + dx, z + dz)
                            && c.get(lx, h + dy, lz) == 0
                        {
                            c.put(lx, h + dy, lz, OAK_LEAVES);
                        }
                    }
                }
            }
            if let Some((lx, lz)) = local(x, z) {
                for dy in 1..=trunk {
                    c.put(lx, h + dy, lz, OAK_LOG);
                }
            }
        }
    }
    c
}

/// World spawn: the surface at the origin, inside the tree-free patch.
pub fn spawn(seed: u64) -> Vec3 {
    Vec3::new(0.5, height(seed, 0, 0) as f32 + 1.001, 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_trees_continue_across_borders() {
        let seed = 42;
        let a = generate(seed, IVec2::new(3, -2));
        assert_eq!(a.encode(), generate(seed, IVec2::new(3, -2)).encode());
        assert_ne!(a.encode(), generate(seed + 1, IVec2::new(3, -2)).encode());
        // Find a tree rooted within two blocks of a column border and check
        // that its canopy appears in the neighbouring column as well.
        let mut checked = 0;
        'search: for z in -400..400 {
            for x in -400..400 {
                let Some(trunk) = tree(seed, x, z) else {
                    continue;
                };
                if x.rem_euclid(16) != 15 {
                    continue;
                }
                let h = height(seed, x, z);
                let right = generate(seed, IVec2::new((x + 1).div_euclid(16), z.div_euclid(16)));
                let lz = z.rem_euclid(16);
                assert_eq!(right.get(0, h + trunk, lz), OAK_LEAVES, "canopy at {x},{z}");
                let own = generate(seed, IVec2::new(x.div_euclid(16), z.div_euclid(16)));
                assert_eq!(own.get(15, h + 1, lz), OAK_LOG);
                checked += 1;
                if checked >= 3 {
                    break 'search;
                }
            }
        }
        assert!(checked >= 3, "no border trees found");
    }

    #[test]
    fn spawn_is_on_solid_ground_with_headroom() {
        for seed in [1, 42, 9_999] {
            let s = spawn(seed);
            let c = generate(seed, IVec2::ZERO);
            let y = s.y.floor() as i32;
            assert_ne!(c.get(0, y - 1, 0), 0);
            assert_eq!(c.get(0, y, 0), 0);
            assert_eq!(c.get(0, y + 1, 0), 0);
            assert_eq!(c.get(0, 0, 0), BEDROCK);
        }
    }
}
