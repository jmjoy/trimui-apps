//! Day–night cycle: sun position, sky colours and daylight factor.
//!
//! A day lasts 24 000 ticks (20 minutes at 20 ticks per second). Tick 0 is
//! sunrise, 6 000 noon, 12 000 sunset and 18 000 midnight.
use glam::Vec3;

pub const DAY: f64 = 24_000.0;
pub const TICKS_PER_SECOND: f64 = 20.0;

#[derive(Clone, Copy, Debug)]
pub struct Sky {
    pub zenith: Vec3,
    pub horizon: Vec3,
    /// Fog blends into this colour (the horizon seen from inside the world).
    pub fog: Vec3,
    pub sun: Vec3,
    /// Multiplier for sky light on blocks, 0.2 at night to 1.0 by day.
    pub daylight: f32,
    /// 0 by day, 1 on a clear night.
    pub stars: f32,
    /// Strength of the sunrise/sunset glow towards the sun, 0–1.
    pub dusk: f32,
    /// Fraction of the day in 0..1.
    pub phase: f32,
}

fn lerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    a + (b - a) * t.clamp(0.0, 1.0)
}
fn smooth(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn sky(time: f64) -> Sky {
    let phase = (time.rem_euclid(DAY) / DAY) as f32;
    let angle = phase * std::f32::consts::TAU;
    // Rises in the east (+X), sets in the west, slightly tilted south.
    let sun = Vec3::new(angle.cos(), angle.sin(), 0.18).normalize();
    let elevation = sun.y;
    let day = smooth(-0.18, 0.28, elevation);
    let daylight = 0.2 + 0.8 * day;
    let day_zenith = Vec3::new(0.36, 0.56, 0.95);
    let day_horizon = Vec3::new(0.72, 0.84, 0.98);
    let night_zenith = Vec3::new(0.01, 0.015, 0.05);
    let night_horizon = Vec3::new(0.04, 0.05, 0.11);
    let mut zenith = lerp(night_zenith, day_zenith, day);
    let horizon = lerp(night_horizon, day_horizon, day);
    // The warm glow itself is drawn towards the sun by the sky shader;
    // here the whole sky only cools slightly.
    let dusk = (1.0 - (elevation / 0.3).abs()).clamp(0.0, 1.0);
    zenith = lerp(zenith, Vec3::new(0.26, 0.28, 0.5), dusk * 0.3);
    let fog = lerp(horizon, zenith, 0.25);
    Sky {
        zenith,
        horizon,
        fog: lerp(fog, Vec3::new(0.85, 0.6, 0.5), dusk * 0.25),
        sun,
        daylight,
        stars: 1.0 - smooth(-0.3, 0.05, elevation),
        dusk,
        phase,
    }
}

/// Days elapsed (starting at day 1) and a 24-hour clock where tick 0 is
/// 06:00, as players expect sunrise in the morning.
pub fn clock(time: f64) -> (u64, u32, u32) {
    let day = (time / DAY).floor().max(0.0) as u64 + 1;
    let minutes = ((time.rem_euclid(DAY) / DAY * 1440.0) as u32 + 360) % 1440;
    (day, minutes / 60, minutes % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lum(v: Vec3) -> f32 {
        v.x * 0.3 + v.y * 0.59 + v.z * 0.11
    }

    #[test]
    fn noon_is_bright_midnight_dark_and_the_cycle_repeats() {
        let noon = sky(6_000.0);
        let midnight = sky(18_000.0);
        assert!(noon.sun.y > 0.95 && midnight.sun.y < -0.95);
        assert!((noon.daylight - 1.0).abs() < 1e-4);
        assert!((midnight.daylight - 0.2).abs() < 1e-4);
        assert!(lum(noon.zenith) > lum(midnight.zenith) * 8.0);
        assert!(noon.stars < 0.01 && midnight.stars > 0.99);
        assert!(noon.zenith.z > noon.zenith.x, "blue day sky");
        let sunrise = sky(0.0);
        assert!(sunrise.sun.x > 0.9, "rises in the east");
        assert!(sunrise.dusk > 0.9 && noon.dusk == 0.0 && midnight.dusk == 0.0);
        assert!(
            noon.horizon.z > noon.horizon.x,
            "noon horizon is not orange"
        );
        let a = sky(1_234.0);
        let b = sky(1_234.0 + 3.0 * DAY);
        assert!((a.zenith - b.zenith).length() < 1e-4);
        // Daylight changes smoothly: no jumps between consecutive ticks.
        let mut previous = sky(0.0).daylight;
        for t in 1..24_000 {
            let d = sky(t as f64).daylight;
            assert!((d - previous).abs() < 0.002);
            previous = d;
        }
    }

    #[test]
    fn clock_starts_at_six_in_the_morning() {
        assert_eq!(clock(0.0), (1, 6, 0));
        assert_eq!(clock(6_000.0), (1, 12, 0));
        assert_eq!(clock(18_000.0), (1, 0, 0));
        assert_eq!(clock(DAY + 1_000.0), (2, 7, 0));
    }
}
