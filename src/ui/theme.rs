//! Colours and widget styling.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};

pub const BG: Color32 = Color32::from_rgb(0x0b, 0x0e, 0x13);
pub const PANEL: Color32 = Color32::from_rgb(0x12, 0x16, 0x1d);
pub const RAISED: Color32 = Color32::from_rgb(0x1a, 0x20, 0x29);
pub const RAISED_HOVER: Color32 = Color32::from_rgb(0x23, 0x2b, 0x37);
pub const BORDER: Color32 = Color32::from_rgb(0x2a, 0x32, 0x3e);
pub const TEXT: Color32 = Color32::from_rgb(0xd9, 0xdf, 0xe8);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x86, 0x91, 0xa3);
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x55, 0x5f, 0x6e);
/// The VFO readout and other "this is the radio" accents.
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0xb3, 0x47);
pub const TX: Color32 = Color32::from_rgb(0xe8, 0x45, 0x4b);
pub const OK: Color32 = Color32::from_rgb(0x3d, 0xd6, 0x8c);
pub const WARN: Color32 = Color32::from_rgb(0xf5, 0xa5, 0x24);
/// POTA spot markers.
pub const SPOT: Color32 = Color32::from_rgb(0x6f, 0xd8, 0x6b);
pub const SPOT_CW: Color32 = Color32::from_rgb(0x5c, 0xc8, 0xf2);
pub const SPOT_DATA: Color32 = Color32::from_rgb(0xc3, 0x9b, 0xf7);
pub const SPECTRUM_BG: Color32 = Color32::from_rgb(0x07, 0x0a, 0x0e);
pub const GRID: Color32 = Color32::from_rgb(0x1c, 0x23, 0x2d);

pub const MONO: FontFamily = FontFamily::Monospace;

pub fn apply(ctx: &egui::Context) {
    let mut v = Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = RAISED;
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(8);
    v.selection.bg_fill = ACCENT.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.override_text_color = Some(TEXT);

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.weak_bg_fill = RAISED;
    w.inactive.bg_fill = RAISED;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.hovered.weak_bg_fill = RAISED_HOVER;
    w.hovered.bg_fill = RAISED_HOVER;
    w.hovered.bg_stroke = Stroke::new(1.0, TEXT_FAINT);
    w.active.weak_bg_fill = RAISED_HOVER;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = CornerRadius::same(5);
    }
    ctx.set_visuals(v);

    ctx.all_styles_mut(|style| {
        style.text_styles.insert(TextStyle::Body, FontId::proportional(14.0));
        style.text_styles.insert(TextStyle::Button, FontId::proportional(14.0));
        style.text_styles.insert(TextStyle::Small, FontId::proportional(11.5));
        style.text_styles.insert(TextStyle::Monospace, FontId::monospace(13.5));
        style.text_styles.insert(TextStyle::Heading, FontId::proportional(18.0));
        style.spacing.button_padding = egui::vec2(9.0, 4.0);
        style.spacing.item_spacing = egui::vec2(7.0, 6.0);
    });
}
