//! The IF receiver: a background thread that reads samples from the RTL-SDR
//! (or the demo generator), runs the spectrum chain and hands finished
//! spectrum frames to the UI.

pub mod rtl;
pub mod synth;

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::dsp::Complex32;
use crate::dsp::ifmap::IfMap;
use crate::dsp::spectrum::{ChainConfig, SpectrumChain, SpectrumFrame};
use crate::rig::Notify;

/// RTL-SDR sample rates offered in the settings.
pub const SAMPLE_RATES: [u32; 6] = [960_000, 1_024_000, 1_536_000, 1_920_000, 2_048_000, 2_400_000];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SdrSettings {
    /// Which RTL-SDR, if more than one is plugged in.
    pub device_index: usize,
    pub sample_rate: u32,
    /// Tuner gain in tenths of a dB; `None` lets the tuner's AGC decide.
    pub gain_tenths_db: Option<i32>,
    pub if_map: IfMap,
    /// The RTL-SDR is tuned this far above the IF centre so its DC spike
    /// lands away from the dial frequency.
    pub tune_offset_hz: f64,
    /// 0 shows the whole band the RTL-SDR captures; each step halves the span.
    pub zoom: u32,
    pub frames_per_sec: f32,
}

impl Default for SdrSettings {
    fn default() -> Self {
        SdrSettings {
            device_index: 0,
            sample_rate: 2_400_000,
            gain_tenths_db: Some(207),
            if_map: IfMap::default(),
            tune_offset_hz: 150_000.0,
            zoom: 3,
            frames_per_sec: 30.0,
        }
    }
}

impl SdrSettings {
    /// Frequency the RTL-SDR is tuned to. Rounded to 10 kHz so small
    /// calibration changes are absorbed in software rather than by retuning.
    pub fn tuner_hz(&self) -> u32 {
        let target = self.if_map.if_center_hz + self.tune_offset_hz;
        ((target / 10_000.0).round() * 10_000.0) as u32
    }

    pub fn chain_config(&self, tuner_hz: u32) -> ChainConfig {
        ChainConfig {
            sample_rate: f64::from(self.sample_rate),
            dial_offset_hz: self.if_map.if_center_hz - f64::from(tuner_hz),
            inverted: self.if_map.inverted,
            zoom: self.zoom,
            frames_per_sec: self.frames_per_sec,
            max_columns: 4096,
        }
    }
}

/// What the UI shows about the receiver.
#[derive(Debug, Clone, Default)]
pub struct SdrStatus {
    pub running: bool,
    /// E.g. "RTL-SDR (R820T)".
    pub description: String,
    pub error: Option<String>,
    /// Gains the tuner supports, tenths of a dB.
    pub gains_tenths_db: Vec<i32>,
    /// ADC clipping seen in the last second: turn the gain down.
    pub overload: bool,
    /// Frames the UI was too slow to take.
    pub dropped_frames: u64,
}

/// Samples read in one go, and how many of them were clipped.
pub struct ReadStats {
    pub clipped: usize,
}

/// A source of IF samples.
pub trait IqSource: Send {
    fn describe(&self) -> String;
    fn gains_tenths_db(&self) -> Vec<i32>;
    /// Replaces `out` with the next block of samples. Blocks briefly.
    fn read(&mut self, out: &mut Vec<Complex32>) -> Result<ReadStats, String>;
    fn tune(&mut self, hz: u32) -> Result<(), String>;
    fn set_gain(&mut self, tenths_db: Option<i32>) -> Result<(), String>;
}

/// Opens a source for the given settings, tuned to the given frequency.
pub type SourceFactory = Box<dyn FnMut(&SdrSettings, u32) -> Result<Box<dyn IqSource>, String> + Send>;

pub enum SdrCommand {
    Configure(SdrSettings),
    Shutdown,
}

pub struct SdrHandle {
    tx: Sender<SdrCommand>,
    frames: Receiver<SpectrumFrame>,
    status: Arc<Mutex<SdrStatus>>,
    done: Receiver<()>,
}

impl SdrHandle {
    pub fn spawn(factory: SourceFactory, settings: SdrSettings, notify: Notify) -> SdrHandle {
        let (tx, rx) = mpsc::channel();
        let (frame_tx, frames) = mpsc::sync_channel(32);
        let (done_tx, done) = mpsc::channel();
        let status = Arc::new(Mutex::new(SdrStatus::default()));
        let thread_status = status.clone();
        std::thread::Builder::new()
            .name("sdr".into())
            .spawn(move || {
                run(factory, rx, frame_tx, thread_status, notify, settings);
                let _ = done_tx.send(());
            })
            .expect("spawn sdr thread");
        SdrHandle { tx, frames, status, done }
    }

    pub fn configure(&self, settings: SdrSettings) {
        let _ = self.tx.send(SdrCommand::Configure(settings));
    }

    pub fn status(&self) -> SdrStatus {
        lock(&self.status).clone()
    }

    /// Frames produced since the last call.
    pub fn take_frames(&self) -> Vec<SpectrumFrame> {
        self.frames.try_iter().collect()
    }
}

impl Drop for SdrHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(SdrCommand::Shutdown);
        // A wedged USB read must not hang the app on exit; give the thread a
        // moment, then let it go.
        let _ = self.done.recv_timeout(Duration::from_secs(1));
    }
}

fn lock(s: &Mutex<SdrStatus>) -> MutexGuard<'_, SdrStatus> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

fn run(
    mut factory: SourceFactory,
    rx: Receiver<SdrCommand>,
    frames: SyncSender<SpectrumFrame>,
    status: Arc<Mutex<SdrStatus>>,
    notify: Notify,
    mut settings: SdrSettings,
) {
    let mut generation = 0u64;
    'open: loop {
        let mut tuner_hz = settings.tuner_hz();
        let mut source = match factory(&settings, tuner_hz) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("SDR: {e}");
                {
                    let mut st = lock(&status);
                    st.running = false;
                    st.error = Some(e);
                }
                notify();
                // Retry later, applying settings changes meanwhile.
                match rx.recv_timeout(Duration::from_secs(2)) {
                    Ok(SdrCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                    Ok(SdrCommand::Configure(s)) => settings = s,
                    Err(RecvTimeoutError::Timeout) => {}
                }
                continue 'open;
            }
        };
        log::info!("SDR: {} tuned to {tuner_hz} Hz", source.describe());
        {
            let mut st = lock(&status);
            *st = SdrStatus {
                running: true,
                description: source.describe(),
                gains_tenths_db: source.gains_tenths_db(),
                dropped_frames: st.dropped_frames,
                ..SdrStatus::default()
            };
        }
        notify();

        generation += 1;
        let mut chain = SpectrumChain::new(settings.chain_config(tuner_hz), generation);
        let mut block = Vec::new();
        let mut out = Vec::new();
        let mut clip_window = (Instant::now(), 0usize);

        loop {
            loop {
                match rx.try_recv() {
                    Ok(SdrCommand::Shutdown) | Err(TryRecvError::Disconnected) => return,
                    Err(TryRecvError::Empty) => break,
                    Ok(SdrCommand::Configure(new)) => {
                        if new.sample_rate != settings.sample_rate || new.device_index != settings.device_index {
                            settings = new;
                            continue 'open;
                        }
                        let mut result = Ok(());
                        if new.tuner_hz() != tuner_hz {
                            tuner_hz = new.tuner_hz();
                            result = source.tune(tuner_hz);
                        }
                        if result.is_ok() && new.gain_tenths_db != settings.gain_tenths_db {
                            result = source.set_gain(new.gain_tenths_db);
                        }
                        if let Err(e) = result {
                            lock(&status).error = Some(e);
                        }
                        settings = new;
                        let cfg = settings.chain_config(tuner_hz);
                        if &cfg != chain.config() {
                            generation += 1;
                            chain = SpectrumChain::new(cfg, generation);
                        }
                    }
                }
            }

            let stats = match source.read(&mut block) {
                Ok(stats) => stats,
                Err(e) => {
                    log::warn!("SDR read failed: {e}");
                    {
                        let mut st = lock(&status);
                        st.running = false;
                        st.error = Some(e);
                    }
                    notify();
                    std::thread::sleep(Duration::from_millis(500));
                    continue 'open;
                }
            };
            clip_window.1 += stats.clipped;
            if clip_window.0.elapsed() >= Duration::from_secs(1) {
                let overload = clip_window.1 > 0;
                let mut st = lock(&status);
                if st.overload != overload {
                    st.overload = overload;
                }
                clip_window = (Instant::now(), 0);
            }

            chain.process(std::mem::take(&mut block), &mut out);
            if !out.is_empty() {
                let mut dropped = 0;
                for frame in out.drain(..) {
                    if frames.try_send(frame).is_err() {
                        dropped += 1;
                    }
                }
                if dropped > 0 {
                    lock(&status).dropped_frames += dropped;
                }
                notify();
            }
        }
    }
}
