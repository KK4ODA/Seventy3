//! Demo mode content: a made-up but plausible set of stations across the HF
//! bands, and POTA spots for some of them. All callsigns use four-letter
//! suffixes and all parks are fictional, so nothing here is a real station.

use crate::pota::Spot;

/// A tiny deterministic random number generator (xorshift64*).
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in [0, 1).
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.f64()
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

/// Mixes values into a well-spread hash, for time-slot pseudo-randomness.
pub fn hash(a: u64, b: u64) -> u64 {
    let mut x = a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 33;
    x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    x ^= x >> 33;
    x = x.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
    x ^ (x >> 33)
}

#[derive(Debug, Clone, PartialEq)]
pub enum DemoKind {
    /// SSB voice; `usb` selects the sideband.
    Voice { usb: bool },
    /// CW sending a CQ at the given speed.
    Cw { wpm: f64 },
    /// An FT8 signal (its lowest tone at `rf_hz`), transmitting in even or
    /// odd 15-second periods.
    Ft8 { odd: bool },
    /// A steady carrier.
    Carrier,
}

#[derive(Debug, Clone)]
pub struct DemoPark {
    pub reference: String,
    pub name: String,
    pub location: String,
}

#[derive(Debug, Clone)]
pub struct DemoStation {
    /// Dial (suppressed carrier) frequency for voice; carrier for CW and FT8.
    pub rf_hz: f64,
    pub kind: DemoKind,
    /// Peak level, dB relative to the RTL-SDR's full scale at the default gain.
    pub level_db: f32,
    pub call: String,
    pub park: Option<DemoPark>,
    pub seed: u64,
}

const SUFFIXES: [&str; 30] = [
    "PARK", "HIKE", "TREE", "LAKE", "MESA", "PINE", "FERN", "DUNE", "MOSS", "CAMP", "OAKS", "ELMS", "BEAR", "DEER",
    "HAWK", "WREN", "LYNX", "BASS", "CAVE", "PEAK", "GLEN", "DALE", "COVE", "ROCK", "SAGE", "RUSH", "REED", "TIDE",
    "VALE", "KNOB",
];

const PARKS: [(&str, &str); 16] = [
    ("Cedar Hollow State Park", "US-TN"),
    ("Twin Falls Recreation Area", "US-ID"),
    ("Bluebird Ridge State Forest", "US-NY"),
    ("Lantern Lake State Park", "US-WI"),
    ("Copper Creek Wildlife Area", "US-CO"),
    ("Whispering Pines State Park", "US-NC"),
    ("Eagle Bluff National Forest", "US-MO"),
    ("Silver Fork State Natural Area", "US-TX"),
    ("Painted Rock State Park", "US-NM"),
    ("Heron Marsh Wildlife Refuge", "US-LA"),
    ("Granite Gap Historic Site", "US-PA"),
    ("Maple Run State Park", "US-OH"),
    ("Kestrel Point State Park", "US-OR"),
    ("Foxglove Meadow Preserve", "US-VT"),
    ("Thunder Canyon State Park", "US-AZ"),
    ("Juniper Flats Recreation Area", "US-NV"),
];

/// The demo's stations, the same on every run.
pub fn stations() -> Vec<DemoStation> {
    let mut rng = Rng::new(73);
    let mut out = Vec::new();
    let mut call_n = 0usize;
    let mut next_call = |rng: &mut Rng| {
        let prefix = ["K", "N", "W"][rng.below(3)];
        let digit = rng.below(10);
        let s = SUFFIXES[call_n % SUFFIXES.len()];
        call_n += 1;
        format!("{prefix}{digit}{s}")
    };

    // SSB voice: (low, high, upper sideband, how many)
    let voice = [
        (1_843_000.0, 1_990_000.0, false, 4),
        (3_600_000.0, 3_990_000.0, false, 10),
        (7_125_000.0, 7_295_000.0, false, 14),
        (14_150_000.0, 14_345_000.0, true, 18),
        (18_110_000.0, 18_165_000.0, true, 6),
        (21_200_000.0, 21_445_000.0, true, 10),
        (24_931_000.0, 24_988_000.0, true, 4),
        (28_300_000.0, 28_600_000.0, true, 8),
        (50_125_000.0, 50_200_000.0, true, 3),
    ];
    for (lo, hi, usb, count) in voice {
        let mut placed: Vec<f64> = Vec::new();
        let mut tries = 0;
        while placed.len() < count && tries < 500 {
            tries += 1;
            // Voice stations mostly sit on round kHz, sometimes on half kHz.
            let step = if rng.f64() < 0.8 { 1000.0 } else { 500.0 };
            let f = (rng.range(lo, hi) / step).round() * step;
            if placed.iter().any(|p| (p - f).abs() < 4000.0) {
                continue;
            }
            placed.push(f);
            out.push(DemoStation {
                rf_hz: f,
                kind: DemoKind::Voice { usb },
                level_db: rng.range(-62.0, -18.0) as f32,
                call: next_call(&mut rng),
                park: None,
                seed: rng.next_u64(),
            });
        }
    }

    // CW near the bottom of each band.
    for (lo, count) in [(3_520_000.0, 5), (7_010_000.0, 8), (10_105_000.0, 4), (14_010_000.0, 9), (21_015_000.0, 5)] {
        for _ in 0..count {
            out.push(DemoStation {
                rf_hz: (rng.range(lo, lo + 50_000.0) / 100.0).round() * 100.0,
                kind: DemoKind::Cw { wpm: rng.range(15.0, 28.0) },
                level_db: rng.range(-70.0, -30.0) as f32,
                call: next_call(&mut rng),
                park: None,
                seed: rng.next_u64(),
            });
        }
    }

    // FT8: signals between 200 and 2900 Hz above each band's dial frequency.
    for dial in [3_573_000.0, 7_074_000.0, 10_136_000.0, 14_074_000.0, 18_100_000.0, 21_074_000.0, 28_074_000.0] {
        for _ in 0..12 {
            out.push(DemoStation {
                rf_hz: dial + (rng.range(200.0, 2900.0) / 10.0).round() * 10.0,
                kind: DemoKind::Ft8 { odd: rng.f64() < 0.5 },
                level_db: rng.range(-72.0, -40.0) as f32,
                call: next_call(&mut rng),
                park: None,
                seed: rng.next_u64(),
            });
        }
    }

    // Steady carriers: a time-signal station and a beacon, handy for calibration.
    for (f, db) in [(5_000_000.0, -35.0), (10_000_000.0, -30.0), (15_000_000.0, -38.0), (14_100_000.0, -55.0)] {
        out.push(DemoStation {
            rf_hz: f,
            kind: DemoKind::Carrier,
            level_db: db,
            call: String::new(),
            park: None,
            seed: rng.next_u64(),
        });
    }

    // Make a share of the voice and CW stations POTA activators.
    let mut park_n = 0;
    for st in out.iter_mut() {
        let eligible = matches!(st.kind, DemoKind::Voice { .. } | DemoKind::Cw { .. });
        if eligible && rng.f64() < 0.4 {
            let (name, location) = PARKS[park_n % PARKS.len()];
            st.park = Some(DemoPark {
                reference: format!("US-D{:03}", 101 + park_n),
                name: name.to_owned(),
                location: location.to_owned(),
            });
            park_n += 1;
        }
    }
    out
}

/// POTA spots for the demo's activators, timed relative to `now_unix`.
pub fn spots(now_unix: i64) -> Vec<Spot> {
    let mut rng = Rng::new(4);
    let mut spots: Vec<Spot> = stations()
        .into_iter()
        .filter_map(|st| {
            let park = st.park?;
            let (mode, freq_hz) = match st.kind {
                DemoKind::Voice { .. } => ("SSB", st.rf_hz as u64),
                DemoKind::Cw { .. } => ("CW", st.rf_hz as u64),
                _ => return None,
            };
            let qrt = rng.f64() < 0.08;
            let comments = if qrt {
                "QRT, thanks for the contacts".to_owned()
            } else {
                ["", "59 here", "Loud in the park", "Thanks for the P2P", "QSY from 20m"][rng.below(5)].to_owned()
            };
            Some(Spot {
                id: st.seed,
                activator: st.call.clone(),
                freq_hz,
                mode: mode.into(),
                reference: park.reference,
                park_name: park.name,
                location: park.location,
                spotter: if rng.f64() < 0.5 { st.call.clone() } else { format!("K{}SPOT", rng.below(10)) },
                comments,
                time_unix: now_unix - rng.range(10.0, 1500.0) as i64,
            })
        })
        .collect();
    spots.sort_by_key(|s| s.freq_hz);
    spots
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stations_are_deterministic_and_in_band() {
        let a = stations();
        let b = stations();
        assert_eq!(a.len(), b.len());
        assert!(a.len() > 100);
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.rf_hz, y.rf_hz);
            assert_eq!(x.call, y.call);
        }
        for st in a.iter().filter(|s| matches!(s.kind, DemoKind::Voice { .. })) {
            assert!(crate::bands::band_for(st.rf_hz as u64).is_some(), "{}", st.rf_hz);
        }
    }

    #[test]
    fn demo_spots_exist_on_20m_ssb() {
        let spots = spots(1_800_000_000);
        assert!(spots.iter().any(|s| s.band().map(|b| b.name) == Some("20") && s.mode == "SSB"));
        for s in &spots {
            // Four-letter suffix: never a real US callsign.
            let suffix: String = s.activator.chars().skip_while(|c| !c.is_ascii_digit()).skip(1).collect();
            assert_eq!(suffix.len(), 4, "{}", s.activator);
        }
    }
}
