//! Amateur bands the TS-480 covers (ITU Region 2 / US edges).

use crate::rig::protocol::Mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub name: &'static str,
    pub low_hz: u64,
    pub high_hz: u64,
    /// Where a band button goes the first time it is used.
    pub default_hz: u64,
}

pub const BANDS: [Band; 11] = [
    Band { name: "160", low_hz: 1_800_000, high_hz: 2_000_000, default_hz: 1_900_000 },
    Band { name: "80", low_hz: 3_500_000, high_hz: 4_000_000, default_hz: 3_850_000 },
    Band { name: "60", low_hz: 5_330_500, high_hz: 5_406_400, default_hz: 5_357_000 },
    Band { name: "40", low_hz: 7_000_000, high_hz: 7_300_000, default_hz: 7_200_000 },
    Band { name: "30", low_hz: 10_100_000, high_hz: 10_150_000, default_hz: 10_120_000 },
    Band { name: "20", low_hz: 14_000_000, high_hz: 14_350_000, default_hz: 14_285_000 },
    Band { name: "17", low_hz: 18_068_000, high_hz: 18_168_000, default_hz: 18_140_000 },
    Band { name: "15", low_hz: 21_000_000, high_hz: 21_450_000, default_hz: 21_300_000 },
    Band { name: "12", low_hz: 24_890_000, high_hz: 24_990_000, default_hz: 24_950_000 },
    Band { name: "10", low_hz: 28_000_000, high_hz: 29_700_000, default_hz: 28_400_000 },
    Band { name: "6", low_hz: 50_000_000, high_hz: 54_000_000, default_hz: 50_125_000 },
];

impl Band {
    pub fn contains(&self, hz: u64) -> bool {
        (self.low_hz..=self.high_hz).contains(&hz)
    }

    /// Mode used the first time the band is selected: SSB voice on the
    /// conventional sideband, except CW on 30 m where voice isn't allowed.
    pub fn default_mode(&self) -> Mode {
        if self.name == "30" { Mode::Cw } else { Mode::ssb_for(self.default_hz) }
    }
}

/// The band containing `hz`, if it is in one.
pub fn band_for(hz: u64) -> Option<&'static Band> {
    BANDS.iter().find(|b| b.contains(hz))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_bands() {
        assert_eq!(band_for(14_285_000).unwrap().name, "20");
        assert_eq!(band_for(7_074_000).unwrap().name, "40");
        assert_eq!(band_for(50_313_000).unwrap().name, "6");
        assert!(band_for(9_000_000).is_none());
        for b in &BANDS {
            assert!(b.contains(b.default_hz), "{}", b.name);
        }
        assert_eq!(band_for(10_120_000).unwrap().default_mode(), Mode::Cw);
        assert_eq!(band_for(7_200_000).unwrap().default_mode(), Mode::Lsb);
    }
}
