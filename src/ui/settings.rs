//! The settings window.

use egui::{RichText, Ui};

use super::theme;
use crate::config::{Palette, Settings};
use crate::dsp::ifmap::TS480_FIRST_IF_HZ;
use crate::rig::serial::{BAUD_RATES, PortInfo, list_ports};
use crate::sdr::SAMPLE_RATES;
use crate::update::{RELEASES_PAGE, UpdateState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Rig,
    Receiver,
    Display,
    Tuning,
    About,
}

#[derive(Default)]
pub struct SettingsUi {
    pub open: bool,
    pub tab: Tab,
    ports: Option<Vec<PortInfo>>,
}

/// What the About tab shows about updates, and what the user asked for.
pub struct UpdatesView<'a> {
    pub state: &'a UpdateState,
    /// Seconds since the last check, if there was one this session.
    pub last_check_secs: Option<u64>,
    pub check_now: bool,
    pub install: bool,
}

/// Shows the window. Edits `settings` in place; the caller notices changes.
pub fn show(
    ctx: &egui::Context,
    ui_state: &mut SettingsUi,
    settings: &mut Settings,
    gains: &[i32],
    updates: &mut UpdatesView<'_>,
) {
    let mut open = ui_state.open;
    egui::Window::new("Settings").open(&mut open).resizable(false).collapsible(false).default_width(460.0).show(
        ctx,
        |ui| {
            ui.horizontal(|ui| {
                for (tab, name) in [
                    (Tab::Rig, "Rig (CAT)"),
                    (Tab::Receiver, "IF receiver"),
                    (Tab::Display, "Display"),
                    (Tab::Tuning, "Tuning"),
                    (Tab::About, "About"),
                ] {
                    ui.selectable_value(&mut ui_state.tab, tab, name);
                }
            });
            ui.separator();
            match ui_state.tab {
                Tab::Rig => rig_tab(ui, ui_state, settings),
                Tab::Receiver => receiver_tab(ui, settings, gains),
                Tab::Display => display_tab(ui, settings),
                Tab::Tuning => tuning_tab(ui, settings),
                Tab::About => about_tab(ui, settings, updates),
            }
        },
    );
    ui_state.open = open;
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(theme::TEXT_DIM));
}

fn rig_tab(ui: &mut Ui, st: &mut SettingsUi, s: &mut Settings) {
    ui.checkbox(&mut s.demo, "Demo mode (simulated TS-480 and IF, no hardware)");
    ui.add_space(6.0);
    ui.add_enabled_ui(!s.demo, |ui| {
        let ports = st.ports.get_or_insert_with(list_ports);
        egui::Grid::new("rig").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("CAT port");
            ui.horizontal(|ui| {
                let current = if s.serial.port.is_empty() { "Choose…".to_owned() } else { s.serial.port.clone() };
                egui::ComboBox::from_id_salt("port").selected_text(current).width(220.0).show_ui(ui, |ui| {
                    if ports.is_empty() {
                        ui.label("No serial ports found");
                    }
                    for p in ports.iter() {
                        let label = if p.description.is_empty() {
                            p.name.clone()
                        } else {
                            format!("{}  —  {}", p.name, p.description)
                        };
                        ui.selectable_value(&mut s.serial.port, p.name.clone(), label);
                    }
                });
                if ui.button("⟳").on_hover_text("Look for ports again").clicked() {
                    *ports = list_ports();
                }
            });
            ui.end_row();

            ui.label("Baud rate");
            egui::ComboBox::from_id_salt("baud").selected_text(s.serial.baud.to_string()).show_ui(ui, |ui| {
                for b in BAUD_RATES {
                    ui.selectable_value(&mut s.serial.baud, b, b.to_string());
                }
            });
            ui.end_row();

            ui.label("Stop bits");
            ui.horizontal(|ui| {
                ui.radio_value(&mut s.serial.two_stop_bits, false, "1");
                ui.radio_value(&mut s.serial.two_stop_bits, true, "2");
            });
            ui.end_row();

            ui.label("Handshake");
            ui.horizontal(|ui| {
                ui.radio_value(&mut s.serial.hardware_flow, false, "None");
                ui.radio_value(&mut s.serial.hardware_flow, true, "RTS/CTS");
            });
            ui.end_row();

            ui.label("Control lines");
            ui.horizontal(|ui| {
                ui.add_enabled(!s.serial.hardware_flow, egui::Checkbox::new(&mut s.serial.rts, "RTS on"));
                ui.checkbox(&mut s.serial.dtr, "DTR on");
            });
            ui.end_row();

            ui.label("Poll every");
            ui.add(egui::Slider::new(&mut s.poll_ms, 50..=500).suffix(" ms"));
            ui.end_row();
        });
        hint(
            ui,
            "Match the TS-480's menu 56 (COM port speed). If the rig doesn't answer, try RTS on/off; \
             if it keys up when Seventy3 starts, turn DTR and RTS off.",
        );
    });

    ui.add_space(8.0);
    ui.label(RichText::new("Transmit").strong());
    egui::Grid::new("tx").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Transmit timeout");
        ui.horizontal(|ui| {
            let mut on = s.tx_timeout_secs > 0;
            if ui.checkbox(&mut on, "").changed() {
                s.tx_timeout_secs = if on { 180 } else { 0 };
            }
            ui.add_enabled(on, egui::Slider::new(&mut s.tx_timeout_secs, 30..=600).suffix(" s"));
        });
        ui.end_row();
        ui.label("Space bar");
        ui.checkbox(&mut s.space_bar_ptt, "Hold to transmit");
        ui.end_row();
    });
    hint(ui, "Esc always stops transmitting. Closing Seventy3 unkeys the rig.");
}

fn receiver_tab(ui: &mut Ui, s: &mut Settings, gains: &[i32]) {
    let sdr = &mut s.sdr;
    egui::Grid::new("rx").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("RTL-SDR device");
        ui.add(egui::DragValue::new(&mut sdr.device_index).range(0..=7).prefix("#"));
        ui.end_row();

        ui.label("Sample rate");
        egui::ComboBox::from_id_salt("rate")
            .selected_text(format!("{:.3} MS/s", f64::from(sdr.sample_rate) / 1e6))
            .show_ui(ui, |ui| {
                for r in SAMPLE_RATES {
                    ui.selectable_value(&mut sdr.sample_rate, r, format!("{:.3} MS/s", f64::from(r) / 1e6));
                }
            });
        ui.end_row();

        ui.label("RF gain");
        ui.horizontal(|ui| {
            let mut auto = sdr.gain_tenths_db.is_none();
            if ui.checkbox(&mut auto, "Auto").changed() {
                sdr.gain_tenths_db = if auto { None } else { Some(207) };
            }
            if let Some(g) = &mut sdr.gain_tenths_db {
                let max = gains.last().copied().unwrap_or(496);
                let mut db = *g as f32 / 10.0;
                if ui.add(egui::Slider::new(&mut db, 0.0..=max as f32 / 10.0).suffix(" dB").step_by(0.1)).changed() {
                    let want = (db * 10.0).round() as i32;
                    *g = gains.iter().copied().min_by_key(|x| (x - want).abs()).unwrap_or(want);
                }
            }
        });
        ui.end_row();

        ui.label("IF centre");
        ui.horizontal(|ui| {
            ui.add(
                egui::DragValue::new(&mut sdr.if_map.if_center_hz)
                    .speed(1.0)
                    .range(72_000_000.0..=74_000_000.0)
                    .suffix(" Hz")
                    .max_decimals(0),
            );
            if ui.small_button("Reset").on_hover_text("Back to 73.095 MHz").clicked() {
                sdr.if_map.if_center_hz = TS480_FIRST_IF_HZ;
            }
        });
        ui.end_row();

        ui.label("Spectrum");
        ui.checkbox(&mut sdr.if_map.inverted, "IF is mirrored (TS-480: yes)");
        ui.end_row();

        ui.label("Tuner offset");
        ui.add(egui::DragValue::new(&mut sdr.tune_offset_hz).speed(1000.0).range(0.0..=400_000.0).suffix(" Hz"));
        ui.end_row();

        ui.label("Frame rate");
        ui.add(egui::Slider::new(&mut sdr.frames_per_sec, 10.0..=60.0).suffix(" fps"));
        ui.end_row();
    });
    hint(
        ui,
        "Use Calibrate (toolbar) rather than editing the IF centre by hand: tune the rig to a \
         carrier on a known frequency and click it. The tuner offset keeps the RTL-SDR's DC \
         spike away from the dial frequency.",
    );
    if cfg!(windows) {
        hint(
            ui,
            "The RTL-SDR needs the WinUSB driver: run Zadig once and install it for \"Bulk-In, Interface (Interface 0)\".",
        );
    }
}

fn display_tab(ui: &mut Ui, s: &mut Settings) {
    let d = &mut s.display;
    egui::Grid::new("display").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Colours");
        egui::ComboBox::from_id_salt("palette").selected_text(d.palette.label()).show_ui(ui, |ui| {
            for p in Palette::ALL {
                ui.selectable_value(&mut d.palette, p, p.label());
            }
        });
        ui.end_row();

        ui.label("Levels");
        ui.checkbox(&mut d.auto_level, "Follow the noise floor");
        ui.end_row();

        ui.label("Floor");
        ui.add_enabled(!d.auto_level, egui::Slider::new(&mut d.floor_db, -150.0..=-40.0).suffix(" dB"));
        ui.end_row();

        ui.label("Contrast range");
        ui.add(egui::Slider::new(&mut d.range_db, 15.0..=90.0).suffix(" dB"));
        ui.end_row();

        ui.label("Averaging");
        ui.add(egui::Slider::new(&mut d.averaging, 0.0..=0.95));
        ui.end_row();

        ui.label("Waterfall speed");
        egui::ComboBox::from_id_salt("speed")
            .selected_text(match d.frames_per_line {
                1 => "Fast",
                2 => "Medium",
                _ => "Slow",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut d.frames_per_line, 1, "Fast");
                ui.selectable_value(&mut d.frames_per_line, 2, "Medium");
                ui.selectable_value(&mut d.frames_per_line, 4, "Slow");
            });
        ui.end_row();

        ui.label("Spectrum height");
        ui.add(egui::Slider::new(&mut d.spectrum_fraction, 0.15..=0.6).show_value(false));
        ui.end_row();

        ui.label("");
        ui.checkbox(&mut d.peak_hold, "Peak hold");
        ui.end_row();

        ui.label("");
        ui.checkbox(&mut d.pause_on_tx, "Pause the waterfall while transmitting");
        ui.end_row();
    });
}

fn tuning_tab(ui: &mut Ui, s: &mut Settings) {
    let t = &mut s.tuning;
    egui::Grid::new("tuning").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Wheel / arrow step");
        egui::ComboBox::from_id_salt("step").selected_text(format!("{} Hz", t.step_hz)).show_ui(ui, |ui| {
            for v in [10, 50, 100, 500, 1000] {
                ui.selectable_value(&mut t.step_hz, v, format!("{v} Hz"));
            }
        });
        ui.end_row();

        ui.label("Click snaps to");
        egui::ComboBox::from_id_salt("snap").selected_text(format!("{} Hz", t.click_snap_hz)).show_ui(ui, |ui| {
            for v in [10, 50, 100, 500, 1000] {
                ui.selectable_value(&mut t.click_snap_hz, v, format!("{v} Hz"));
            }
        });
        ui.end_row();

        ui.label("SSB clicks");
        ui.checkbox(&mut t.ssb_click_centers_passband, "Put the clicked signal in the passband");
        ui.end_row();

        ui.label("Spot markers");
        ui.checkbox(&mut s.pota.show_markers, "Show POTA spots on the panadapter");
        ui.end_row();
    });
    hint(ui, "Shift + wheel tunes in 10× steps; Ctrl + wheel zooms. Drag the panadapter to slide the band.");
}

fn about_tab(ui: &mut Ui, s: &mut Settings, u: &mut UpdatesView<'_>) {
    ui.label(RichText::new(format!("Seventy3 {}", env!("CARGO_PKG_VERSION"))).heading());
    ui.label("Panadapter, waterfall and CAT control for the Kenwood TS-480.");
    ui.add_space(8.0);

    ui.label(RichText::new("Updates").strong());
    ui.checkbox(&mut s.updates.auto_check, "Check for new versions automatically (daily)");
    ui.horizontal(|ui| {
        let busy = matches!(u.state, UpdateState::Checking | UpdateState::Downloading { .. });
        if ui.add_enabled(!busy, egui::Button::new("Check now")).clicked() {
            u.check_now = true;
        }
        let (text, color) = match u.state {
            UpdateState::Idle => ("Not checked yet".to_owned(), theme::TEXT_DIM),
            UpdateState::Checking => ("Checking…".to_owned(), theme::TEXT_DIM),
            UpdateState::UpToDate => ("You have the latest version".to_owned(), theme::OK),
            UpdateState::Available(a) => (format!("Version {} is available", a.version()), theme::ACCENT),
            UpdateState::Downloading { version, done, total } => {
                (format!("Downloading {version}: {}%", done * 100 / (*total).max(1)), theme::TEXT_DIM)
            }
            UpdateState::Installed { version } => (format!("{version} installed; restarting…"), theme::OK),
            UpdateState::Failed(e) => (e.clone(), theme::WARN),
        };
        ui.label(RichText::new(text).color(color));
    });
    if let UpdateState::Available(a) = u.state
        && ui.button(RichText::new(format!("Install {} and restart", a.version())).strong()).clicked()
    {
        u.install = true;
    }
    if let Some(secs) = u.last_check_secs {
        hint(ui, &format!("Last checked {} min ago.", secs / 60));
    }
    hint(
        ui,
        "Updates come from the project's GitHub releases. Each one is signed, and Seventy3 checks the \
         signature and the file's SHA-256 before installing. It never installs while you're transmitting.",
    );
    ui.hyperlink_to("All releases", RELEASES_PAGE);

    ui.add_space(8.0);
    ui.label(RichText::new("License").strong());
    ui.label(
        "Seventy3 is free software under the GNU General Public License, version 3 or later. \
         It comes with no warranty.",
    );
    ui.hyperlink_to("Source code", "https://github.com/KK4ODA/Seventy3");
}
