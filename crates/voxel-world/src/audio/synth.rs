//! DSP building blocks for offline sound synthesis.
//!
//! Everything here runs on a background thread while the sound bank is
//! generated; nothing is used from the real-time audio callback.

/// Small deterministic xorshift64* generator.
#[derive(Clone)]
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut rng = Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        rng.next_u64();
        rng
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    /// Uniform in [0, 1).
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f32()
    }
    /// Uniform in [-1, 1).
    pub fn noise(&mut self) -> f32 {
        self.f32() * 2.0 - 1.0
    }
    pub fn below(&mut self, n: usize) -> usize {
        ((self.next_u64() >> 33) as usize) % n.max(1)
    }
    pub fn count(&mut self, range: std::ops::RangeInclusive<usize>) -> usize {
        range.start() + self.below(range.end() - range.start() + 1)
    }
}

/// RBJ cookbook biquad in transposed direct form II.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}
impl Biquad {
    fn design(rate: f32, frequency: f32, q: f32) -> (f32, f32) {
        let f = frequency.clamp(10.0, rate * 0.45);
        let w = std::f32::consts::TAU * f / rate;
        (w.cos(), w.sin() / (2.0 * q.max(0.05)))
    }
    fn normalized(b0: f32, b1: f32, b2: f32, a0: f32, a1: f32, a2: f32) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }
    pub fn lowpass(rate: f32, frequency: f32, q: f32) -> Self {
        let (c, alpha) = Self::design(rate, frequency, q);
        Self::normalized(
            (1.0 - c) / 2.0,
            1.0 - c,
            (1.0 - c) / 2.0,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        )
    }
    pub fn highpass(rate: f32, frequency: f32, q: f32) -> Self {
        let (c, alpha) = Self::design(rate, frequency, q);
        Self::normalized(
            (1.0 + c) / 2.0,
            -(1.0 + c),
            (1.0 + c) / 2.0,
            1.0 + alpha,
            -2.0 * c,
            1.0 - alpha,
        )
    }
    /// Band-pass with a 0 dB peak at the centre frequency.
    pub fn bandpass(rate: f32, frequency: f32, q: f32) -> Self {
        let (c, alpha) = Self::design(rate, frequency, q);
        Self::normalized(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * c, 1.0 - alpha)
    }
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Filter {
    Lp(f32, f32),
    Hp(f32, f32),
    Bp(f32, f32),
}
impl Filter {
    fn build(self, rate: f32) -> Biquad {
        match self {
            Filter::Lp(f, q) => Biquad::lowpass(rate, f, q),
            Filter::Hp(f, q) => Biquad::highpass(rate, f, q),
            Filter::Bp(f, q) => Biquad::bandpass(rate, f, q),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Color {
    White,
    /// −3 dB/octave (Paul Kellet's filter).
    Pink,
    /// −6 dB/octave (leaky integrator).
    Brown,
}

/// Noise source of a given colour.
pub struct Noise {
    color: Color,
    state: [f32; 7],
}
impl Noise {
    pub fn new(color: Color) -> Self {
        Self {
            color,
            state: [0.0; 7],
        }
    }
    pub fn sample(&mut self, rng: &mut Rng) -> f32 {
        let white = rng.noise();
        let b = &mut self.state;
        match self.color {
            Color::White => white,
            Color::Pink => {
                b[0] = 0.99886 * b[0] + white * 0.055_517_9;
                b[1] = 0.99332 * b[1] + white * 0.075_075_9;
                b[2] = 0.96900 * b[2] + white * 0.153_852;
                b[3] = 0.86650 * b[3] + white * 0.310_485_6;
                b[4] = 0.55000 * b[4] + white * 0.532_952_2;
                b[5] = -0.7616 * b[5] - white * 0.016_898;
                let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + white * 0.5362;
                b[6] = white * 0.115_926;
                pink * 0.11
            }
            Color::Brown => {
                b[0] = (b[0] + 0.02 * white) / 1.02;
                b[0] * 3.5
            }
        }
    }
}

/// One damped sinusoidal mode: (frequency Hz, decay time constant s, amplitude).
pub type Mode = (f32, f32, f32);

/// Mono sample buffer being synthesised.
pub struct Buf {
    pub rate: f32,
    pub data: Vec<f32>,
}
impl Buf {
    pub fn new(rate: f32, seconds: f32) -> Self {
        Self {
            rate,
            data: vec![0.0; (rate * seconds).ceil().max(1.0) as usize],
        }
    }
    fn span(&self, start: f32, length: f32) -> std::ops::Range<usize> {
        let a = ((start * self.rate).max(0.0) as usize).min(self.data.len());
        let b = (((start + length) * self.rate).max(0.0) as usize).min(self.data.len());
        a..b
    }
    /// Filtered noise with a linear attack, exponential decay and a short
    /// fade at `length` so the layer never ends abruptly.
    #[allow(clippy::too_many_arguments)]
    pub fn noise(
        &mut self,
        rng: &mut Rng,
        start: f32,
        attack: f32,
        tau: f32,
        length: f32,
        color: Color,
        filters: &[Filter],
        amp: f32,
    ) {
        let mut source = Noise::new(color);
        let mut chain: Vec<Biquad> = filters.iter().map(|f| f.build(self.rate)).collect();
        let fade = (length * 0.25).min(0.01);
        for (i, index) in self.span(start, length).enumerate() {
            let t = i as f32 / self.rate;
            let mut x = source.sample(rng);
            for f in &mut chain {
                x = f.process(x);
            }
            let env = (t / attack.max(1e-4)).min(1.0)
                * (-(t - attack).max(0.0) / tau).exp()
                * ((length - t) / fade).clamp(0.0, 1.0);
            self.data[index] += x * env * amp;
        }
    }
    /// Short Hann-windowed burst of band-passed white noise.
    pub fn grain(&mut self, rng: &mut Rng, start: f32, length: f32, center: f32, q: f32, amp: f32) {
        let mut bp = Biquad::bandpass(self.rate, center, q);
        // Two cascaded stages give the grain a clearer pitch-less "crunch".
        let mut bp2 = Biquad::bandpass(self.rate, center * 1.07, q);
        let n = (length * self.rate).max(2.0);
        for (i, index) in self.span(start, length).enumerate() {
            let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n).cos();
            let x = bp2.process(bp.process(rng.noise()));
            self.data[index] += x * w * amp * 2.5;
        }
    }
    /// Sum of damped modes with a sub-millisecond attack.
    pub fn modes(&mut self, rng: &mut Rng, start: f32, modes: &[Mode], amp: f32) {
        let longest = modes.iter().map(|m| m.1).fold(0.0, f32::max);
        let length = longest * 7.0;
        let phases: Vec<f32> = modes
            .iter()
            .map(|_| rng.range(0.0, std::f32::consts::TAU))
            .collect();
        for (i, index) in self.span(start, length).enumerate() {
            let t = i as f32 / self.rate;
            let attack = (t / 0.0004).min(1.0);
            let mut x = 0.0;
            for (&(f, tau, a), phase) in modes.iter().zip(&phases) {
                x += a * (std::f32::consts::TAU * f * t + phase).sin() * (-t / tau).exp();
            }
            self.data[index] += x * attack * amp;
        }
    }
    /// Maximum RMS over 30 ms windows, a rough loudness measure.
    pub fn loudness(data: &[f32], rate: f32) -> f32 {
        let window = ((rate * 0.03) as usize).max(1);
        let mut best = 0.0f32;
        let mut sum = 0.0f32;
        for (i, x) in data.iter().enumerate() {
            sum += x * x;
            if i >= window {
                sum -= data[i - window] * data[i - window];
            }
            best = best.max(sum.max(0.0) / window.min(i + 1) as f32);
        }
        best.sqrt()
    }
    /// Scales to the target loudness, limits the peak, trims trailing
    /// silence and fades the tail.
    pub fn finish(mut self, target: f32) -> Vec<f32> {
        let loudness = Self::loudness(&self.data, self.rate);
        if loudness > 1e-9 {
            let mut gain = target / loudness;
            let peak = self.data.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            gain = gain.min(0.94 / peak.max(1e-9));
            for x in &mut self.data {
                *x *= gain;
            }
        }
        let peak = self.data.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let end = self
            .data
            .iter()
            .rposition(|x| x.abs() > peak * 0.002)
            .map_or(1, |i| i + 1);
        self.data.truncate(end);
        let fade = ((self.rate * 0.005) as usize).min(self.data.len());
        let n = self.data.len();
        for i in 0..fade {
            self.data[n - 1 - i] *= i as f32 / fade as f32;
        }
        self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone_gain(mut f: Biquad, rate: f32, freq: f32) -> f32 {
        let mut peak = 0.0f32;
        for i in 0..(rate as usize / 2) {
            let y = f.process((std::f32::consts::TAU * freq * i as f32 / rate).sin());
            if i > rate as usize / 4 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn filters_pass_and_reject() {
        let r = 48_000.0;
        assert!(tone_gain(Biquad::lowpass(r, 500.0, 0.707), r, 100.0) > 0.95);
        assert!(tone_gain(Biquad::lowpass(r, 500.0, 0.707), r, 8000.0) < 0.01);
        assert!(tone_gain(Biquad::highpass(r, 2000.0, 0.707), r, 100.0) < 0.01);
        let bp = Biquad::bandpass(r, 1000.0, 2.0);
        assert!((tone_gain(bp, r, 1000.0) - 1.0).abs() < 0.02);
        assert!(tone_gain(bp, r, 4000.0) < 0.2);
    }

    #[test]
    fn rng_and_noise_are_deterministic_and_bounded() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
            let x = a.f32();
            assert!((0.0..1.0).contains(&x));
            b.f32();
        }
        for color in [Color::White, Color::Pink, Color::Brown] {
            let mut n = Noise::new(color);
            let mut rng = Rng::new(1);
            let max = (0..48_000)
                .map(|_| n.sample(&mut rng).abs())
                .fold(0.0, f32::max);
            assert!(max > 0.05 && max < 2.0, "{color:?} {max}");
        }
    }

    #[test]
    fn finish_normalizes_and_trims() {
        let mut buf = Buf::new(48_000.0, 1.0);
        let mut rng = Rng::new(3);
        buf.noise(&mut rng, 0.0, 0.002, 0.03, 0.3, Color::White, &[], 3.0);
        let data = buf.finish(0.2);
        assert!(data.len() < 48_000 / 2, "silence trimmed: {}", data.len());
        assert!(data.iter().all(|x| x.is_finite() && x.abs() <= 0.95));
        assert!((Buf::loudness(&data, 48_000.0) - 0.2).abs() < 0.02);
        assert_eq!(*data.last().unwrap(), 0.0);
    }
}
