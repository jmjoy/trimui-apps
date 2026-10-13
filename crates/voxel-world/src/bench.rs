//! Scripted fly-through benchmark with frame-time statistics.
use glam::Vec3;

/// Seconds excluded from statistics while the first chunks load.
pub const WARMUP: f64 = 3.0;

/// Deterministic camera route: (position, yaw, pitch) at time `t`.
///
/// The route alternates long straight flights, which stream new terrain, with
/// slow turns, which exercise frustum changes over already loaded chunks.
pub fn route(origin: Vec3, t: f64, speed: f64) -> (Vec3, f32, f32) {
    // Leg durations in seconds: straight, turn, straight, turn, ...
    const LEG: f64 = 12.0;
    const TURN: f64 = 4.0;
    let cycle = LEG + TURN;
    let n = (t / cycle).floor();
    let local = t - n * cycle;
    let mut position = origin;
    // Legs alternate between north and east, turning right then left, so
    // the flight keeps reaching new terrain instead of circling.
    let heading_of = |i: i64| {
        if i % 2 == 0 {
            0.0
        } else {
            std::f64::consts::FRAC_PI_2
        }
    };
    for i in 0..n as i64 {
        position += direction(heading_of(i)) * (speed * LEG) as f32;
    }
    let heading = heading_of(n as i64);
    let turn = if n as i64 % 2 == 0 { 1.0 } else { -1.0 } * std::f64::consts::FRAC_PI_2;
    let yaw = if local < LEG {
        position += direction(heading) * (speed * local) as f32;
        heading
    } else {
        position += direction(heading) * (speed * LEG) as f32;
        heading + (local - LEG) / TURN * turn
    };
    let pitch = -0.22 + 0.12 * (t * 0.7).sin();
    (
        position,
        yaw.rem_euclid(std::f64::consts::TAU) as f32,
        pitch as f32,
    )
}

fn direction(yaw: f64) -> Vec3 {
    Vec3::new(yaw.sin() as f32, 0.0, -yaw.cos() as f32)
}

/// Per-frame CPU time split, in milliseconds.
#[derive(Clone, Copy, Default, Debug)]
pub struct Timings {
    pub update: f32,
    pub upload: f32,
    pub render: f32,
    pub present: f32,
}

#[derive(Default)]
pub struct Stats {
    frames: Vec<f32>,
    sum: Timings,
}

impl Stats {
    pub fn record(&mut self, frame_ms: f32, timings: Timings) {
        self.frames.push(frame_ms);
        self.sum.update += timings.update;
        self.sum.upload += timings.upload;
        self.sum.render += timings.render;
        self.sum.present += timings.present;
    }
    pub fn frames(&self) -> usize {
        self.frames.len()
    }
    /// Returns (average FPS, 1% low FPS, worst frame ms, average timings).
    pub fn summary(&self) -> (f32, f32, f32, Timings) {
        if self.frames.is_empty() {
            return (0.0, 0.0, 0.0, Timings::default());
        }
        let n = self.frames.len() as f32;
        let total: f32 = self.frames.iter().sum();
        let mut sorted = self.frames.clone();
        sorted.sort_by(f32::total_cmp);
        // Average of the slowest 1% of frames, as commonly reported.
        let slow = (sorted.len() / 100).max(1);
        let p1 = sorted[sorted.len() - slow..].iter().sum::<f32>() / slow as f32;
        let worst = *sorted.last().unwrap();
        let avg = Timings {
            update: self.sum.update / n,
            upload: self.sum.upload / n,
            render: self.sum.render / n,
            present: self.sum.present / n,
        };
        (1000.0 * n / total, 1000.0 / p1, worst, avg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_is_continuous_and_turns() {
        let origin = Vec3::new(0.0, 80.0, 0.0);
        let mut previous = route(origin, 0.0, 9.0);
        for i in 1..2000 {
            let t = i as f64 * 0.05;
            let next = route(origin, t, 9.0);
            assert!(next.0.distance(previous.0) < 0.5, "jump at {t}");
            assert!((next.0.y - 80.0).abs() < 1e-3);
            assert!(next.2 > -0.4 && next.2 < 0.0);
            previous = next;
        }
        let (a, yaw_a, _) = route(origin, 10.0, 9.0);
        assert!(a.z < -80.0 && yaw_a.abs() < 1e-4);
        let (_, yaw_b, _) = route(origin, 20.0, 9.0);
        assert!((yaw_b - std::f32::consts::FRAC_PI_2).abs() < 1e-3);
        // The route keeps moving away from the start (a diagonal staircase).
        let far = route(origin, 200.0, 10.0).0;
        assert!(far.distance(origin) > 1000.0, "{far}");
        let (_, yaw_c, _) = route(origin, 36.0, 9.0);
        assert!(yaw_c.abs() < 1e-3, "back to north: {yaw_c}");
    }

    #[test]
    fn summary_reports_low_percentile() {
        let mut stats = Stats::default();
        for i in 0..200 {
            let ms = if i % 100 == 0 { 50.0 } else { 16.0 };
            stats.record(
                ms,
                Timings {
                    update: 1.0,
                    ..Default::default()
                },
            );
        }
        let (avg, low, worst, timings) = stats.summary();
        assert!((avg - 1000.0 / 16.34).abs() < 0.5, "{avg}");
        assert!((low - 20.0).abs() < 0.01, "{low}");
        assert_eq!(worst, 50.0);
        assert_eq!(timings.update, 1.0);
        assert_eq!(stats.frames(), 200);
    }
}
