//! The POTA spots list.

use egui::{RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};

use super::panadapter::spot_color;
use super::theme;
use crate::config::PotaSettings;
use crate::pota::{FeedState, ModeGroup, Spot};

pub struct SpotsState {
    pub newest_first: bool,
}

impl Default for SpotsState {
    fn default() -> Self {
        SpotsState { newest_first: true }
    }
}

/// Spots passing the user's filters.
pub fn filtered<'a>(spots: &'a [Spot], settings: &PotaSettings, dial_hz: u64) -> Vec<&'a Spot> {
    let band = crate::bands::band_for(dial_hz);
    spots
        .iter()
        .filter(|s| settings.modes.is_empty() || settings.modes.contains(&s.mode_group()))
        .filter(|s| !settings.hide_qrt || !s.is_qrt())
        .filter(|s| !settings.current_band_only || (band.is_some() && s.band() == band))
        .collect()
}

fn age(secs: i64) -> String {
    if secs < 60 {
        "<1m".into()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

/// Draws the panel. Returns the id of a spot the user clicked.
pub fn show(
    ui: &mut Ui,
    feed: &FeedState,
    settings: &mut PotaSettings,
    state: &mut SpotsState,
    dial_hz: u64,
    on_refresh: impl FnOnce(),
) -> Option<u64> {
    let mut clicked = None;
    ui.horizontal(|ui| {
        ui.heading("POTA spots");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(!feed.fetching, egui::Button::new("⟳").small()).on_hover_text("Refresh now").clicked() {
                on_refresh();
            }
            let status = if feed.fetching {
                RichText::new("updating…").color(theme::TEXT_DIM)
            } else if feed.error.is_some() {
                RichText::new("offline").color(theme::WARN)
            } else if let Some(t) = feed.updated {
                RichText::new(format!("{}s ago", t.elapsed().as_secs())).color(theme::TEXT_DIM)
            } else {
                RichText::new("")
            };
            ui.label(status.small());
        });
    });

    ui.horizontal_wrapped(|ui| {
        for group in [ModeGroup::Ssb, ModeGroup::Cw, ModeGroup::Data] {
            let mut on = settings.modes.contains(&group);
            let text = RichText::new(group.label()).color(if on { spot_color(group) } else { theme::TEXT_DIM });
            if ui.toggle_value(&mut on, text).changed() {
                if on {
                    settings.modes.push(group);
                } else {
                    settings.modes.retain(|g| *g != group);
                }
            }
        }
        ui.separator();
        ui.toggle_value(&mut settings.current_band_only, "Band")
            .on_hover_text("Only spots on the band you're tuned to");
        ui.toggle_value(&mut settings.hide_qrt, "No QRT").on_hover_text("Hide activators who have signed off");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let label = if state.newest_first { "Newest ▾" } else { "Freq ▾" };
            if ui.button(label).on_hover_text("Sort order").clicked() {
                state.newest_first = !state.newest_first;
            }
        });
    });
    if let Some(e) = &feed.error {
        ui.label(RichText::new(e).small().color(theme::WARN));
    }
    ui.add_space(2.0);

    let mut rows = filtered(&feed.spots, settings, dial_hz);
    if state.newest_first {
        rows.sort_by_key(|s| std::cmp::Reverse(s.time_unix));
    }
    let now = crate::pota::now_unix();
    if rows.is_empty() {
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(if feed.spots.is_empty() { "No spots yet" } else { "No spots match the filters" })
                    .color(theme::TEXT_DIM),
            );
        });
        return None;
    }

    let row_h = 34.0;
    TableBuilder::new(ui)
        .striped(true)
        .sense(Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(64.0))
        .column(Column::exact(92.0))
        .column(Column::remainder().clip(true))
        .column(Column::exact(30.0))
        .header(18.0, |mut h| {
            for title in ["kHz", "Activator", "Park", "Age"] {
                h.col(|ui| {
                    ui.label(RichText::new(title).small().color(theme::TEXT_DIM));
                });
            }
        })
        .body(|body| {
            body.rows(row_h, rows.len(), |mut row| {
                let s = rows[row.index()];
                let tuned = (s.freq_hz as i64 - dial_hz as i64).abs() < 300;
                row.set_selected(tuned);
                let color = spot_color(s.mode_group());
                let dim = if s.is_qrt() { 0.45 } else { 1.0 };
                row.col(|ui| {
                    ui.label(
                        RichText::new(format!("{:.1}", s.freq_hz as f64 / 1000.0))
                            .monospace()
                            .color(theme::TEXT.gamma_multiply(dim)),
                    );
                });
                row.col(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(RichText::new(&s.activator).strong().color(color.gamma_multiply(dim)));
                        ui.label(RichText::new(&s.mode).small().color(theme::TEXT_DIM));
                    });
                });
                row.col(|ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(
                            RichText::new(format!("{} {}", s.reference, s.location))
                                .color(theme::TEXT.gamma_multiply(dim)),
                        );
                        ui.label(RichText::new(&s.park_name).small().color(theme::TEXT_DIM));
                    });
                });
                row.col(|ui| {
                    ui.label(RichText::new(age(s.age_secs(now))).small().color(theme::TEXT_DIM));
                });
                let resp = row.response();
                let resp = if s.comments.is_empty() {
                    resp
                } else {
                    resp.on_hover_text(format!("“{}” — {}", s.comments, s.spotter))
                };
                if resp.clicked() {
                    clicked = Some(s.id);
                }
            });
        });
    clicked
}
