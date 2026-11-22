//! Mapping between the TS-480's first IF and the frequency on the dial.
//!
//! The TS-480 up-converts everything it receives (30 kHz – 60 MHz) to a
//! first IF of 73.095 MHz with a local oscillator *above* the signal:
//!
//! ```text
//! LO1 = dial + IF      and      IF(signal) = LO1 − RF(signal)
//! ```
//!
//! So the dial frequency always lands on the IF centre, and a signal 1 kHz
//! above the dial appears 1 kHz *below* the IF centre: the spectrum at the
//! tap is mirrored. [`IfMap`] captures both facts, plus a calibrated IF
//! centre that absorbs the RTL-SDR's crystal error and the rig's own offsets.

use serde::{Deserialize, Serialize};

/// Nominal first IF of the TS-480.
pub const TS480_FIRST_IF_HZ: f64 = 73_095_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IfMap {
    /// IF frequency, as measured by the RTL-SDR, at which the dial frequency
    /// appears. Nominally 73.095 MHz; calibration moves it.
    pub if_center_hz: f64,
    /// The IF spectrum is mirrored (high-side first LO). True for the TS-480.
    pub inverted: bool,
}

impl Default for IfMap {
    fn default() -> Self {
        IfMap { if_center_hz: TS480_FIRST_IF_HZ, inverted: true }
    }
}

impl IfMap {
    fn sign(&self) -> f64 {
        if self.inverted { -1.0 } else { 1.0 }
    }

    /// RF frequency of a signal seen at `if_hz` while the rig is tuned to `dial_hz`.
    pub fn rf_from_if(&self, dial_hz: f64, if_hz: f64) -> f64 {
        dial_hz + self.sign() * (if_hz - self.if_center_hz)
    }

    /// IF frequency at which a signal on `rf_hz` appears while tuned to `dial_hz`.
    pub fn if_from_rf(&self, dial_hz: f64, rf_hz: f64) -> f64 {
        self.if_center_hz + self.sign() * (rf_hz - dial_hz)
    }

    /// Returns the map corrected so that a carrier currently displayed at
    /// `displayed_rf_hz` is shown at its true frequency `true_rf_hz`.
    /// Independent of the dial frequency.
    pub fn calibrated(&self, displayed_rf_hz: f64, true_rf_hz: f64) -> IfMap {
        IfMap { if_center_hz: self.if_center_hz + self.sign() * (displayed_rf_hz - true_rf_hz), ..*self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dial_frequency_lands_on_if_center() {
        let m = IfMap::default();
        assert_eq!(m.if_from_rf(14_285_000.0, 14_285_000.0), TS480_FIRST_IF_HZ);
        assert_eq!(m.rf_from_if(14_285_000.0, TS480_FIRST_IF_HZ), 14_285_000.0);
    }

    #[test]
    fn spectrum_is_mirrored() {
        let m = IfMap::default();
        // A signal 1 kHz above the dial is 1 kHz below the IF centre.
        assert_eq!(m.if_from_rf(7_200_000.0, 7_201_000.0), TS480_FIRST_IF_HZ - 1000.0);
        let upright = IfMap { inverted: false, ..m };
        assert_eq!(upright.if_from_rf(7_200_000.0, 7_201_000.0), TS480_FIRST_IF_HZ + 1000.0);
    }

    #[test]
    fn round_trips() {
        for inverted in [true, false] {
            let m = IfMap { if_center_hz: 73_093_812.5, inverted };
            let rf = 21_312_345.0;
            let iff = m.if_from_rf(21_300_000.0, rf);
            assert!((m.rf_from_if(21_300_000.0, iff) - rf).abs() < 1e-6);
        }
    }

    #[test]
    fn calibration_moves_the_carrier_to_its_true_frequency() {
        for inverted in [true, false] {
            // The real IF centre is 1.7 kHz off what we assume.
            let truth = IfMap { if_center_hz: TS480_FIRST_IF_HZ + 1_700.0, inverted };
            let assumed = IfMap { if_center_hz: TS480_FIRST_IF_HZ, inverted };
            let dial = 10_000_000.0;
            let carrier = 10_000_000.0; // WWV
            let if_seen = truth.if_from_rf(dial, carrier);
            let displayed = assumed.rf_from_if(dial, if_seen);
            assert!((displayed - carrier).abs() > 1_000.0);

            let fixed = assumed.calibrated(displayed, carrier);
            assert!((fixed.if_center_hz - truth.if_center_hz).abs() < 1e-6);
            // ...and it holds on another band too.
            let other_if = truth.if_from_rf(14_100_000.0, 14_100_500.0);
            assert!((fixed.rf_from_if(14_100_000.0, other_if) - 14_100_500.0).abs() < 1e-6);
        }
    }
}
