//! Settings, saved as JSON in the user's config folder
//! (`%APPDATA%\Seventy3\config\settings.json` on Windows).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::pota::ModeGroup;
use crate::rig::protocol::Mode;
use crate::rig::serial::SerialSettings;
use crate::sdr::SdrSettings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Palette {
    Classic,
    Ember,
    Phosphor,
    Gray,
}

impl Palette {
    pub const ALL: [Palette; 4] = [Palette::Classic, Palette::Ember, Palette::Phosphor, Palette::Gray];

    pub fn label(self) -> &'static str {
        match self {
            Palette::Classic => "Classic",
            Palette::Ember => "Ember",
            Palette::Phosphor => "Phosphor",
            Palette::Gray => "Grayscale",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplaySettings {
    pub palette: Palette,
    /// Follow the noise floor automatically.
    pub auto_level: bool,
    /// Bottom of the colour scale (manual mode), dBFS.
    pub floor_db: f32,
    /// Height of the colour scale, dB.
    pub range_db: f32,
    /// Spectrum trace averaging, 0 (none) to 0.95 (heavy).
    pub averaging: f32,
    /// Spectrum frames combined into each waterfall line (1 = fastest scroll).
    pub frames_per_line: u32,
    /// Don't add waterfall lines while transmitting (the IF tap sees our
    /// own signal, which would smear the history).
    pub pause_on_tx: bool,
    pub peak_hold: bool,
    /// Fraction of the panadapter height given to the spectrum trace.
    pub spectrum_fraction: f32,
}

impl Default for DisplaySettings {
    fn default() -> Self {
        DisplaySettings {
            palette: Palette::Classic,
            auto_level: true,
            floor_db: -110.0,
            range_db: 50.0,
            averaging: 0.6,
            frames_per_line: 1,
            pause_on_tx: true,
            peak_hold: false,
            spectrum_fraction: 0.32,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TuningSettings {
    /// Click-to-tune rounds the dial to a multiple of this.
    pub click_snap_hz: u32,
    /// One mouse-wheel notch or arrow key.
    pub step_hz: u32,
    /// For SSB, a click tunes so the clicked signal falls in the middle of
    /// the passband instead of on the dial frequency.
    pub ssb_click_centers_passband: bool,
}

impl Default for TuningSettings {
    fn default() -> Self {
        TuningSettings { click_snap_hz: 100, step_hz: 100, ssb_click_centers_passband: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PotaSettings {
    pub refresh_secs: u64,
    pub modes: Vec<ModeGroup>,
    pub current_band_only: bool,
    pub hide_qrt: bool,
    /// Draw spot markers on the panadapter.
    pub show_markers: bool,
}

impl Default for PotaSettings {
    fn default() -> Self {
        PotaSettings {
            refresh_secs: 60,
            modes: vec![ModeGroup::Ssb],
            current_band_only: false,
            hide_qrt: true,
            show_markers: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BandMemory {
    pub freq_hz: u64,
    pub mode: Mode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Look for a new version at startup and once a day.
    pub auto_check: bool,
    /// A version the user chose not to install; not offered again.
    pub skipped_version: Option<String>,
    /// When the last automatic check ran, Unix seconds.
    pub last_check_unix: i64,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings { auto_check: true, skipped_version: None, last_check_unix: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Run against the simulated rig and IF instead of hardware.
    pub demo: bool,
    /// Set once the first-run welcome has been dismissed.
    pub welcomed: bool,
    pub serial: SerialSettings,
    pub poll_ms: u64,
    /// Unkey after this many seconds of transmitting; 0 turns it off.
    pub tx_timeout_secs: u32,
    /// Hold the space bar to transmit.
    pub space_bar_ptt: bool,
    pub sdr: SdrSettings,
    pub display: DisplaySettings,
    pub tuning: TuningSettings,
    pub pota: PotaSettings,
    /// Last frequency and mode used on each band, by band name.
    pub band_memory: BTreeMap<String, BandMemory>,
    pub spots_panel: bool,
    pub updates: UpdateSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            demo: false,
            welcomed: false,
            serial: SerialSettings::default(),
            poll_ms: 100,
            tx_timeout_secs: 180,
            space_bar_ptt: true,
            sdr: SdrSettings::default(),
            display: DisplaySettings::default(),
            tuning: TuningSettings::default(),
            pota: PotaSettings::default(),
            band_memory: BTreeMap::new(),
            spots_panel: true,
            updates: UpdateSettings::default(),
        }
    }
}

pub fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("", "", "Seventy3")
}

pub fn settings_path() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().join("settings.json"))
}

impl Settings {
    /// Loads saved settings, falling back to defaults. A file that can't be
    /// read is set aside (not overwritten) so nothing is silently lost.
    pub fn load() -> Settings {
        let Some(path) = settings_path() else {
            return Settings::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("{}: {e}; starting from defaults", path.display());
                    let _ = std::fs::rename(&path, path.with_extension("json.bad"));
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = settings_path() else { return };
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
            // Write then rename, so a crash mid-write can't corrupt the file.
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, json)?;
            std::fs::rename(&tmp, &path)
        })();
        if let Err(e) = result {
            log::warn!("Couldn't save settings to {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let mut s = Settings::default();
        s.serial.port = "COM4".into();
        s.band_memory.insert("20".into(), BandMemory { freq_hz: 14_285_000, mode: Mode::Usb });
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn missing_fields_take_defaults() {
        let s: Settings = serde_json::from_str(r#"{"demo": true, "serial": {"port": "COM7"}}"#).unwrap();
        assert!(s.demo);
        assert_eq!(s.serial.port, "COM7");
        assert_eq!(s.serial.baud, 9600);
        assert_eq!(s.sdr.if_map.if_center_hz, 73_095_000.0);
        assert_eq!(s.tx_timeout_secs, 180);
    }
}
