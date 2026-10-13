//! Seeded hashing and value noise shared by terrain generation.

/// Stateless 64-bit hash of a seed and two integer coordinates.
pub fn hash(seed: u64, x: i32, z: i32) -> u64 {
    let mut v = seed
        ^ (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (z as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    v = (v ^ (v >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    v ^ (v >> 31)
}

/// Smooth 2D value noise in [0, 1]; continuous for negative coordinates too.
pub fn value(seed: u64, x: f32, z: f32) -> f32 {
    let fx = x.floor();
    let fz = z.floor();
    let (ix, iz) = (fx as i32, fz as i32);
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let tx = smooth(x - fx);
    let tz = smooth(z - fz);
    let h = |x, z| (hash(seed, x, z) >> 40) as f32 / 16_777_215.0;
    let a = h(ix, iz) * (1.0 - tx) + h(ix + 1, iz) * tx;
    let b = h(ix, iz + 1) * (1.0 - tx) + h(ix + 1, iz + 1) * tx;
    a * (1.0 - tz) + b * tz
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn value_noise_is_bounded_and_continuous_across_zero() {
        let mut previous = value(7, -3.0, 0.4);
        for i in 1..=6000 {
            let x = -3.0 + i as f32 * 0.001;
            let v = value(7, x, 0.4);
            assert!((0.0..=1.0).contains(&v));
            assert!((v - previous).abs() < 0.01, "jump at x={x}");
            previous = v;
        }
        assert_ne!(hash(1, -1, 0), hash(1, 1, 0));
    }
}
