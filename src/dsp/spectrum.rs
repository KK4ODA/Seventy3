//! The spectrum chain: raw IF samples in, spectrum frames centred on the dial out.
//!
//! ```text
//! IQ ─▶ DC removal ─▶ shift dial to 0 Hz ─▶ un-mirror ─▶ decimate 2^zoom ─▶
//!       windowed FFT (50 % overlap) ─▶ average per frame ─▶ pool into columns ─▶ dB
//! ```
//!
//! Every frame's frequency axis is "Hz relative to the dial", already
//! un-mirrored, so the UI never has to think about the IF.

use std::sync::Arc;

use rustfft::{Fft, FftPlanner};

use super::Complex32;
use super::filters::{DcBlocker, Decimator, Shifter};

/// Deepest zoom offered: decimation by 2^6 = 64.
pub const MAX_ZOOM: u32 = 6;

/// Fraction of each band shown. The outer edges are lost to filter roll-off
/// (the RTL-SDR's own at zoom 0, the halfband stages above that).
const USABLE_FRACTION: f64 = 0.8;

#[derive(Debug, Clone, PartialEq)]
pub struct ChainConfig {
    /// Input sample rate, Hz.
    pub sample_rate: f64,
    /// Where the dial frequency sits in the input baseband, Hz
    /// (IF centre minus the frequency the RTL-SDR is tuned to).
    pub dial_offset_hz: f64,
    /// The IF spectrum is mirrored relative to RF.
    pub inverted: bool,
    /// Decimation stages: each halves the span and doubles the resolution.
    pub zoom: u32,
    pub frames_per_sec: f32,
    /// Columns per frame are capped at this; finer bins are max-pooled.
    pub max_columns: usize,
}

impl ChainConfig {
    pub fn decimated_rate(&self) -> f64 {
        self.sample_rate / f64::from(1u32 << self.zoom.min(MAX_ZOOM))
    }

    /// Half the displayed span, Hz. The display is centred on the dial.
    pub fn half_span_hz(&self) -> f64 {
        // At zoom 0 nothing is filtered, so the usable band is the RTL-SDR's,
        // which is centred on the tuner, not on the dial.
        let unzoomed = (USABLE_FRACTION * self.sample_rate / 2.0 - self.dial_offset_hz.abs()).max(1.0);
        let zoomed = USABLE_FRACTION * self.decimated_rate() / 2.0;
        if self.zoom == 0 { unzoomed } else { zoomed.min(unzoomed) }
    }

    /// FFT length: as fine as possible while each FFT covers at most ~80 ms,
    /// so the waterfall still moves smoothly.
    pub fn fft_size(&self) -> usize {
        let target = (self.decimated_rate() * 0.08) as usize;
        let pow2 = if target < 2 { 2 } else { 1usize << (usize::BITS - 1 - target.leading_zeros()) };
        pow2.clamp(1024, 8192)
    }

    pub fn bin_hz(&self) -> f64 {
        self.decimated_rate() / self.fft_size() as f64
    }
}

/// One line of spectrum, in dB relative to full scale.
#[derive(Debug, Clone)]
pub struct SpectrumFrame {
    /// Increments whenever the chain is rebuilt with a new geometry.
    pub generation: u64,
    pub db: Vec<f32>,
    /// Offset from the dial of the centre of column 0, Hz.
    pub start_hz: f64,
    /// Width of each column, Hz.
    pub col_hz: f64,
}

impl SpectrumFrame {
    /// Offset from the dial of the centre of column `i`.
    pub fn column_hz(&self, i: usize) -> f64 {
        self.start_hz + i as f64 * self.col_hz
    }
}

pub struct SpectrumChain {
    cfg: ChainConfig,
    generation: u64,
    dc: DcBlocker,
    shift: Shifter,
    decim: Decimator,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    /// Converts summed |X|² to power relative to a full-scale tone.
    norm: f32,
    n: usize,
    hop: usize,
    pending: Vec<Complex32>,
    work: Vec<Complex32>,
    fft_scratch: Vec<Complex32>,
    power: Vec<f32>,
    power_count: u32,
    since_frame: usize,
    frame_interval: usize,
    /// First FFT bin (after fftshift) of the displayed range.
    first_bin: usize,
    pool: usize,
    columns: usize,
    start_hz: f64,
    col_hz: f64,
}

impl SpectrumChain {
    pub fn new(cfg: ChainConfig, generation: u64) -> Self {
        let n = cfg.fft_size();
        let fft = FftPlanner::new().plan_fft_forward(n);
        let window = blackman_harris(n);
        let wsum: f32 = window.iter().sum();
        let bin_hz = cfg.bin_hz();

        // Bins within the span, symmetric about the dial (bin n/2).
        let half_bins = ((cfg.half_span_hz() / bin_hz).floor() as usize).min(n / 2 - 1);
        let count = 2 * half_bins + 1;
        let pool = count.div_ceil(cfg.max_columns.max(16));
        let columns = count / pool;
        let used = columns * pool;
        let first_bin = n / 2 - used / 2;
        let start_hz = (first_bin as f64 + (pool as f64 - 1.0) / 2.0 - (n / 2) as f64) * bin_hz;

        let frame_interval = ((cfg.decimated_rate() / f64::from(cfg.frames_per_sec.max(1.0))) as usize).max(1);
        let scratch_len = fft.get_inplace_scratch_len();
        SpectrumChain {
            shift: Shifter::new(-cfg.dial_offset_hz, cfg.sample_rate),
            decim: Decimator::new(cfg.zoom.min(MAX_ZOOM)),
            dc: DcBlocker::default(),
            fft,
            window,
            norm: 1.0 / (wsum * wsum),
            n,
            hop: n / 2,
            pending: Vec::with_capacity(2 * n),
            work: vec![Complex32::default(); n],
            fft_scratch: vec![Complex32::default(); scratch_len],
            power: vec![0.0; n],
            power_count: 0,
            since_frame: 0,
            frame_interval,
            first_bin,
            pool,
            columns,
            start_hz,
            col_hz: pool as f64 * bin_hz,
            cfg,
            generation,
        }
    }

    pub fn config(&self) -> &ChainConfig {
        &self.cfg
    }

    pub fn columns(&self) -> usize {
        self.columns
    }

    /// Feeds a block of raw IF samples; completed frames are pushed to `out`.
    pub fn process(&mut self, mut block: Vec<Complex32>, out: &mut Vec<SpectrumFrame>) {
        self.dc.process(&mut block);
        self.shift.process(&mut block);
        if self.cfg.inverted {
            for s in &mut block {
                s.im = -s.im;
            }
        }
        self.decim.process(&mut block);
        self.pending.extend_from_slice(&block);

        let n = self.n;
        let mut consumed = 0;
        while self.pending.len() - consumed >= n {
            let src = &self.pending[consumed..consumed + n];
            for ((w, s), win) in self.work.iter_mut().zip(src).zip(&self.window) {
                *w = s * *win;
            }
            self.fft.process_with_scratch(&mut self.work, &mut self.fft_scratch);
            // Accumulate with fftshift: output index 0 is -fs/2.
            let half = n / 2;
            for (i, x) in self.work.iter().enumerate() {
                let shifted = (i + half) % n;
                self.power[shifted] += x.norm_sqr();
            }
            self.power_count += 1;
            consumed += self.hop;
            self.since_frame += self.hop;
            // Emit when the next hop would overshoot the interval by more
            // than it undershoots now, so narrow spans (one FFT per frame or
            // less) still run near the requested frame rate.
            if self.since_frame + self.hop / 2 >= self.frame_interval {
                out.push(self.emit());
            }
        }
        self.pending.drain(..consumed);
    }

    fn emit(&mut self) -> SpectrumFrame {
        let scale = self.norm / self.power_count.max(1) as f32;
        let mut db = Vec::with_capacity(self.columns);
        for c in 0..self.columns {
            let lo = self.first_bin + c * self.pool;
            let peak = self.power[lo..lo + self.pool].iter().copied().fold(0.0f32, f32::max);
            db.push(10.0 * (peak * scale + 1e-20).log10());
        }
        self.power.iter_mut().for_each(|p| *p = 0.0);
        self.power_count = 0;
        self.since_frame = 0;
        SpectrumFrame { generation: self.generation, db, start_hz: self.start_hz, col_hz: self.col_hz }
    }
}

/// 4-term Blackman-Harris window: -92 dB sidelobes, so weak stations next
/// to strong ones stay visible.
fn blackman_harris(n: usize) -> Vec<f32> {
    let (a0, a1, a2, a3) = (0.35875, 0.48829, 0.14128, 0.01168);
    let m = (n - 1) as f64;
    (0..n)
        .map(|i| {
            let x = 2.0 * std::f64::consts::PI * i as f64 / m;
            (a0 - a1 * x.cos() + a2 * (2.0 * x).cos() - a3 * (3.0 * x).cos()) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::ifmap::IfMap;
    use std::f64::consts::PI;

    /// IF samples as the RTL-SDR would see them: one carrier on `rf_hz`,
    /// with the rig on `dial_hz` and the tuner at `rtl_center_hz`.
    fn if_samples(
        map: &IfMap,
        dial_hz: f64,
        rf_hz: f64,
        rtl_center_hz: f64,
        fs: f64,
        n: usize,
        amp: f32,
    ) -> Vec<Complex32> {
        let bb = map.if_from_rf(dial_hz, rf_hz) - rtl_center_hz;
        (0..n)
            .map(|i| {
                let p = 2.0 * PI * bb * i as f64 / fs;
                Complex32::new(p.cos() as f32, p.sin() as f32) * amp
            })
            .collect()
    }

    fn run(cfg: ChainConfig, input: Vec<Complex32>) -> Vec<SpectrumFrame> {
        let mut chain = SpectrumChain::new(cfg, 1);
        let mut frames = Vec::new();
        for block in input.chunks(16_384) {
            chain.process(block.to_vec(), &mut frames);
        }
        frames
    }

    fn peak(frame: &SpectrumFrame) -> (f64, f32) {
        let (i, db) = frame.db.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
        (frame.column_hz(i), *db)
    }

    fn cfg(zoom: u32, inverted: bool) -> ChainConfig {
        ChainConfig {
            sample_rate: 2_400_000.0,
            dial_offset_hz: -150_000.0,
            inverted,
            zoom,
            frames_per_sec: 25.0,
            max_columns: 4096,
        }
    }

    #[test]
    fn carrier_appears_at_its_offset_from_the_dial() {
        let map = IfMap::default();
        let fs = 2_400_000.0;
        let rtl = map.if_center_hz + 150_000.0;
        for zoom in [0, 2, 5] {
            let c = cfg(zoom, true);
            let input = if_samples(&map, 14_285_000.0, 14_286_000.0, rtl, fs, 480_000, 0.5);
            let frames = run(c.clone(), input);
            assert!(frames.len() >= 3, "zoom {zoom}: only {} frames", frames.len());
            let (f, db) = peak(frames.last().unwrap());
            assert!(
                (f - 1000.0).abs() <= frames[0].col_hz,
                "zoom {zoom}: peak at {f} Hz, column width {}",
                frames[0].col_hz
            );
            // A 0.5-amplitude tone is -6 dBFS (pooling and scalloping cost a little).
            assert!((-8.0..=-5.5).contains(&db), "zoom {zoom}: level {db}");
        }
    }

    #[test]
    fn signal_below_the_dial_stays_below_when_unmirrored() {
        let map = IfMap::default();
        let fs = 2_400_000.0;
        let rtl = map.if_center_hz + 150_000.0;
        let input = if_samples(&map, 7_200_000.0, 7_150_000.0, rtl, fs, 240_000, 0.5);
        let frames = run(cfg(1, true), input);
        let (f, _) = peak(frames.last().unwrap());
        assert!((f + 50_000.0).abs() < 200.0, "peak at {f}");
    }

    #[test]
    fn upright_ifs_are_supported() {
        let map = IfMap { inverted: false, ..IfMap::default() };
        let fs = 2_400_000.0;
        let rtl = map.if_center_hz + 150_000.0;
        let input = if_samples(&map, 14_200_000.0, 14_230_000.0, rtl, fs, 240_000, 0.5);
        let frames = run(cfg(1, false), input);
        let (f, _) = peak(frames.last().unwrap());
        assert!((f - 30_000.0).abs() < 200.0, "peak at {f}");
    }

    #[test]
    fn geometry_is_centred_and_capped() {
        for zoom in 0..=MAX_ZOOM {
            let c = cfg(zoom, true);
            let chain = SpectrumChain::new(c.clone(), 0);
            assert!(chain.columns() <= 4096);
            let first = chain.start_hz;
            let last = chain.start_hz + (chain.columns() - 1) as f64 * chain.col_hz;
            assert!((first + last).abs() <= chain.col_hz, "zoom {zoom}: {first}..{last}");
            assert!(last <= c.half_span_hz() + chain.col_hz);
        }
    }

    #[test]
    fn frame_rate_follows_config() {
        let map = IfMap::default();
        let fs = 2_400_000.0;
        let input = if_samples(&map, 14e6, 14e6, map.if_center_hz + 150e3, fs, 2_400_000, 0.1);
        let frames = run(cfg(0, true), input);
        // One second of input at 25 frames per second.
        assert!((22..=27).contains(&frames.len()), "{} frames", frames.len());
    }
}
