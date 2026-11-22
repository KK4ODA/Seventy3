//! Building blocks: DC removal, frequency shifting and decimation.

use super::Complex32;
use std::f64::consts::PI;

/// Removes the DC offset of the RTL-SDR's ADC, which otherwise shows as a
/// spike. Tracks the mean block by block, with a time constant of a few blocks.
#[derive(Debug, Default)]
pub struct DcBlocker {
    mean: Complex32,
    primed: bool,
}

impl DcBlocker {
    pub fn process(&mut self, samples: &mut [Complex32]) {
        if samples.is_empty() {
            return;
        }
        let sum: Complex32 = samples.iter().sum();
        let block_mean = sum / samples.len() as f32;
        if self.primed {
            self.mean += (block_mean - self.mean) * 0.2;
        } else {
            self.mean = block_mean;
            self.primed = true;
        }
        let m = self.mean;
        for s in samples {
            *s -= m;
        }
    }
}

/// Shifts the spectrum by a fixed frequency (a numerically controlled oscillator).
#[derive(Debug)]
pub struct Shifter {
    phasor: Complex32,
    step: Complex32,
    since_normalize: u32,
    enabled: bool,
}

impl Shifter {
    /// Moves every component up by `shift_hz` (a negative value moves it down).
    pub fn new(shift_hz: f64, sample_rate: f64) -> Self {
        let w = 2.0 * PI * shift_hz / sample_rate;
        Shifter {
            phasor: Complex32::new(1.0, 0.0),
            step: Complex32::new(w.cos() as f32, w.sin() as f32),
            since_normalize: 0,
            enabled: shift_hz != 0.0,
        }
    }

    pub fn process(&mut self, samples: &mut [Complex32]) {
        if !self.enabled {
            return;
        }
        for chunk in samples.chunks_mut(1024) {
            for s in chunk.iter_mut() {
                *s *= self.phasor;
                self.phasor *= self.step;
            }
            // Keep |phasor| = 1 despite rounding.
            self.since_normalize += chunk.len() as u32;
            if self.since_normalize >= 1024 {
                self.phasor /= self.phasor.norm();
                self.since_normalize = 0;
            }
        }
    }
}

/// Number of taps in each halfband stage. With a Kaiser window this keeps
/// aliases into the inner 80 % of the output band below about -80 dB.
const HALFBAND_TAPS: usize = 51;

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half = x / 2.0;
    for k in 1..50 {
        term *= (half / k as f64) * (half / k as f64);
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// Halfband low-pass coefficients: only the centre tap and odd offsets are
/// non-zero. Returns (centre, taps at offsets ±1, ±3, ±5, ...).
fn halfband_design() -> (f32, Vec<f32>) {
    let n = HALFBAND_TAPS as i64;
    let half = (n - 1) / 2;
    let beta = 7.86; // Kaiser beta for ~80 dB
    let i0b = bessel_i0(beta);
    let mut taps = Vec::new();
    let mut raw = vec![0.0f64; HALFBAND_TAPS];
    for (i, h) in raw.iter_mut().enumerate() {
        let k = i as i64 - half;
        let sinc = if k == 0 {
            0.5
        } else {
            let x = PI * k as f64 / 2.0;
            0.5 * x.sin() / x
        };
        let r = k as f64 / half as f64;
        let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / i0b;
        *h = sinc * w;
    }
    // Unity gain at DC.
    let sum: f64 = raw.iter().sum();
    for h in &mut raw {
        *h /= sum;
    }
    let center = raw[half as usize] as f32;
    let mut off = 1;
    while off <= half {
        taps.push(raw[(half + off) as usize] as f32);
        off += 2;
    }
    (center, taps)
}

/// Decimates by two with a halfband low-pass filter.
#[derive(Debug, Clone)]
pub struct HalfbandDecimator {
    center: f32,
    taps: Vec<f32>,
    history: Vec<Complex32>,
}

impl Default for HalfbandDecimator {
    fn default() -> Self {
        let (center, taps) = halfband_design();
        HalfbandDecimator { center, taps, history: vec![Complex32::default(); HALFBAND_TAPS - 1] }
    }
}

impl HalfbandDecimator {
    pub fn process(&mut self, input: &[Complex32], out: &mut Vec<Complex32>) {
        self.history.extend_from_slice(input);
        let half = (HALFBAND_TAPS - 1) / 2;
        let h = &self.history;
        let mut start = 0;
        while start + HALFBAND_TAPS <= h.len() {
            let c = start + half;
            let mut acc = h[c] * self.center;
            for (k, &t) in self.taps.iter().enumerate() {
                let o = 2 * k + 1;
                acc += (h[c - o] + h[c + o]) * t;
            }
            out.push(acc);
            start += 2;
        }
        self.history.drain(..start);
    }
}

/// A cascade of halfband decimators: decimates by `2^stages`.
#[derive(Debug, Clone, Default)]
pub struct Decimator {
    stages: Vec<HalfbandDecimator>,
    scratch: Vec<Complex32>,
}

impl Decimator {
    pub fn new(stages: u32) -> Self {
        Decimator { stages: (0..stages).map(|_| HalfbandDecimator::default()).collect(), scratch: Vec::new() }
    }

    /// Replaces `samples` with the decimated signal.
    pub fn process(&mut self, samples: &mut Vec<Complex32>) {
        for stage in &mut self.stages {
            self.scratch.clear();
            stage.process(samples, &mut self.scratch);
            std::mem::swap(samples, &mut self.scratch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f64, fs: f64, n: usize) -> Vec<Complex32> {
        (0..n)
            .map(|i| {
                let p = 2.0 * PI * freq * i as f64 / fs;
                Complex32::new(p.cos() as f32, p.sin() as f32)
            })
            .collect()
    }

    fn rms(x: &[Complex32]) -> f32 {
        (x.iter().map(|c| c.norm_sqr()).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn dc_blocker_removes_offset() {
        let mut x = tone(1000.0, 48_000.0, 4800);
        for s in &mut x {
            *s += Complex32::new(0.3, -0.2);
        }
        let mut dc = DcBlocker::default();
        dc.process(&mut x);
        let mean: Complex32 = x.iter().sum::<Complex32>() / x.len() as f32;
        assert!(mean.norm() < 1e-3, "{mean}");
    }

    #[test]
    fn shifter_moves_a_tone() {
        let fs = 48_000.0;
        let mut x = tone(1000.0, fs, 4800);
        Shifter::new(2000.0, fs).process(&mut x);
        let expect = tone(3000.0, fs, 4800);
        for (a, b) in x.iter().zip(&expect) {
            assert!((a - b).norm() < 1e-3);
        }
    }

    #[test]
    fn halfband_passes_inner_band_and_rejects_alias_band() {
        let fs = 1_000_000.0;
        // 0.18 fs: inside the passband; keeps its level.
        let mut d = Decimator::new(1);
        let mut x = tone(0.18 * fs, fs, 20_000);
        d.process(&mut x);
        let level = rms(&x[100..]);
        assert!((level - 1.0).abs() < 0.01, "passband level {level}");

        // 0.32 fs: would alias to -0.18 of the input rate; must be suppressed.
        let mut d = Decimator::new(1);
        let mut x = tone(0.32 * fs, fs, 20_000);
        d.process(&mut x);
        let level = rms(&x[100..]);
        assert!(level < 1e-3, "alias level {level}");
    }

    #[test]
    fn decimator_streams_across_block_boundaries() {
        let fs = 1_000_000.0;
        let x = tone(20_000.0, fs, 40_000);
        let mut whole = x.clone();
        Decimator::new(3).process(&mut whole);

        let mut d = Decimator::new(3);
        let mut pieces = Vec::new();
        for chunk in x.chunks(777) {
            let mut c = chunk.to_vec();
            d.process(&mut c);
            pieces.extend(c);
        }
        assert_eq!(whole.len(), pieces.len());
        for (a, b) in whole.iter().zip(&pieces) {
            assert!((a - b).norm() < 1e-5);
        }
        // The filters start primed with zeros, so there is no start-up loss.
        assert_eq!(whole.len(), 40_000 / 8);
    }
}
