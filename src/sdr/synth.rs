//! Demo IF signal: what the RTL-SDR would see on a TS-480's first IF with
//! the demo stations on the air, following the simulated rig's VFO.
//!
//! It produces real IQ samples so demo mode exercises the same DSP chain as
//! real hardware.

use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::{IqSource, ReadStats, SdrSettings};
use crate::demo::{self, DemoKind, DemoStation, Rng};
use crate::dsp::Complex32;
use crate::dsp::ifmap::IfMap;
use crate::rig::sim::SimShared;

/// Gains a typical R820T offers, tenths of a dB.
const GAINS: [i32; 29] = [
    0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338, 364, 372, 386, 402, 421, 434,
    439, 445, 480, 496,
];
const REFERENCE_GAIN: i32 = 207;
/// Noise amplitude at the reference gain: puts the floor near -100 dBFS per bin.
const NOISE_AMP: f32 = 0.004;

struct Tone {
    phase: f64,
    amp: f32,
}

struct Live {
    st: DemoStation,
    tones: Vec<Tone>,
    /// Keying pattern for CW stations.
    pattern: Vec<bool>,
}

pub struct SynthSource {
    fs: f64,
    tuner_hz: f64,
    rig: Arc<SimShared>,
    /// The "real" IF of the simulated rig. Calibration in the app moves the
    /// app's own map toward this one.
    truth: IfMap,
    stations: Vec<Live>,
    tx_tones: Vec<Tone>,
    rng: Rng,
    start: Instant,
    produced: u64,
    gain_scale: f32,
}

impl SynthSource {
    pub fn new(settings: &SdrSettings, tuner_hz: u32, rig: Arc<SimShared>) -> Self {
        let stations = demo::stations()
            .into_iter()
            .map(|st| {
                let n = match st.kind {
                    DemoKind::Voice { .. } => 5,
                    _ => 1,
                };
                let pattern = match st.kind {
                    DemoKind::Cw { .. } => cq_pattern(&st.call),
                    _ => Vec::new(),
                };
                Live { st, tones: (0..n).map(|_| Tone { phase: 0.0, amp: 0.0 }).collect(), pattern }
            })
            .collect();
        let mut s = SynthSource {
            fs: f64::from(settings.sample_rate),
            tuner_hz: f64::from(tuner_hz),
            rig,
            truth: IfMap::default(),
            stations,
            tx_tones: (0..5).map(|_| Tone { phase: 0.0, amp: 0.0 }).collect(),
            rng: Rng::new(1),
            start: Instant::now(),
            produced: 0,
            gain_scale: 1.0,
        };
        let _ = s.set_gain(settings.gain_tenths_db);
        s
    }

    /// Seconds since the Unix epoch, so FT8 periods line up with the clock.
    fn wall_time(&self) -> f64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
    }
}

/// Adds a tone ramping linearly from `amp0` to `amp1` over the block.
fn add_tone(out: &mut [Complex32], freq_hz: f64, fs: f64, phase: &mut f64, amp0: f32, amp1: f32) {
    let w = TAU * freq_hz / fs;
    if amp0 > 0.0 || amp1 > 0.0 {
        let mut ph = Complex32::from_polar(1.0, *phase as f32);
        let rot = Complex32::from_polar(1.0, w as f32);
        let da = (amp1 - amp0) / out.len() as f32;
        let mut a = amp0;
        for s in out.iter_mut() {
            *s += ph * a;
            ph *= rot;
            a += da;
        }
    }
    *phase = (*phase + w * out.len() as f64).rem_euclid(TAU);
}

/// Voice-like activity for a station at time `t`: (amplitude 0..1, formant
/// offsets in Hz for each of the five tones).
fn voice(seed: u64, t: f64) -> (f32, [f64; 5]) {
    // Over/under: talk for a while, then listen.
    let cycle = 10.0 + (seed % 1000) as f64 / 1000.0 * 14.0;
    let offset = (seed >> 10) as f64 % cycle;
    let pos = (t + offset) % cycle;
    let talking = pos < cycle * 0.62;
    // Syllables of ~180 ms.
    let slot = ((t + offset) / 0.18) as u64;
    let h = demo::hash(seed, slot);
    let on = talking && (h % 100) < 82;
    let amp = if on { 0.55 + ((h >> 8) % 45) as f32 / 100.0 } else { 0.0 };
    let jitter = |k: u64, lo: f64, hi: f64| lo + (demo::hash(h, k) % 1000) as f64 / 1000.0 * (hi - lo);
    let formants = [
        jitter(1, 300.0, 650.0),
        jitter(2, 700.0, 1150.0),
        jitter(3, 1200.0, 1800.0),
        jitter(4, 1850.0, 2400.0),
        jitter(5, 2400.0, 2750.0),
    ];
    (amp, formants)
}

const FORMANT_LEVELS: [f32; 5] = [1.0, 0.8, 0.5, 0.32, 0.18];

/// Morse for "CQ POTA DE <call> K" as key-down flags, one per dit length.
fn cq_pattern(call: &str) -> Vec<bool> {
    fn code(c: char) -> &'static str {
        match c {
            'A' => ".-",
            'B' => "-...",
            'C' => "-.-.",
            'D' => "-..",
            'E' => ".",
            'F' => "..-.",
            'G' => "--.",
            'H' => "....",
            'I' => "..",
            'J' => ".---",
            'K' => "-.-",
            'L' => ".-..",
            'M' => "--",
            'N' => "-.",
            'O' => "---",
            'P' => ".--.",
            'Q' => "--.-",
            'R' => ".-.",
            'S' => "...",
            'T' => "-",
            'U' => "..-",
            'V' => "...-",
            'W' => ".--",
            'X' => "-..-",
            'Y' => "-.--",
            'Z' => "--..",
            '0' => "-----",
            '1' => ".----",
            '2' => "..---",
            '3' => "...--",
            '4' => "....-",
            '5' => ".....",
            '6' => "-....",
            '7' => "--...",
            '8' => "---..",
            '9' => "----.",
            _ => "",
        }
    }
    let text = format!("CQ POTA DE {call} K");
    let mut out = Vec::new();
    for word in text.split(' ') {
        for ch in word.chars() {
            for el in code(ch).chars() {
                let n = if el == '-' { 3 } else { 1 };
                out.extend(std::iter::repeat_n(true, n));
                out.push(false);
            }
            out.extend([false, false]); // letter gap (3 total)
        }
        out.extend([false; 4]); // word gap (7 total)
    }
    out.extend([false; 40]); // pause before repeating
    out
}

impl IqSource for SynthSource {
    fn describe(&self) -> String {
        "Demo IF generator".into()
    }

    fn gains_tenths_db(&self) -> Vec<i32> {
        GAINS.to_vec()
    }

    fn read(&mut self, out: &mut Vec<Complex32>) -> Result<ReadStats, String> {
        // Real time: 10 ms blocks, paced by the clock.
        let n = (self.fs / 100.0) as usize;
        let due = self.start + Duration::from_secs_f64(self.produced as f64 / self.fs);
        let now = Instant::now();
        if due > now {
            std::thread::sleep(due - now);
        } else if now - due > Duration::from_millis(500) {
            // Fell far behind (debugger, sleep): skip ahead rather than race.
            self.start = now - Duration::from_secs_f64(self.produced as f64 / self.fs);
        }
        self.produced += n as u64;

        out.clear();
        out.resize(n, Complex32::default());
        let noise = NOISE_AMP * self.gain_scale;
        for s in out.iter_mut() {
            let a = (self.rng.f64() + self.rng.f64() - 1.0) as f32;
            let b = (self.rng.f64() + self.rng.f64() - 1.0) as f32;
            *s = Complex32::new(a, b) * noise;
        }

        let dial = self.rig.freq_hz.load(Ordering::Relaxed) as f64;
        let t = self.wall_time();
        let (fs, tuner, truth, gain) = (self.fs, self.tuner_hz, self.truth, self.gain_scale);
        let baseband = |rf: f64| truth.if_from_rf(dial, rf) - tuner;
        let in_band = |rf: f64| baseband(rf).abs() < fs / 2.0 + 5_000.0;

        for live in &mut self.stations {
            let st = &live.st;
            if !in_band(st.rf_hz) {
                // Out of view: let any sound fade out next time it is visible.
                live.tones.iter_mut().for_each(|tone| tone.amp = 0.0);
                continue;
            }
            let level = 10f32.powf(st.level_db / 20.0) * gain;
            match st.kind {
                DemoKind::Voice { usb } => {
                    let (amp, formants) = voice(st.seed, t);
                    for (k, tone) in live.tones.iter_mut().enumerate() {
                        let audio = if usb { formants[k] } else { -formants[k] };
                        let target = amp * FORMANT_LEVELS[k] * level;
                        add_tone(out, baseband(st.rf_hz + audio), fs, &mut tone.phase, tone.amp, target);
                        tone.amp = target;
                    }
                }
                DemoKind::Cw { wpm } => {
                    let dit = 1.2 / wpm;
                    let idx = ((t + (st.seed % 97) as f64) / dit) as usize % live.pattern.len();
                    let target = if live.pattern[idx] { level } else { 0.0 };
                    let tone = &mut live.tones[0];
                    add_tone(out, baseband(st.rf_hz), fs, &mut tone.phase, tone.amp, target);
                    tone.amp = target;
                }
                DemoKind::Ft8 { odd } => {
                    let period = (t / 15.0) as u64;
                    let within = t % 15.0;
                    let active = (period % 2 == 1) == odd
                        && (0.5..13.1).contains(&within)
                        && demo::hash(st.seed, period) % 100 < 85;
                    let symbol = ((within - 0.5) / 0.16) as u64;
                    let tone_n = demo::hash(st.seed ^ period, symbol) % 8;
                    let target = if active { level } else { 0.0 };
                    let tone = &mut live.tones[0];
                    let f = st.rf_hz + tone_n as f64 * 6.25;
                    add_tone(out, baseband(f), fs, &mut tone.phase, tone.amp, target);
                    tone.amp = target;
                }
                DemoKind::Carrier => {
                    let tone = &mut live.tones[0];
                    add_tone(out, baseband(st.rf_hz), fs, &mut tone.phase, level, level);
                    tone.amp = level;
                }
            }
        }

        // Our own transmission leaks onto the IF tap, very loudly.
        let tx = self.rig.transmitting.load(Ordering::Relaxed);
        let usb = self.rig.mode() != crate::rig::protocol::Mode::Lsb;
        let (amp, formants) = voice(0xC0FFEE, t);
        for (k, tone) in self.tx_tones.iter_mut().enumerate() {
            let audio = if usb { formants[k] } else { -formants[k] };
            let target = if tx { (0.3 + 0.5 * amp) * FORMANT_LEVELS[k] * gain } else { 0.0 };
            add_tone(out, baseband(dial + audio), fs, &mut tone.phase, tone.amp, target);
            tone.amp = target;
        }

        // An 8-bit ADC clips at full scale.
        let mut clipped = 0;
        for s in out.iter_mut() {
            if s.re.abs() >= 1.0 || s.im.abs() >= 1.0 {
                clipped += 1;
                s.re = s.re.clamp(-1.0, 1.0);
                s.im = s.im.clamp(-1.0, 1.0);
            }
        }
        Ok(ReadStats { clipped })
    }

    fn tune(&mut self, hz: u32) -> Result<(), String> {
        self.tuner_hz = f64::from(hz);
        Ok(())
    }

    fn set_gain(&mut self, tenths_db: Option<i32>) -> Result<(), String> {
        // "Auto" behaves like a middling fixed gain.
        let g = tenths_db.unwrap_or(REFERENCE_GAIN + 80);
        self.gain_scale = 10f32.powf((g - REFERENCE_GAIN) as f32 / 200.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::spectrum::SpectrumChain;

    #[test]
    fn carrier_shows_at_its_frequency_through_the_real_chain() {
        // Tune the simulated rig near the 10 MHz carrier.
        let rig = Arc::new(SimShared::new(9_999_000));
        let settings = SdrSettings { sample_rate: 960_000, zoom: 4, ..SdrSettings::default() };
        let tuner = settings.tuner_hz();
        let mut src = SynthSource::new(&settings, tuner, rig);
        let mut chain = SpectrumChain::new(settings.chain_config(tuner), 1);
        let mut frames = Vec::new();
        let mut block = Vec::new();
        for _ in 0..40 {
            src.read(&mut block).unwrap();
            chain.process(std::mem::take(&mut block), &mut frames);
        }
        let f = frames.last().expect("frames");
        let (i, _) = f.db.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).unwrap();
        let offset = f.column_hz(i);
        assert!((offset - 1000.0).abs() <= f.col_hz, "carrier at {offset} Hz from dial");
    }

    #[test]
    fn cq_pattern_has_marks_and_spaces() {
        let p = cq_pattern("K4PARK");
        assert!(p.iter().filter(|&&on| on).count() > 30);
        assert!(!p[p.len() - 1]);
    }
}
