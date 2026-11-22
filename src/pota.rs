//! Parks on the Air activator spots from the public POTA API.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::bands::{Band, band_for};
use crate::rig::Notify;
use crate::rig::protocol::Mode;

pub const SPOTS_URL: &str = "https://api.pota.app/spot/activator";

#[derive(Debug, Clone, PartialEq)]
pub struct Spot {
    pub id: u64,
    pub activator: String,
    pub freq_hz: u64,
    /// As reported: "SSB", "CW", "FT8", sometimes "LSB"/"USB" or empty.
    pub mode: String,
    /// Park reference, e.g. "US-1234".
    pub reference: String,
    pub park_name: String,
    /// E.g. "US-TN".
    pub location: String,
    pub spotter: String,
    pub comments: String,
    /// Spot time, Unix seconds (UTC).
    pub time_unix: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModeGroup {
    Ssb,
    Cw,
    Data,
    Other,
}

impl ModeGroup {
    pub fn label(self) -> &'static str {
        match self {
            ModeGroup::Ssb => "SSB",
            ModeGroup::Cw => "CW",
            ModeGroup::Data => "Data",
            ModeGroup::Other => "Other",
        }
    }
}

impl Spot {
    pub fn mode_group(&self) -> ModeGroup {
        match self.mode.to_ascii_uppercase().as_str() {
            "SSB" | "LSB" | "USB" | "PHONE" => ModeGroup::Ssb,
            "CW" => ModeGroup::Cw,
            "FT8" | "FT4" | "DATA" | "RTTY" | "PSK31" | "JS8" | "DIGI" => ModeGroup::Data,
            _ => ModeGroup::Other,
        }
    }

    /// The rig mode to use when tuning to this spot.
    pub fn rig_mode(&self) -> Option<Mode> {
        match self.mode.to_ascii_uppercase().as_str() {
            "LSB" => Some(Mode::Lsb),
            "USB" => Some(Mode::Usb),
            "SSB" | "PHONE" => Some(Mode::ssb_for(self.freq_hz)),
            "CW" => Some(Mode::Cw),
            "AM" => Some(Mode::Am),
            "FM" => Some(Mode::Fm),
            // Digital modes are worked with a USB dial.
            "FT8" | "FT4" | "DATA" | "JS8" | "PSK31" | "DIGI" => Some(Mode::Usb),
            "RTTY" => Some(Mode::Fsk),
            _ => None,
        }
    }

    pub fn band(&self) -> Option<&'static Band> {
        band_for(self.freq_hz)
    }

    /// The activator has signed off (by convention, "QRT" in the comments).
    pub fn is_qrt(&self) -> bool {
        self.comments.to_ascii_uppercase().contains("QRT")
    }

    pub fn age_secs(&self, now_unix: i64) -> i64 {
        (now_unix - self.time_unix).max(0)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSpot {
    spot_id: u64,
    #[serde(default)]
    activator: String,
    #[serde(default)]
    frequency: String,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    reference: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    park_name: Option<String>,
    #[serde(default)]
    location_desc: Option<String>,
    #[serde(default)]
    spotter: Option<String>,
    #[serde(default)]
    comments: Option<String>,
    #[serde(default)]
    spot_time: Option<String>,
    #[serde(default)]
    invalid: Option<serde_json::Value>,
}

/// Parses the API's JSON. Spots without a usable frequency are skipped.
pub fn parse_spots(json: &str) -> Result<Vec<Spot>, String> {
    let raw: Vec<RawSpot> = serde_json::from_str(json).map_err(|e| format!("unexpected spot data: {e}"))?;
    let mut spots: Vec<Spot> = raw
        .into_iter()
        .filter(|r| !matches!(r.invalid, Some(serde_json::Value::Bool(true))))
        .filter_map(|r| {
            // Frequency is in kHz, as text: "14285" or "14285.5".
            let khz: f64 = r.frequency.trim().parse().ok()?;
            if !(1.0..=60_000.0).contains(&khz) {
                return None;
            }
            Some(Spot {
                id: r.spot_id,
                activator: r.activator.trim().to_owned(),
                freq_hz: (khz * 1000.0).round() as u64,
                mode: r.mode.unwrap_or_default().trim().to_owned(),
                reference: r.reference.unwrap_or_default(),
                park_name: r.name.or(r.park_name).unwrap_or_default(),
                location: r.location_desc.unwrap_or_default(),
                spotter: r.spotter.unwrap_or_default(),
                comments: r.comments.unwrap_or_default(),
                time_unix: r.spot_time.as_deref().and_then(parse_utc).unwrap_or(0),
            })
        })
        .collect();
    spots.sort_by_key(|s| s.freq_hz);
    Ok(spots)
}

/// Parses "2026-11-22T18:20:41" (UTC, optional fraction and "Z").
pub fn parse_utc(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('Z');
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let time = time.split('.').next()?;
    let mut t = time.split(':').map(|p| p.parse::<i64>());
    let (hh, mm, ss) = (t.next()?.ok()?, t.next()?.ok()?, t.next().unwrap_or(Ok(0)).ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || hh > 23 || mm > 59 || ss > 60 {
        return None;
    }
    Some(days_from_civil(y, m, day) * 86_400 + hh * 3600 + mm * 60 + ss)
}

/// Days since 1970-01-01 (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, Default)]
pub struct FeedState {
    pub spots: Vec<Spot>,
    pub updated: Option<Instant>,
    pub error: Option<String>,
    pub fetching: bool,
}

/// Keeps a fresh list of spots, fetched in the background.
pub struct SpotFeed {
    state: Arc<Mutex<FeedState>>,
    refresh: Option<Sender<()>>,
}

fn lock(s: &Mutex<FeedState>) -> MutexGuard<'_, FeedState> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

impl SpotFeed {
    /// Polls the POTA API every `interval`.
    pub fn live(interval: Duration, notify: Notify) -> SpotFeed {
        let state = Arc::new(Mutex::new(FeedState::default()));
        let (tx, rx) = mpsc::channel();
        let s = state.clone();
        std::thread::Builder::new()
            .name("pota".into())
            .spawn(move || poll_loop(rx, s, interval, notify))
            .expect("spawn pota thread");
        SpotFeed { state, refresh: Some(tx) }
    }

    /// A fixed list (demo mode).
    pub fn fixed(spots: Vec<Spot>) -> SpotFeed {
        SpotFeed {
            state: Arc::new(Mutex::new(FeedState { spots, updated: Some(Instant::now()), ..FeedState::default() })),
            refresh: None,
        }
    }

    pub fn state(&self) -> FeedState {
        lock(&self.state).clone()
    }

    pub fn refresh_now(&self) {
        if let Some(tx) = &self.refresh {
            let _ = tx.send(());
        }
    }
}

fn poll_loop(rx: Receiver<()>, state: Arc<Mutex<FeedState>>, interval: Duration, notify: Notify) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(concat!("Seventy3/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    loop {
        lock(&state).fetching = true;
        notify();
        let result = agent
            .get(SPOTS_URL)
            .call()
            .map_err(|e| format!("POTA: {e}"))
            .and_then(|mut r| r.body_mut().read_to_string().map_err(|e| format!("POTA: {e}")))
            .and_then(|body| parse_spots(&body));
        {
            let mut s = lock(&state);
            s.fetching = false;
            match result {
                Ok(spots) => {
                    s.spots = spots;
                    s.updated = Some(Instant::now());
                    s.error = None;
                }
                Err(e) => {
                    log::warn!("{e}");
                    s.error = Some(e);
                }
            }
        }
        notify();
        match rx.recv_timeout(interval) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/pota_spots.json");

    #[test]
    fn parses_api_response() {
        let spots = parse_spots(FIXTURE).unwrap();
        // One spot has no frequency and one is flagged invalid.
        assert_eq!(spots.len(), 5);
        assert!(spots.windows(2).all(|w| w[0].freq_hz <= w[1].freq_hz));
        let s = spots.iter().find(|s| s.activator == "K4PARK").unwrap();
        assert_eq!(s.freq_hz, 14_285_500);
        assert_eq!(s.reference, "US-D101");
        assert_eq!(s.park_name, "Cedar Hollow State Park");
        assert_eq!(s.location, "US-TN");
        assert_eq!(s.mode_group(), ModeGroup::Ssb);
        assert_eq!(s.rig_mode(), Some(Mode::Usb));
        assert_eq!(s.band().unwrap().name, "20");
        assert_eq!(s.time_unix, parse_utc("2026-11-22T18:20:41").unwrap());
    }

    #[test]
    fn classifies_modes() {
        let spots = parse_spots(FIXTURE).unwrap();
        let by_call = |c: &str| spots.iter().find(|s| s.activator == c).unwrap().clone();
        assert_eq!(by_call("W7HIKE").rig_mode(), Some(Mode::Lsb)); // SSB on 40 m
        assert_eq!(by_call("N2TREE").mode_group(), ModeGroup::Cw);
        assert_eq!(by_call("K9LAKE").mode_group(), ModeGroup::Data);
        assert_eq!(by_call("K9LAKE").rig_mode(), Some(Mode::Usb));
        assert_eq!(by_call("W5MESA").mode_group(), ModeGroup::Other);
        assert!(by_call("W5MESA").is_qrt());
        assert!(!by_call("K4PARK").is_qrt());
    }

    #[test]
    fn parses_times() {
        assert_eq!(parse_utc("1970-01-01T00:00:00"), Some(0));
        assert_eq!(parse_utc("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(parse_utc("2026-11-22T18:20:41.123"), Some(1_795_371_641));
        assert_eq!(parse_utc("garbage"), None);
        assert_eq!(parse_utc("2026-13-01T00:00:00"), None);
    }

    #[test]
    fn rejects_non_json() {
        assert!(parse_spots("<html>").is_err());
    }
}
