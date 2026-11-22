//! The Seventy3 window.

pub mod palette;
pub mod panadapter;
pub mod settings;
pub mod spots;
pub mod theme;
pub mod vfo;
pub mod waterfall;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Align, Color32, CornerRadius, Key, Layout, Margin, RichText, Sense, Stroke, StrokeKind, Ui, vec2};

use crate::bands::{BANDS, Band, band_for};
use crate::config::{BandMemory, Palette, Settings};
use crate::demo;
use crate::dsp::spectrum::{MAX_ZOOM, SpectrumFrame};
use crate::pota::{SpotFeed, now_unix};
use crate::rig::protocol::Mode;
use crate::rig::serial::SerialTransport;
use crate::rig::sim::{SimRig, SimShared};
use crate::rig::{Notify, Opener, RigHandle, RigOptions, RigState, Transport};
use crate::sdr::rtl::RtlSource;
use crate::sdr::synth::SynthSource;
use crate::sdr::{IqSource, SdrHandle, SourceFactory};
use panadapter::{PanAction, PanInput, PanState, Trace, TuneTracker};
use settings::SettingsUi;
use spots::SpotsState;
use vfo::{VfoAction, VfoState};
use waterfall::Waterfall;

/// Options from the command line.
#[derive(Debug, Clone, Default)]
pub struct LaunchOptions {
    /// Start in demo mode regardless of the saved setting.
    pub demo: bool,
    /// Save a screenshot here after `screenshot_after`, then quit.
    pub screenshot: Option<PathBuf>,
    pub screenshot_after: Duration,
    /// Initial dial frequency for the demo rig.
    pub demo_freq_hz: Option<u64>,
    pub zoom: Option<u32>,
}

struct Calibration {
    known_khz: String,
}

pub struct App {
    settings: Settings,
    saved: Settings,
    last_save: Instant,
    /// Settings the running rig/receiver/feed were built from.
    built: Settings,
    /// A CAT settings change waiting to settle: (new values, first seen).
    rig_change: Option<((crate::rig::serial::SerialSettings, u64, u32), Instant)>,
    sim: Option<Arc<SimShared>>,
    rig: RigHandle,
    sdr: SdrHandle,
    spots: SpotFeed,
    notify: Notify,

    waterfall: Waterfall,
    trace: Trace,
    trace_dial: u64,
    line_acc: Vec<f32>,
    line_count: u32,
    floor_est: Option<f32>,
    colors_applied: (Palette, f32, f32),
    /// (start of this second, frames so far, frames in the last second)
    frames_this_sec: (Instant, u32, u32),

    pan: PanState,
    vfo: VfoState,
    spots_ui: SpotsState,
    settings_ui: SettingsUi,
    tune: TuneTracker,
    keyboard_ptt: bool,
    calibration: Option<Calibration>,
    toasts: Vec<(String, Instant, Color32)>,

    launch: LaunchOptions,
    started: Instant,
    screenshot_requested: bool,
    screenshot: Arc<Mutex<Option<Arc<egui::ColorImage>>>>,
}

fn rig_options(s: &Settings) -> RigOptions {
    RigOptions {
        poll_interval: Duration::from_millis(s.poll_ms.clamp(20, 2000)),
        tx_timeout: (s.tx_timeout_secs > 0).then(|| Duration::from_secs(u64::from(s.tx_timeout_secs))),
    }
}

fn build_rig(s: &Settings, sim: Option<&Arc<SimShared>>, notify: &Notify) -> RigHandle {
    let opener: Opener = match sim {
        Some(sim) => {
            let sim = sim.clone();
            Box::new(move || Ok(Box::new(SimRig::new(sim.clone())) as Box<dyn Transport>))
        }
        None => {
            let serial = s.serial.clone();
            Box::new(move || SerialTransport::open(&serial).map(|t| Box::new(t) as Box<dyn Transport>))
        }
    };
    RigHandle::spawn(opener, rig_options(s), notify.clone())
}

fn build_sdr(s: &Settings, sim: Option<&Arc<SimShared>>, notify: &Notify) -> SdrHandle {
    let factory: SourceFactory = match sim {
        Some(sim) => {
            let sim = sim.clone();
            Box::new(move |st, tuner| Ok(Box::new(SynthSource::new(st, tuner, sim.clone())) as Box<dyn IqSource>))
        }
        None => Box::new(|st, tuner| RtlSource::open(st, tuner).map(|s| Box::new(s) as Box<dyn IqSource>)),
    };
    SdrHandle::spawn(factory, s.sdr.clone(), notify.clone())
}

fn build_spots(s: &Settings, notify: &Notify) -> SpotFeed {
    if s.demo {
        SpotFeed::fixed(demo::spots(now_unix()))
    } else {
        SpotFeed::live(Duration::from_secs(s.pota.refresh_secs.max(30)), notify.clone())
    }
}

/// The value at fraction `q` of the sorted values (q = 0.2: 20th percentile).
fn percentile(values: &[f32], q: f32) -> f32 {
    let mut v: Vec<f32> = values.iter().copied().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return -120.0;
    }
    let k = ((v.len() - 1) as f32 * q) as usize;
    let (_, x, _) = v.select_nth_unstable_by(k, f32::total_cmp);
    *x
}

fn span_text(hz: f64) -> String {
    if hz >= 1e6 {
        format!("{:.2} MHz", hz / 1e6)
    } else if hz >= 10e3 {
        format!("{:.0} kHz", hz / 1e3)
    } else {
        format!("{:.1} kHz", hz / 1e3)
    }
}

pub fn save_png(path: &std::path::Path, image: &egui::ColorImage) -> std::io::Result<()> {
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), image.size[0] as u32, image.size[1] as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(std::io::Error::other)?;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
    writer.write_image_data(&bytes).map_err(std::io::Error::other)
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, mut settings: Settings, launch: LaunchOptions) -> App {
        theme::apply(&cc.egui_ctx);
        if launch.demo {
            settings.demo = true;
            settings.welcomed = true;
        }
        if let Some(z) = launch.zoom {
            settings.sdr.zoom = z.min(MAX_ZOOM);
        }
        let ctx = cc.egui_ctx.clone();
        let notify: Notify = Arc::new(move || ctx.request_repaint());
        let sim = settings.demo.then(|| Arc::new(SimShared::new(launch.demo_freq_hz.unwrap_or(14_285_000))));
        let rig = build_rig(&settings, sim.as_ref(), &notify);
        let sdr = build_sdr(&settings, sim.as_ref(), &notify);
        let spots = build_spots(&settings, &notify);
        App {
            saved: settings.clone(),
            built: settings.clone(),
            rig_change: None,
            settings,
            last_save: Instant::now(),
            sim,
            rig,
            sdr,
            spots,
            notify,
            waterfall: Waterfall::default(),
            trace: Trace::default(),
            trace_dial: 0,
            line_acc: Vec::new(),
            line_count: 0,
            floor_est: None,
            colors_applied: (Palette::Classic, f32::NAN, f32::NAN),
            frames_this_sec: (Instant::now(), 0, 0),
            pan: PanState::default(),
            vfo: VfoState::default(),
            spots_ui: SpotsState::default(),
            settings_ui: SettingsUi::default(),
            tune: TuneTracker::default(),
            keyboard_ptt: false,
            calibration: None,
            toasts: Vec::new(),
            launch,
            started: Instant::now(),
            screenshot_requested: false,
            screenshot: Arc::new(Mutex::new(None)),
        }
    }

    fn toast(&mut self, text: impl Into<String>, color: Color32) {
        self.toasts.push((text.into(), Instant::now(), color));
    }

    // ---- settings changes ----------------------------------------------------

    /// Rebuilds or reconfigures whatever the settings changes affect.
    ///
    /// Receiver changes apply at once (zoom must feel instant). Changes that
    /// reopen the CAT port wait until they have been stable for a moment, so
    /// dragging a slider doesn't reopen the port on every frame.
    fn apply_settings(&mut self) {
        let s = &self.settings;
        let b = &self.built;
        let demo_changed = s.demo != b.demo;
        if demo_changed {
            self.sim = s.demo.then(|| Arc::new(SimShared::new(14_285_000)));
        }
        let rig_key = |x: &Settings| (x.serial.clone(), x.poll_ms, x.tx_timeout_secs);
        let rig_changed = rig_key(s) != rig_key(b);
        let rig_settled = match &self.rig_change {
            Some((key, since)) if *key == rig_key(s) => since.elapsed() >= Duration::from_millis(600),
            _ => {
                self.rig_change = rig_changed.then(|| (rig_key(s), Instant::now()));
                false
            }
        };
        if demo_changed || (rig_changed && rig_settled) {
            // Dropping the old handle unkeys the rig first.
            self.keyboard_ptt = false;
            self.rig = build_rig(s, self.sim.as_ref(), &self.notify);
            self.rig_change = None;
            self.built.serial = s.serial.clone();
            self.built.poll_ms = s.poll_ms;
            self.built.tx_timeout_secs = s.tx_timeout_secs;
        }
        if demo_changed {
            self.sdr = build_sdr(s, self.sim.as_ref(), &self.notify);
        } else if s.sdr != self.built.sdr {
            self.sdr.configure(s.sdr.clone());
        }
        if demo_changed || s.pota.refresh_secs != self.built.pota.refresh_secs {
            self.spots = build_spots(s, &self.notify);
        }
        // Everything except a still-pending CAT change is now applied.
        let (serial, poll_ms, tx_timeout_secs) =
            (self.built.serial.clone(), self.built.poll_ms, self.built.tx_timeout_secs);
        self.built = self.settings.clone();
        if self.rig_change.is_some() {
            self.built.serial = serial;
            self.built.poll_ms = poll_ms;
            self.built.tx_timeout_secs = tx_timeout_secs;
        }
    }

    fn maybe_save(&mut self) {
        if self.settings != self.saved && self.last_save.elapsed() > Duration::from_secs(3) {
            if self.launch.screenshot.is_none() {
                self.settings.save();
            }
            self.saved = self.settings.clone();
            self.last_save = Instant::now();
        }
    }

    // ---- tuning ----------------------------------------------------------------

    fn tune_to(&mut self, hz: u64) {
        let hz = hz.clamp(crate::rig::protocol::MIN_FREQ_HZ, crate::rig::protocol::MAX_FREQ_HZ);
        self.rig.set_freq(hz);
        self.tune.record(hz);
    }

    fn tune_by(&mut self, delta: i64, dial: u64) {
        let base = self.tune.base(dial) as i64;
        self.tune_to((base + delta).max(0) as u64);
    }

    fn tune_spot(&mut self, id: u64, rig: &RigState) {
        let feed = self.spots.state();
        let Some(spot) = feed.spots.iter().find(|s| s.id == id) else { return };
        if let Some(mode) = spot.rig_mode()
            && rig.mode != Some(mode)
        {
            self.rig.set_mode(mode);
        }
        let f = spot.freq_hz;
        self.tune_to(f);
    }

    fn select_band(&mut self, band: &Band, rig: &RigState) {
        let memory = self.settings.band_memory.get(band.name).copied();
        let (freq, mode) = match memory {
            Some(m) if band.contains(m.freq_hz) => (m.freq_hz, m.mode),
            _ => (band.default_hz, band.default_mode()),
        };
        if rig.mode != Some(mode) {
            self.rig.set_mode(mode);
        }
        self.tune_to(freq);
    }

    fn zoom(&mut self, steps: i32) {
        let z = (self.settings.sdr.zoom as i32 + steps).clamp(0, MAX_ZOOM as i32) as u32;
        self.settings.sdr.zoom = z;
    }

    fn remember_band(&mut self, rig: &RigState) {
        let (Some(band), Some(mode)) = (band_for(rig.freq_hz), rig.mode) else { return };
        if !rig.connected {
            return;
        }
        let m = BandMemory { freq_hz: rig.freq_hz, mode };
        if self.settings.band_memory.get(band.name) != Some(&m) {
            self.settings.band_memory.insert(band.name.to_owned(), m);
        }
    }

    // ---- spectrum frames ---------------------------------------------------------

    fn levels(&self) -> (f32, f32) {
        let d = &self.settings.display;
        if d.auto_level { (self.floor_est.unwrap_or(d.floor_db) - 4.0, d.range_db) } else { (d.floor_db, d.range_db) }
    }

    fn take_frames(&mut self, ctx: &egui::Context, rig: &RigState) {
        let dial = rig.freq_hz;
        let frames: Vec<SpectrumFrame> = self.sdr.take_frames();
        let paused = rig.transmitting && self.settings.display.pause_on_tx;
        let d = self.settings.display.clone();
        for frame in frames {
            self.frames_this_sec.1 += 1;
            if self.waterfall.generation != frame.generation || self.waterfall.width() != frame.db.len() {
                self.waterfall.reset(frame.db.len(), frame.generation, frame.start_hz, frame.col_hz);
                self.trace = Trace {
                    db: frame.db.clone(),
                    peak: frame.db.clone(),
                    start_hz: frame.start_hz,
                    col_hz: frame.col_hz,
                };
                self.trace_dial = dial;
                self.line_acc.clear();
                self.line_count = 0;
            }

            let floor = percentile(&frame.db, 0.2);
            self.floor_est = Some(match self.floor_est {
                Some(f) => f + (floor - f) * 0.05,
                None => floor,
            });

            // The trace: averaged, unless the dial moved (then averaging
            // would smear the old and new spectra together).
            if dial != self.trace_dial {
                self.trace.db.clone_from(&frame.db);
                self.trace.peak.clone_from(&frame.db);
                self.trace_dial = dial;
            } else {
                let a = d.averaging.clamp(0.0, 0.95);
                for (t, &x) in self.trace.db.iter_mut().zip(&frame.db) {
                    *t = a * *t + (1.0 - a) * x;
                }
                for (p, &x) in self.trace.peak.iter_mut().zip(&frame.db) {
                    *p = (*p - 0.15).max(x);
                }
            }

            if !paused {
                if self.line_acc.len() != frame.db.len() {
                    self.line_acc = vec![0.0; frame.db.len()];
                    self.line_count = 0;
                }
                for (acc, &x) in self.line_acc.iter_mut().zip(&frame.db) {
                    *acc += x;
                }
                self.line_count += 1;
                if self.line_count >= d.frames_per_line.max(1) {
                    let n = self.line_count as f32;
                    let line: Vec<f32> = self.line_acc.iter().map(|v| v / n).collect();
                    self.waterfall.push(&line, dial as f64, rig.transmitting);
                    self.line_acc.iter_mut().for_each(|v| *v = 0.0);
                    self.line_count = 0;
                }
            }
        }
        if self.frames_this_sec.0.elapsed() >= Duration::from_secs(1) {
            self.frames_this_sec = (Instant::now(), 0, self.frames_this_sec.1);
        }

        let (floor, range) = self.levels();
        let (p, f, r) = self.colors_applied;
        let tolerance = if d.auto_level { 3.0 } else { 0.01 };
        let floor_moved = f.is_nan() || (floor - f).abs() >= tolerance;
        let recolor = p != d.palette || floor_moved || (range - r).abs() > 0.01;
        self.waterfall.set_colors(d.palette, floor, range, recolor);
        if recolor {
            self.colors_applied = (d.palette, floor, range);
        }
        self.waterfall.upload(ctx);
    }

    // ---- keyboard ----------------------------------------------------------------

    fn keys(&mut self, ctx: &egui::Context, rig: &RigState) {
        let typing = ctx.egui_wants_keyboard_input();
        let (space, esc, left, right, zin, zout, focused, shift) = ctx.input(|i| {
            (
                i.key_down(Key::Space),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::Equals) || i.key_pressed(Key::Plus),
                i.key_pressed(Key::Minus),
                i.focused,
                i.modifiers.shift,
            )
        });
        if esc {
            // Always a way out of transmit.
            if rig.ptt_engaged || self.keyboard_ptt {
                self.rig.ptt(false);
            }
            self.keyboard_ptt = false;
            self.calibration = None;
        }
        if self.settings.space_bar_ptt && !typing && rig.connected {
            if space && !self.keyboard_ptt && !rig.ptt_engaged {
                self.rig.ptt(true);
                self.keyboard_ptt = true;
            } else if !space && self.keyboard_ptt {
                self.rig.ptt(false);
                self.keyboard_ptt = false;
            }
        }
        // A key-up can be missed if the window loses focus mid-press.
        if self.keyboard_ptt && (!focused || typing) {
            self.rig.ptt(false);
            self.keyboard_ptt = false;
        }
        if !typing {
            let step = i64::from(self.settings.tuning.step_hz) * if shift { 10 } else { 1 };
            if left {
                self.tune_by(-step, rig.freq_hz);
            }
            if right {
                self.tune_by(step, rig.freq_hz);
            }
            if zin {
                self.zoom(1);
            }
            if zout {
                self.zoom(-1);
            }
        }
    }

    // ---- panels ------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut Ui, rig: &RigState) {
        let live = rig.connected;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.add_space(4.0);
                ui.label(RichText::new("SEVENTY3").strong().size(17.0).color(theme::ACCENT));
                ui.label(RichText::new("TS-480 panadapter").small().color(theme::TEXT_DIM));
            });
            ui.add_space(10.0);
            if let Some(action) = vfo::show(ui, rig.freq_hz, &mut self.vfo, live) {
                match action {
                    VfoAction::Set(hz) => self.tune_to(hz),
                    VfoAction::Step(d) => self.tune_by(d, rig.freq_hz),
                }
            }
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 5.0);
                ui.horizontal(|ui| {
                    for m in Mode::BUTTONS {
                        let selected = rig.mode == Some(m);
                        let text = RichText::new(m.label()).size(13.0);
                        let text = if selected { text.strong().color(theme::BG) } else { text };
                        let mut b = egui::Button::new(text).min_size(vec2(44.0, 22.0));
                        if selected {
                            b = b.fill(theme::ACCENT);
                        }
                        if ui.add_enabled(live, b).clicked() {
                            self.rig.set_mode(m);
                        }
                    }
                    ui.add_space(6.0);
                    let mut flags = Vec::new();
                    if rig.split {
                        flags.push("SPLIT".to_owned());
                    }
                    if rig.rit_on {
                        flags.push(format!("RIT {:+}", rig.rit_xit_hz));
                    }
                    if rig.xit_on {
                        flags.push(format!("XIT {:+}", rig.rit_xit_hz));
                    }
                    if let Some(w) = rig.power_w {
                        flags.push(format!("{w} W"));
                    }
                    ui.label(RichText::new(flags.join("  ")).small().color(theme::TEXT_DIM));
                });
                ui.horizontal(|ui| {
                    let current = band_for(rig.freq_hz).map(|b| b.name);
                    for band in &BANDS {
                        let selected = current == Some(band.name);
                        let text = RichText::new(band.name).size(12.5);
                        let text =
                            if selected { text.strong().color(theme::ACCENT) } else { text.color(theme::TEXT_DIM) };
                        let mut b = egui::Button::new(text).min_size(vec2(34.0, 20.0));
                        if selected {
                            b = b.stroke(Stroke::new(1.0, theme::ACCENT));
                        }
                        if ui.add_enabled(live, b).on_hover_text(format!("{} m", band.name)).clicked() {
                            self.select_band(band, rig);
                        }
                    }
                });
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.ptt_button(ui, rig);
            });
        });
    }

    fn ptt_button(&mut self, ui: &mut Ui, rig: &RigState) {
        let on_air = rig.transmitting || rig.ptt_engaged;
        let (rect, resp) = ui.allocate_exact_size(vec2(124.0, 50.0), Sense::click());
        let enabled = rig.connected;
        let fill = if on_air {
            theme::TX
        } else if resp.hovered() && enabled {
            theme::RAISED_HOVER
        } else {
            theme::RAISED
        };
        let painter = ui.painter();
        painter.rect(
            rect,
            CornerRadius::same(8),
            fill,
            Stroke::new(1.5, if enabled { theme::TX } else { theme::BORDER }),
            StrokeKind::Inside,
        );
        let (title, sub) = if on_air {
            let secs = rig.ptt_since.map_or(0, |t| t.elapsed().as_secs());
            ("ON AIR", if rig.ptt_engaged { format!("{}:{:02}", secs / 60, secs % 60) } else { "rig PTT".into() })
        } else {
            ("PTT", if self.settings.space_bar_ptt { "hold Space".into() } else { "click".into() })
        };
        let text_color = if on_air {
            Color32::WHITE
        } else if enabled {
            theme::TEXT
        } else {
            theme::TEXT_FAINT
        };
        painter.text(
            rect.center() - vec2(0.0, 7.0),
            egui::Align2::CENTER_CENTER,
            title,
            egui::FontId::proportional(18.0),
            text_color,
        );
        painter.text(
            rect.center() + vec2(0.0, 12.0),
            egui::Align2::CENTER_CENTER,
            sub,
            egui::FontId::proportional(11.0),
            if on_air { Color32::WHITE } else { theme::TEXT_DIM },
        );
        let resp = resp.on_hover_text("Click to transmit, click again to stop. Esc always stops.");
        if resp.clicked() && enabled {
            self.rig.ptt(!rig.ptt_engaged);
            self.keyboard_ptt = false;
        }
        if rig.tx_timed_out {
            ui.label(RichText::new("TX timeout").small().color(theme::WARN))
                .on_hover_text("The transmit timeout unkeyed the rig. Change it in Settings → Rig.");
        }
    }

    fn tool_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let cfg = self.settings.sdr.chain_config(self.settings.sdr.tuner_hz());
            ui.label(RichText::new("Span").color(theme::TEXT_DIM));
            if ui
                .add_enabled(self.settings.sdr.zoom > 0, egui::Button::new("−"))
                .on_hover_text("Zoom out (−)")
                .clicked()
            {
                self.zoom(-1);
            }
            ui.label(RichText::new(span_text(2.0 * cfg.half_span_hz())).monospace().strong());
            if ui
                .add_enabled(self.settings.sdr.zoom < MAX_ZOOM, egui::Button::new("+"))
                .on_hover_text("Zoom in (+)")
                .clicked()
            {
                self.zoom(1);
            }
            ui.label(RichText::new(format!("{:.0} Hz/bin", cfg.bin_hz())).small().color(theme::TEXT_DIM));
            ui.separator();

            let d = &mut self.settings.display;
            ui.toggle_value(&mut d.auto_level, "Auto level").on_hover_text("Follow the noise floor");
            if !d.auto_level {
                ui.add(
                    egui::DragValue::new(&mut d.floor_db)
                        .range(-150.0..=-30.0)
                        .speed(0.5)
                        .prefix("floor ")
                        .suffix(" dB"),
                );
            }
            ui.add(egui::DragValue::new(&mut d.range_db).range(15.0..=90.0).speed(0.3).prefix("range ").suffix(" dB"));
            egui::ComboBox::from_id_salt("tb_palette").selected_text(d.palette.label()).width(90.0).show_ui(ui, |ui| {
                for p in Palette::ALL {
                    ui.selectable_value(&mut d.palette, p, p.label());
                }
            });
            ui.toggle_value(&mut d.peak_hold, "Peak");
            ui.separator();

            let calibrating = self.calibration.is_some();
            if ui
                .selectable_label(calibrating, "Calibrate")
                .on_hover_text("Line the display up with a carrier on a known frequency")
                .clicked()
            {
                self.calibration = if calibrating {
                    None
                } else {
                    let dial = self.rig.state().freq_hz;
                    Some(Calibration { known_khz: format!("{:.3}", dial as f64 / 1000.0) })
                };
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Settings").clicked() {
                    self.settings_ui.open = !self.settings_ui.open;
                }
                ui.toggle_value(&mut self.settings.spots_panel, "Spots");
                if self.settings.demo {
                    ui.label(RichText::new(" DEMO ").strong().color(theme::BG).background_color(theme::WARN));
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut Ui, rig: &RigState) {
        let sdr = self.sdr.status();
        ui.horizontal(|ui| {
            let (color, text) = if rig.connected {
                let name = if self.settings.demo {
                    "Simulated TS-480".to_owned()
                } else {
                    let model = match rig.rig_id {
                        Some(crate::rig::protocol::TS480_ID) | None => "TS-480".to_owned(),
                        Some(id) => format!("Rig ID{id:03}"),
                    };
                    format!("{model} on {} at {} baud", self.settings.serial.port, self.settings.serial.baud)
                };
                (theme::OK, name)
            } else {
                let msg = rig.error.clone().unwrap_or_else(|| "Connecting to the rig…".into());
                (if rig.error.is_some() { theme::TX } else { theme::WARN }, msg)
            };
            vfo::status_dot(ui, color, RichText::new(text).small());
            ui.separator();
            let (color, text) = if sdr.running {
                let gain = match self.settings.sdr.gain_tenths_db {
                    Some(g) => format!("{:.1} dB", g as f32 / 10.0),
                    None => "auto gain".into(),
                };
                (
                    theme::OK,
                    format!(
                        "{} · {:.3} MS/s · {gain}",
                        sdr.description,
                        f64::from(self.settings.sdr.sample_rate) / 1e6
                    ),
                )
            } else {
                (
                    if sdr.error.is_some() { theme::TX } else { theme::WARN },
                    sdr.error.clone().unwrap_or_else(|| "Starting the IF receiver…".into()),
                )
            };
            vfo::status_dot(ui, color, RichText::new(text).small());
            if sdr.overload {
                ui.label(RichText::new("ADC overload: lower the RF gain").small().strong().color(theme::TX));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).small().color(theme::TEXT_FAINT));
                ui.label(RichText::new(format!("{} fps", self.frames_this_sec.2)).small().color(theme::TEXT_DIM));
            });
        });
    }

    fn calibration_window(&mut self, ctx: &egui::Context) {
        let Some(cal) = &mut self.calibration else { return };
        let mut open = true;
        egui::Window::new("Calibrate")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, vec2(0.0, 150.0))
            .show(ctx, |ui| {
                ui.set_max_width(360.0);
                ui.label(
                    "1. Tune the rig to a station on a known frequency with a steady carrier, \
                     such as WWV or CHU in AM mode.",
                );
                ui.label("2. Zoom in, then click the centre of the carrier on the waterfall.");
                ui.horizontal(|ui| {
                    ui.label("Its true frequency:");
                    ui.add(egui::TextEdit::singleline(&mut cal.known_khz).desired_width(100.0));
                    ui.label("kHz");
                });
            });
        if !open {
            self.calibration = None;
        }
    }

    fn welcome_window(&mut self, ctx: &egui::Context) {
        if self.settings.welcomed {
            return;
        }
        egui::Window::new("Welcome to Seventy3")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(
                    "Seventy3 shows the TS-480's first IF (73.095 MHz) as a panadapter and \
                     waterfall using an RTL-SDR, and controls the rig through its COM port.",
                );
                ui.add_space(6.0);
                ui.label("To start, plug in the RTL-SDR and the CAT cable, then choose the CAT port.");
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Set up my TS-480").strong()).clicked() {
                        self.settings.welcomed = true;
                        self.settings_ui.open = true;
                        self.settings_ui.tab = settings::Tab::Rig;
                    }
                    if ui.button("Try the demo").clicked() {
                        self.settings.welcomed = true;
                        self.settings.demo = true;
                    }
                });
            });
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.launch.screenshot.clone() else { return };
        if !self.screenshot_requested && self.started.elapsed() >= self.launch.screenshot_after {
            let slot = self.screenshot.clone();
            ctx.request_screenshot(move |image| {
                if let Ok(mut s) = slot.lock() {
                    *s = Some(image);
                }
            });
            self.screenshot_requested = true;
        }
        let image = self.screenshot.lock().ok().and_then(|mut s| s.take());
        if let Some(image) = image {
            match save_png(&path, &image) {
                Ok(()) => log::info!("Saved screenshot to {}", path.display()),
                Err(e) => log::error!("Screenshot: {e}"),
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn handle_pan_action(&mut self, action: PanAction, rig: &RigState) {
        match action {
            PanAction::Tune(hz) => self.tune_to(hz),
            PanAction::TuneBy(d) => self.tune_by(d, rig.freq_hz),
            PanAction::TuneSpot(id) => self.tune_spot(id, rig),
            PanAction::Zoom(steps) => self.zoom(steps),
            PanAction::Calibrate(displayed) => {
                let known = self.calibration.as_ref().and_then(|c| vfo::parse_freq_input(&c.known_khz));
                match known {
                    Some(truth) => {
                        let old = self.settings.sdr.if_map;
                        let new = old.calibrated(displayed, truth as f64);
                        self.settings.sdr.if_map = new;
                        self.calibration = None;
                        self.toast(
                            format!("Calibrated: IF centre moved {:+.0} Hz", new.if_center_hz - old.if_center_hz),
                            theme::OK,
                        );
                    }
                    None => self.toast("Enter the reference frequency first", theme::WARN),
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let rig = self.rig.state();
        self.take_frames(&ctx, &rig);
        self.keys(&ctx, &rig);
        self.remember_band(&rig);

        egui::Panel::top("top")
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 8)))
            .show(ui, |ui| self.top_bar(ui, &rig));
        egui::Panel::top("tools")
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(Margin::symmetric(12, 5))
                    .stroke(Stroke::new(1.0, theme::BORDER)),
            )
            .show(ui, |ui| self.tool_bar(ui));
        egui::Panel::bottom("status")
            .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(12, 4)))
            .show(ui, |ui| self.status_bar(ui, &rig));

        let feed = self.spots.state();
        if self.settings.spots_panel {
            let mut clicked = None;
            let spots_feed = &self.spots;
            let pota = &mut self.settings.pota;
            let spots_ui = &mut self.spots_ui;
            egui::Panel::right("spots")
                .resizable(true)
                .default_size(400.0)
                .frame(egui::Frame::new().fill(theme::PANEL).inner_margin(Margin::symmetric(10, 8)))
                .show(ui, |ui| {
                    clicked = spots::show(ui, &feed, pota, spots_ui, rig.freq_hz, || spots_feed.refresh_now());
                });
            if let Some(id) = clicked {
                self.tune_spot(id, &rig);
            }
        }

        let marker_spots: Vec<_> = if self.settings.pota.show_markers {
            spots::filtered(&feed.spots, &self.settings.pota, rig.freq_hz).into_iter().cloned().collect()
        } else {
            Vec::new()
        };
        let cfg = self.settings.sdr.chain_config(self.settings.sdr.tuner_hz());
        let (floor, range) = self.levels();
        let sdr_status = self.sdr.status();
        let placeholder = (self.waterfall.width() == 0)
            .then(|| sdr_status.error.clone().unwrap_or_else(|| "Starting the IF receiver…".into()));
        let input = PanInput {
            trace: &self.trace,
            show_peak: self.settings.display.peak_hold,
            waterfall: &self.waterfall,
            dial_hz: rig.freq_hz,
            mode: rig.mode,
            passband: rig.passband(),
            transmitting: rig.transmitting,
            half_span_hz: cfg.half_span_hz(),
            floor_db: floor,
            range_db: range,
            spectrum_fraction: self.settings.display.spectrum_fraction,
            spots: &marker_spots,
            calibrating: self.calibration.is_some(),
            tuning: &self.settings.tuning,
            placeholder,
        };
        let mut actions = Vec::new();
        let pan = &mut self.pan;
        egui::CentralPanel::no_frame().show(ui, |ui| {
            actions = panadapter::show(ui, &input, pan);
        });
        for action in actions {
            self.handle_pan_action(action, &rig);
        }

        settings::show(&ctx, &mut self.settings_ui, &mut self.settings, &sdr_status.gains_tenths_db);
        self.calibration_window(&ctx);
        self.welcome_window(&ctx);

        // Toasts, bottom right.
        self.toasts.retain(|t| t.1.elapsed() < Duration::from_secs(5));
        for (i, (text, _, color)) in self.toasts.iter().enumerate() {
            egui::Area::new(egui::Id::unique(("seventy3-toast", i)))
                .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-16.0, -40.0 - 40.0 * i as f32))
                .show(&ctx, |ui| {
                    egui::Frame::new()
                        .fill(theme::RAISED)
                        .stroke(Stroke::new(1.0, *color))
                        .corner_radius(6)
                        .inner_margin(Margin::symmetric(12, 8))
                        .show(ui, |ui| ui.label(RichText::new(text).color(*color)));
                });
        }

        if self.settings != self.built {
            self.apply_settings();
        }
        self.maybe_save();
        self.handle_screenshot(&ctx);
        // Clocks (TX timer, spot ages) tick even without new data.
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if self.settings != self.saved && self.launch.screenshot.is_none() {
            self.settings.save();
        }
    }
}
