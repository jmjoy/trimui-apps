//! Seeded hashing, value noise and Perlin gradient noise.

/// Stateless 64-bit hash of a seed and two integer coordinates.
pub fn hash(seed: u64, x: i32, z: i32) -> u64 {
    let mut v = seed
        ^ (x as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
        ^ (z as u64).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    v = (v ^ (v >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    v = (v ^ (v >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    v ^ (v >> 31)
}
/// Hash of three coordinates.
pub fn hash3(seed: u64, x: i32, y: i32, z: i32) -> u64 {
    hash(hash(seed, x, y), z, 0x5bd1)
}
/// Uniform value in [0, 1) from a hash.
pub fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / 16_777_216.0
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

#[inline]
fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
#[inline]
fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}
#[inline]
fn grad2(h: u8, x: f32, y: f32) -> f32 {
    match h & 7 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x,
        5 => -x,
        6 => y,
        _ => -y,
    }
}
#[inline]
fn grad3(h: u8, x: f32, y: f32, z: f32) -> f32 {
    let h = h & 15;
    let u = if h < 8 { x } else { y };
    let v = if h < 4 {
        y
    } else if h == 12 || h == 14 {
        x
    } else {
        z
    };
    (if h & 1 == 0 { u } else { -u }) + (if h & 2 == 0 { v } else { -v })
}

/// Seeded Perlin ("improved") gradient noise, roughly in [-1, 1].
#[derive(Clone)]
pub struct Perlin {
    perm: [u8; 512],
}

impl Perlin {
    pub fn new(seed: u64) -> Self {
        let mut p: [u8; 256] = std::array::from_fn(|i| i as u8);
        let mut s = seed;
        for i in (1..256).rev() {
            s = hash(s, i as i32, 0x7e57);
            p.swap(i, (s % (i as u64 + 1)) as usize);
        }
        Self {
            perm: std::array::from_fn(|i| p[i & 255]),
        }
    }
    pub fn noise2(&self, x: f32, y: f32) -> f32 {
        let (fx, fy) = (x.floor(), y.floor());
        let xi = (fx as i32 & 255) as usize;
        let yi = (fy as i32 & 255) as usize;
        let (x, y) = (x - fx, y - fy);
        let (u, v) = (fade(x), fade(y));
        let p = &self.perm;
        let a = p[xi] as usize + yi;
        let b = p[xi + 1] as usize + yi;
        lerp(
            v,
            lerp(u, grad2(p[a], x, y), grad2(p[b], x - 1.0, y)),
            lerp(
                u,
                grad2(p[a + 1], x, y - 1.0),
                grad2(p[b + 1], x - 1.0, y - 1.0),
            ),
        )
    }
    pub fn noise3(&self, x: f32, y: f32, z: f32) -> f32 {
        let (fx, fy, fz) = (x.floor(), y.floor(), z.floor());
        let xi = (fx as i32 & 255) as usize;
        let yi = (fy as i32 & 255) as usize;
        let zi = (fz as i32 & 255) as usize;
        let (x, y, z) = (x - fx, y - fy, z - fz);
        let (u, v, w) = (fade(x), fade(y), fade(z));
        let p = &self.perm;
        let a = p[xi] as usize + yi;
        let aa = p[a] as usize + zi;
        let ab = p[a + 1] as usize + zi;
        let b = p[xi + 1] as usize + yi;
        let ba = p[b] as usize + zi;
        let bb = p[b + 1] as usize + zi;
        lerp(
            w,
            lerp(
                v,
                lerp(u, grad3(p[aa], x, y, z), grad3(p[ba], x - 1.0, y, z)),
                lerp(
                    u,
                    grad3(p[ab], x, y - 1.0, z),
                    grad3(p[bb], x - 1.0, y - 1.0, z),
                ),
            ),
            lerp(
                v,
                lerp(
                    u,
                    grad3(p[aa + 1], x, y, z - 1.0),
                    grad3(p[ba + 1], x - 1.0, y, z - 1.0),
                ),
                lerp(
                    u,
                    grad3(p[ab + 1], x, y - 1.0, z - 1.0),
                    grad3(p[bb + 1], x - 1.0, y - 1.0, z - 1.0),
                ),
            ),
        )
    }
    /// Fractal sum of `octaves` layers, normalised to about [-1, 1].
    pub fn fbm2(&self, x: f32, y: f32, octaves: u32) -> f32 {
        let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
        for i in 0..octaves {
            let offset = i as f32 * 19.31;
            sum += amp * self.noise2(x * freq + offset, y * freq - offset);
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        sum / norm
    }
    pub fn fbm3(&self, x: f32, y: f32, z: f32, octaves: u32) -> f32 {
        let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
        for i in 0..octaves {
            let offset = i as f32 * 19.31;
            sum += amp * self.noise3(x * freq + offset, y * freq, z * freq - offset);
            norm += amp;
            amp *= 0.5;
            freq *= 2.0;
        }
        sum / norm
    }
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

    #[test]
    fn perlin_is_seeded_bounded_continuous_and_zero_on_lattice() {
        let a = Perlin::new(1);
        let b = Perlin::new(1);
        let c = Perlin::new(2);
        let mut same = 0;
        let mut previous = a.noise2(-5.0, 0.37);
        let mut spread = (f32::MAX, f32::MIN);
        for i in 0..4000 {
            let x = -5.0 + i as f32 * 0.0025;
            let n = a.noise2(x, 0.37);
            assert_eq!(n, b.noise2(x, 0.37));
            same += usize::from(n == c.noise2(x, 0.37));
            assert!((n - previous).abs() < 0.02, "jump at {x}");
            previous = n;
            let m = a.noise3(x, 1.7, -0.3);
            assert!(m.abs() <= 1.2 && n.abs() <= 1.2);
            spread = (spread.0.min(n), spread.1.max(n));
        }
        assert!(same < 100, "seeds differ");
        assert!(spread.0 < -0.3 && spread.1 > 0.3, "{spread:?}");
        assert_eq!(a.noise2(3.0, -7.0), 0.0);
        assert_eq!(a.noise3(3.0, 4.0, -7.0), 0.0);
        let f = a.fbm2(12.3, 4.5, 4);
        assert!(f.abs() <= 1.0);
    }
}
