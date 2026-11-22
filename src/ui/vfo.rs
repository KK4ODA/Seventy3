//! The big frequency readout. Scroll over a digit to change it; double-click
//! to type a frequency.

use egui::{Align2, Color32, CornerRadius, FontId, Key, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

use super::theme;

#[derive(Default)]
pub struct VfoState {
    editing: Option<String>,
    wheel_points: f32,
    focus_requested: bool,
}

pub enum VfoAction {
    Set(u64),
    Step(i64),
}

/// Parses typed frequencies: below 100 is MHz ("14.285"), below 100 000 is
/// kHz ("14285.5"), anything larger is Hz. "14.285.000" is read as Hz.
pub fn parse_freq_input(text: &str) -> Option<u64> {
    let t: String = text.chars().filter(|c| !c.is_whitespace() && *c != ',' && *c != '_').collect();
    let t = t.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    if t.is_empty() {
        return None;
    }
    if t.matches('.').count() > 1 {
        return t.replace('.', "").parse().ok();
    }
    let v: f64 = t.parse().ok()?;
    if !v.is_finite() || v <= 0.0 {
        return None;
    }
    let hz = if v < 100.0 {
        v * 1e6
    } else if v < 100_000.0 {
        v * 1e3
    } else {
        v
    };
    Some(hz.round() as u64)
}

pub fn show(ui: &mut Ui, freq_hz: u64, state: &mut VfoState, live: bool) -> Option<VfoAction> {
    let font = FontId::monospace(38.0);
    let color = if live { theme::ACCENT } else { theme::TEXT_FAINT };
    let digit_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
    let dot_w = digit_w * 0.55;
    let height = 46.0;

    // (character, place value it edits)
    let mhz = freq_hz / 1_000_000;
    let khz = (freq_hz / 1000) % 1000;
    let hz = freq_hz % 1000;
    let text = format!("{mhz:>2}.{khz:03}.{hz:03}");
    let places: [Option<u64>; 10] = [
        Some(10_000_000),
        Some(1_000_000),
        None,
        Some(100_000),
        Some(10_000),
        Some(1_000),
        None,
        Some(100),
        Some(10),
        Some(1),
    ];
    let width: f32 = places.iter().map(|p| if p.is_some() { digit_w } else { dot_w }).sum::<f32>() + 16.0;

    if let Some(buf) = &mut state.editing {
        let mut action = None;
        let resp = ui.add_sized(
            vec2(width, height),
            egui::TextEdit::singleline(buf)
                .font(FontId::monospace(26.0))
                .hint_text("kHz or MHz")
                .align(egui::Align2::CENTER_CENTER),
        );
        if !state.focus_requested {
            resp.request_focus();
            state.focus_requested = true;
        }
        let enter = ui.input(|i| i.key_pressed(Key::Enter));
        let escape = ui.input(|i| i.key_pressed(Key::Escape));
        if enter {
            action = parse_freq_input(buf).map(VfoAction::Set);
            state.editing = None;
        } else if escape || resp.lost_focus() {
            state.editing = None;
        }
        return action;
    }

    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let painter = ui.painter_at(rect);
    painter.rect(rect, CornerRadius::same(6), theme::BG, Stroke::new(1.0, theme::BORDER), StrokeKind::Inside);

    let pointer = resp.hover_pos();
    let mut x = rect.left() + 8.0;
    let mut hovered_place = None;
    for (ch, place) in text.chars().zip(places) {
        let w = if place.is_some() { digit_w } else { dot_w };
        let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(w, height));
        let hovered = place.is_some() && pointer.is_some_and(|p| cell.contains(p));
        if hovered {
            hovered_place = place;
            painter.rect_filled(cell.shrink2(vec2(0.0, 5.0)), CornerRadius::same(3), theme::RAISED_HOVER);
        }
        // Dim the sub-kHz digits a little; they matter less for SSB.
        let c = match place {
            Some(p) if p < 1000 => color.gamma_multiply(0.75),
            _ => color,
        };
        painter.text(cell.center(), Align2::CENTER_CENTER, ch, font.clone(), c);
        x += w;
    }

    let mut action = None;
    if let Some(place) = hovered_place {
        let scroll: f32 = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::MouseWheel { unit, delta, .. } => Some(match unit {
                        egui::MouseWheelUnit::Line => delta.y * 50.0,
                        egui::MouseWheelUnit::Point => delta.y,
                        egui::MouseWheelUnit::Page => delta.y * 500.0,
                    }),
                    _ => None,
                })
                .sum()
        });
        state.wheel_points += scroll;
        let mut notches = 0i64;
        while state.wheel_points >= 50.0 {
            notches += 1;
            state.wheel_points -= 50.0;
        }
        while state.wheel_points <= -50.0 {
            notches -= 1;
            state.wheel_points += 50.0;
        }
        if notches != 0 {
            action = Some(VfoAction::Step(notches * place as i64));
        }
    }
    if resp.double_clicked() {
        state.editing = Some(format!("{:.3}", freq_hz as f64 / 1000.0));
        state.focus_requested = false;
    }
    resp.on_hover_text("Scroll over a digit to change it. Double-click to type a frequency.");
    action
}

/// A small coloured status dot with a label.
pub fn status_dot(ui: &mut Ui, color: Color32, text: impl Into<egui::WidgetText>) -> egui::Response {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 4.0, color);
        ui.label(text)
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::parse_freq_input as p;

    #[test]
    fn parses_typed_frequencies() {
        assert_eq!(p("14.285"), Some(14_285_000));
        assert_eq!(p("14285"), Some(14_285_000));
        assert_eq!(p("14285.5"), Some(14_285_500));
        assert_eq!(p("7.2"), Some(7_200_000));
        assert_eq!(p("7074000"), Some(7_074_000));
        assert_eq!(p("14.285.000"), Some(14_285_000));
        assert_eq!(p(" 21 300 "), Some(21_300_000));
        assert_eq!(p("14285 kHz"), Some(14_285_000));
        assert_eq!(p(""), None);
        assert_eq!(p("abc"), None);
        assert_eq!(p("-5"), None);
    }
}
