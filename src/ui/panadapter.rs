//! The panadapter: spectrum trace on top, frequency scale, waterfall below,
//! with POTA spot markers, the receive passband, and mouse tuning.

use std::time::{Duration, Instant};

use egui::{Align2, Color32, CornerRadius, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};

use super::theme;
use super::waterfall::Waterfall;
use crate::config::TuningSettings;
use crate::pota::{ModeGroup, Spot};
use crate::rig::protocol::{Mode, Passband};

/// The spectrum line, smoothed for display.
#[derive(Default)]
pub struct Trace {
    pub db: Vec<f32>,
    pub peak: Vec<f32>,
    pub start_hz: f64,
    pub col_hz: f64,
}

pub struct PanInput<'a> {
    pub trace: &'a Trace,
    pub show_peak: bool,
    pub waterfall: &'a Waterfall,
    pub dial_hz: u64,
    pub mode: Option<Mode>,
    pub passband: Option<Passband>,
    pub transmitting: bool,
    pub half_span_hz: f64,
    pub floor_db: f32,
    pub range_db: f32,
    pub spectrum_fraction: f32,
    pub spots: &'a [Spot],
    pub calibrating: bool,
    pub tuning: &'a TuningSettings,
    /// Shown in the middle when there is nothing to draw yet.
    pub placeholder: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PanAction {
    /// Tune the dial to this frequency.
    Tune(u64),
    /// Move the dial by this many Hz from where it is (or is about to be).
    TuneBy(i64),
    /// Tune to the spot with this id.
    TuneSpot(u64),
    /// Zoom in (positive) or out (negative) this many steps.
    Zoom(i32),
    /// Calibration click: the reference carrier is displayed here.
    Calibrate(f64),
}

/// Interaction state kept between frames.
#[derive(Default)]
pub struct PanState {
    drag_base: u64,
    drag_total_hz: f64,
    wheel_points: f32,
}

const AXIS_H: f32 = 20.0;
const LABEL_ROWS: usize = 3;
const LABEL_H: f32 = 17.0;
const TRACE: Color32 = Color32::from_rgb(0xa9, 0xd8, 0xff);

pub fn spot_color(group: ModeGroup) -> Color32 {
    match group {
        ModeGroup::Ssb => theme::SPOT,
        ModeGroup::Cw => theme::SPOT_CW,
        ModeGroup::Data => theme::SPOT_DATA,
        ModeGroup::Other => theme::TEXT_DIM,
    }
}

/// A "nice" grid step (1, 2, 5 × 10^n Hz) giving roughly `target` divisions.
pub fn nice_step(span_hz: f64, target: f64) -> f64 {
    let raw = span_hz / target.max(1.0);
    let mag = 10f64.powf(raw.log10().floor());
    for m in [1.0, 2.0, 5.0, 10.0] {
        if m * mag >= raw {
            return m * mag;
        }
    }
    10.0 * mag
}

/// "14.285" for a 1 kHz step, "14.2855" for 500 Hz, and so on.
pub fn format_mhz(hz: f64, step_hz: f64) -> String {
    let decimals = if step_hz >= 1000.0 {
        3
    } else if step_hz >= 100.0 {
        4
    } else if step_hz >= 10.0 {
        5
    } else {
        6
    };
    format!("{:.*}", decimals, hz / 1e6)
}

fn age_text(secs: i64) -> String {
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else {
        format!("{}h{:02} ago", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn show(ui: &mut Ui, input: &PanInput, state: &mut PanState) -> Vec<PanAction> {
    let mut actions = Vec::new();
    let rect = ui.available_rect_before_wrap();
    let resp = ui.allocate_rect(rect, Sense::click_and_drag());
    let painter = ui.painter_at(rect);

    let half = input.half_span_hz.max(1.0);
    let dial = input.dial_hz as f64;
    let width = rect.width().max(1.0);
    let x_of = |off: f64| rect.left() + ((off + half) / (2.0 * half)) as f32 * width;
    let off_of = |x: f32| f64::from((x - rect.left()) / width) * 2.0 * half - half;
    let hz_per_point = 2.0 * half / f64::from(width);

    let spec_h = ((rect.height() - AXIS_H) * input.spectrum_fraction).max(60.0);
    let spec = Rect::from_min_size(rect.min, vec2(rect.width(), spec_h));
    let axis = Rect::from_min_size(pos2(rect.left(), spec.bottom()), vec2(rect.width(), AXIS_H));
    let wf = Rect::from_min_max(pos2(rect.left(), axis.bottom()), rect.max);

    // ---- frequency grid ---------------------------------------------------
    let step = nice_step(2.0 * half, f64::from(width / 95.0));
    let first = ((dial - half) / step).ceil() * step;
    let mut ticks = Vec::new();
    let mut f = first;
    while f <= dial + half {
        ticks.push(f);
        f += step;
    }

    // ---- spectrum ---------------------------------------------------------
    painter.rect_filled(spec, CornerRadius::ZERO, theme::SPECTRUM_BG);
    let db_lo = input.floor_db - 5.0;
    let db_hi = input.floor_db + input.range_db + 15.0;
    let y_of = |db: f32| spec.bottom() - ((db - db_lo) / (db_hi - db_lo)).clamp(0.0, 1.0) * spec.height();

    for &t in &ticks {
        let x = x_of(t - dial);
        painter.line_segment([pos2(x, spec.top()), pos2(x, spec.bottom())], Stroke::new(1.0, theme::GRID));
    }
    let mut level = (db_lo / 10.0).ceil() * 10.0;
    while level < db_hi {
        let y = y_of(level);
        painter.line_segment([pos2(spec.left(), y), pos2(spec.right(), y)], Stroke::new(1.0, theme::GRID));
        painter.text(
            pos2(spec.left() + 4.0, y - 1.0),
            Align2::LEFT_BOTTOM,
            format!("{level:.0}"),
            FontId::proportional(10.0),
            theme::TEXT_FAINT,
        );
        level += 10.0;
    }

    // Receive passband.
    if let Some(pb) = input.passband {
        let x0 = x_of(pb.low_hz);
        let x1 = x_of(pb.high_hz);
        let band = Rect::from_x_y_ranges(x0..=x1.max(x0 + 1.0), spec.y_range());
        painter.rect_filled(band, CornerRadius::ZERO, Color32::from_white_alpha(18));
        for x in [x0, x1] {
            painter.line_segment(
                [pos2(x, spec.top()), pos2(x, spec.bottom())],
                Stroke::new(1.0, Color32::from_white_alpha(50)),
            );
        }
    }

    let tr = input.trace;
    if !tr.db.is_empty() {
        let pts: Vec<Pos2> =
            tr.db.iter().enumerate().map(|(i, &d)| pos2(x_of(tr.start_hz + i as f64 * tr.col_hz), y_of(d))).collect();
        // Soft fill under the line.
        let mut mesh = Mesh::default();
        let top = TRACE.gamma_multiply(0.30);
        let bottom = TRACE.gamma_multiply(0.02);
        for (i, p) in pts.iter().enumerate() {
            mesh.colored_vertex(*p, top);
            mesh.colored_vertex(pos2(p.x, spec.bottom()), bottom);
            if i > 0 {
                let k = (2 * i) as u32;
                mesh.add_triangle(k - 2, k - 1, k);
                mesh.add_triangle(k - 1, k + 1, k);
            }
        }
        painter.add(Shape::mesh(mesh));
        if input.show_peak && tr.peak.len() == tr.db.len() {
            let peak: Vec<Pos2> = tr
                .peak
                .iter()
                .enumerate()
                .map(|(i, &d)| pos2(x_of(tr.start_hz + i as f64 * tr.col_hz), y_of(d)))
                .collect();
            painter.add(Shape::line(peak, Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.45))));
        }
        painter.add(Shape::line(pts, Stroke::new(1.3, TRACE)));
    }

    // ---- frequency axis ---------------------------------------------------
    painter.rect_filled(axis, CornerRadius::ZERO, theme::PANEL);
    for &t in &ticks {
        let x = x_of(t - dial);
        painter.line_segment([pos2(x, axis.top()), pos2(x, axis.top() + 4.0)], Stroke::new(1.0, theme::TEXT_FAINT));
        painter.text(
            pos2(x, axis.center().y + 2.0),
            Align2::CENTER_CENTER,
            format_mhz(t, step),
            FontId::proportional(11.0),
            theme::TEXT_DIM,
        );
    }

    // ---- waterfall ----------------------------------------------------------
    painter.rect_filled(wf, CornerRadius::ZERO, Color32::BLACK);
    input.waterfall.paint(&painter.with_clip_rect(wf), wf, dial, x_of);
    if let Some(pb) = input.passband {
        for x in [x_of(pb.low_hz), x_of(pb.high_hz)] {
            painter.line_segment(
                [pos2(x, wf.top()), pos2(x, wf.bottom())],
                Stroke::new(1.0, Color32::from_white_alpha(28)),
            );
        }
    }

    // Dial marker through everything.
    let xd = x_of(0.0);
    painter.line_segment(
        [pos2(xd, spec.top()), pos2(xd, spec.bottom())],
        Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.85)),
    );
    painter.line_segment(
        [pos2(xd, wf.top()), pos2(xd, wf.bottom())],
        Stroke::new(1.0, theme::ACCENT.gamma_multiply(0.35)),
    );
    painter.add(Shape::convex_polygon(
        vec![pos2(xd - 5.0, axis.top()), pos2(xd + 5.0, axis.top()), pos2(xd, axis.top() + 6.0)],
        theme::ACCENT,
        Stroke::NONE,
    ));

    // ---- POTA spot markers --------------------------------------------------
    let pointer = resp.hover_pos();
    let now = crate::pota::now_unix();
    let mut hovered_spot: Option<&Spot> = None;
    let mut row_right = [f32::NEG_INFINITY; LABEL_ROWS];
    let mut visible: Vec<&Spot> = input.spots.iter().filter(|s| ((s.freq_hz as f64) - dial).abs() <= half).collect();
    visible.sort_by_key(|s| s.freq_hz);
    for s in visible {
        let x = x_of(s.freq_hz as f64 - dial);
        let color = spot_color(s.mode_group());
        let tuned = (s.freq_hz as f64 - dial).abs() < 300.0;
        let galley = painter.layout_no_wrap(
            s.activator.clone(),
            FontId::proportional(11.5),
            if tuned { theme::BG } else { color },
        );
        let w = galley.size().x + 10.0;
        let Some(row) = (0..LABEL_ROWS).find(|&r| row_right[r] < x - 3.0) else {
            // No room for a label: just a tick.
            painter.line_segment([pos2(x, spec.top()), pos2(x, spec.top() + 6.0)], Stroke::new(1.5, color));
            continue;
        };
        let label =
            Rect::from_min_size(pos2(x - 1.0, spec.top() + 3.0 + row as f32 * (LABEL_H + 2.0)), vec2(w, LABEL_H));
        row_right[row] = label.right();
        painter.line_segment(
            [pos2(x, label.bottom()), pos2(x, spec.bottom())],
            Stroke::new(1.0, color.gamma_multiply(if tuned { 0.8 } else { 0.35 })),
        );
        let hovered = pointer.is_some_and(|p| label.contains(p));
        if hovered {
            hovered_spot = Some(s);
        }
        let fill = if tuned { color } else { theme::BG.gamma_multiply(0.85) };
        painter.rect(
            label,
            CornerRadius::same(3),
            fill,
            Stroke::new(if hovered { 1.5 } else { 1.0 }, color),
            StrokeKind::Inside,
        );
        painter.galley(pos2(label.left() + 5.0, label.center().y - galley.size().y / 2.0), galley, color);
        if s.is_qrt() {
            painter.line_segment([label.left_center(), label.right_center()], Stroke::new(1.0, color));
        }
    }

    // ---- overlays -----------------------------------------------------------
    // A banner on a solid pill, readable over any waterfall content.
    let banner = |text: &str, color: Color32, fill: Color32| {
        let galley = painter.layout_no_wrap(text.to_owned(), FontId::proportional(13.5), color);
        let pill = Rect::from_center_size(pos2(wf.center().x, wf.top() + 20.0), galley.size() + vec2(20.0, 8.0));
        painter.rect_filled(pill, CornerRadius::same(12), fill);
        painter.galley(pill.min + vec2(10.0, 4.0), galley, color);
    };
    if input.transmitting {
        painter.rect_stroke(rect.shrink(1.0), CornerRadius::ZERO, Stroke::new(2.0, theme::TX), StrokeKind::Inside);
        banner("TRANSMITTING", Color32::WHITE, theme::TX);
    } else if input.calibrating {
        painter.rect_stroke(rect.shrink(1.0), CornerRadius::ZERO, Stroke::new(2.0, theme::WARN), StrokeKind::Inside);
        banner("Calibrating: click the centre of the reference carrier", theme::BG, theme::WARN);
    }
    if let Some(text) = &input.placeholder {
        painter.text(wf.center(), Align2::CENTER_CENTER, text, FontId::proportional(15.0), theme::TEXT_DIM);
    }

    // Cursor read-out.
    if let Some(p) = pointer {
        let off = off_of(p.x);
        let stroke = Stroke::new(1.0, if input.calibrating { theme::WARN } else { Color32::from_white_alpha(70) });
        painter.line_segment([pos2(p.x, rect.top()), pos2(p.x, rect.bottom())], stroke);
        if input.calibrating {
            painter.line_segment([pos2(rect.left(), p.y), pos2(rect.right(), p.y)], stroke);
        }
        let text = format_mhz(dial + off, 10.0);
        let galley = painter.layout_no_wrap(text, FontId::monospace(11.5), theme::TEXT);
        let mut tag = Rect::from_center_size(axis.center(), galley.size() + vec2(10.0, 4.0));
        tag = tag.translate(vec2(p.x - tag.center().x, 0.0));
        let dx = (rect.left() - tag.left()).max(0.0) + (rect.right() - tag.right()).min(0.0);
        tag = tag.translate(vec2(dx, 0.0));
        painter.rect(tag, CornerRadius::same(3), theme::RAISED, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);
        painter.galley(tag.min + vec2(5.0, 2.0), galley, theme::TEXT);
        if spec.contains(p) && !tr.db.is_empty() {
            let i = ((off - tr.start_hz) / tr.col_hz).round();
            if i >= 0.0 && (i as usize) < tr.db.len() {
                painter.text(
                    pos2(p.x + 6.0, p.y - 4.0),
                    Align2::LEFT_BOTTOM,
                    format!("{:.0} dB", tr.db[i as usize]),
                    FontId::proportional(11.0),
                    theme::TEXT_DIM,
                );
            }
        }
    }

    // ---- interaction --------------------------------------------------------
    let resp =
        if let Some(s) = hovered_spot { resp.on_hover_ui_at_pointer(|ui| spot_tooltip(ui, s, now)) } else { resp };

    if resp.clicked() {
        if let Some(s) = hovered_spot {
            actions.push(PanAction::TuneSpot(s.id));
        } else if let Some(p) = resp.interact_pointer_pos() {
            let rf = dial + off_of(p.x);
            if input.calibrating {
                actions.push(PanAction::Calibrate(rf));
            } else {
                actions.push(PanAction::Tune(click_target(rf, input.mode, input.passband, input.tuning)));
            }
        }
    }
    if resp.drag_started() {
        state.drag_base = input.dial_hz;
        state.drag_total_hz = 0.0;
    }
    if resp.dragged() && !input.calibrating {
        // Grab the band and slide it: dragging left moves the dial up.
        let dx = resp.drag_delta().x;
        if dx != 0.0 {
            state.drag_total_hz -= f64::from(dx) * hz_per_point;
            let target = state.drag_base as f64 + state.drag_total_hz;
            actions.push(PanAction::Tune(((target / 10.0).round() * 10.0).max(0.0) as u64));
        }
    }
    if resp.hovered() {
        let (events, modifiers) = ui.input(|i| (i.events.clone(), i.modifiers));
        let mut notches = 0i32;
        for e in events {
            if let egui::Event::MouseWheel { unit, delta, .. } = e {
                let d = if delta.y != 0.0 { delta.y } else { delta.x };
                match unit {
                    egui::MouseWheelUnit::Line => state.wheel_points += d * 50.0,
                    egui::MouseWheelUnit::Point => state.wheel_points += d,
                    egui::MouseWheelUnit::Page => state.wheel_points += d * 500.0,
                }
            }
        }
        while state.wheel_points >= 50.0 {
            notches += 1;
            state.wheel_points -= 50.0;
        }
        while state.wheel_points <= -50.0 {
            notches -= 1;
            state.wheel_points += 50.0;
        }
        if notches != 0 {
            if modifiers.command || modifiers.ctrl {
                actions.push(PanAction::Zoom(notches));
            } else {
                let step = i64::from(input.tuning.step_hz) * if modifiers.shift { 10 } else { 1 };
                actions.push(PanAction::TuneBy(i64::from(notches) * step));
            }
        }
    }
    resp.context_menu(|ui| {
        if let Some(p) = resp.interact_pointer_pos().or(pointer) {
            let rf = dial + off_of(p.x);
            if ui.button(format!("Tune to {} MHz", format_mhz(rf, 10.0))).clicked() {
                actions.push(PanAction::Tune(((rf / 10.0).round() * 10.0) as u64));
                ui.close();
            }
        }
        if ui.button("Zoom in").clicked() {
            actions.push(PanAction::Zoom(1));
            ui.close();
        }
        if ui.button("Zoom out").clicked() {
            actions.push(PanAction::Zoom(-1));
            ui.close();
        }
    });
    actions
}

/// Where a click at `rf` should put the dial.
pub fn click_target(rf: f64, mode: Option<Mode>, passband: Option<Passband>, tuning: &TuningSettings) -> u64 {
    let mut dial = rf;
    if let (Some(m), Some(pb)) = (mode, passband)
        && m.is_ssb()
        && tuning.ssb_click_centers_passband
    {
        dial = rf - (pb.low_hz + pb.high_hz) / 2.0;
    }
    let snap = f64::from(tuning.click_snap_hz.max(1));
    ((dial / snap).round() * snap).max(0.0) as u64
}

fn spot_tooltip(ui: &mut Ui, s: &Spot, now: i64) {
    ui.set_max_width(320.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(&s.activator).strong().color(spot_color(s.mode_group())));
        ui.label(
            egui::RichText::new(format!("{:.1} kHz {}", s.freq_hz as f64 / 1000.0, s.mode)).color(theme::TEXT_DIM),
        );
    });
    ui.label(format!("{}  {}", s.reference, s.park_name));
    if !s.location.is_empty() {
        ui.label(egui::RichText::new(&s.location).color(theme::TEXT_DIM));
    }
    if !s.comments.is_empty() {
        ui.label(egui::RichText::new(format!("“{}”", s.comments)).italics());
    }
    ui.label(
        egui::RichText::new(format!("Spotted by {} {}", s.spotter, age_text(s.age_secs(now))))
            .small()
            .color(theme::TEXT_DIM),
    );
    ui.label(egui::RichText::new("Click to tune").small().color(theme::TEXT_FAINT));
}

/// Remembers the most recent tuning target so rapid relative steps (wheel,
/// arrow keys) build on each other even before the rig confirms them.
#[derive(Default)]
pub struct TuneTracker {
    last: Option<(u64, Instant)>,
}

impl TuneTracker {
    pub fn base(&self, dial: u64) -> u64 {
        match self.last {
            Some((f, t)) if t.elapsed() < Duration::from_millis(400) => f,
            _ => dial,
        }
    }

    pub fn record(&mut self, target: u64) {
        self.last = Some((target, Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nice_steps() {
        assert_eq!(nice_step(1_600_000.0, 16.0), 100_000.0);
        assert_eq!(nice_step(30_000.0, 12.0), 5_000.0);
        assert_eq!(nice_step(15_000.0, 12.0), 2_000.0);
        assert_eq!(nice_step(1000.0, 10.0), 100.0);
    }

    #[test]
    fn formats_labels() {
        assert_eq!(format_mhz(14_285_000.0, 5000.0), "14.285");
        assert_eq!(format_mhz(14_285_500.0, 500.0), "14.2855");
        assert_eq!(format_mhz(7_074_120.0, 10.0), "7.07412");
    }

    #[test]
    fn ssb_click_centres_the_passband() {
        let t = TuningSettings::default();
        let usb = Passband { low_hz: 300.0, high_hz: 2700.0 };
        // Voice centred at 14.2865 → dial 14.285.
        assert_eq!(click_target(14_286_500.0, Some(Mode::Usb), Some(usb), &t), 14_285_000);
        let lsb = Passband { low_hz: -2700.0, high_hz: -300.0 };
        assert_eq!(click_target(7_198_500.0, Some(Mode::Lsb), Some(lsb), &t), 7_200_000);
        // CW: the click is the dial.
        let cw = Passband { low_hz: -250.0, high_hz: 250.0 };
        assert_eq!(click_target(7_030_040.0, Some(Mode::Cw), Some(cw), &t), 7_030_000);
        let exact = TuningSettings { ssb_click_centers_passband: false, click_snap_hz: 10, ..t };
        assert_eq!(click_target(14_286_512.0, Some(Mode::Usb), Some(usb), &exact), 14_286_510);
    }
}
