//! The subset of the Kenwood TS-480 PC-control ("CAT") command set that
//! Seventy3 uses.
//!
//! Every command and answer is plain ASCII terminated by `;`. Set commands
//! (`FA00014250000;`) get no answer; read commands (`FA;`) are answered with
//! the same two-letter prefix followed by the value. The rig answers `?;`
//! to a command it could not execute (wrong syntax, or not allowed in the
//! current state), `E;` to a communication error and `O;` to an overflow.

use serde::{Deserialize, Serialize};

/// Identifier the TS-480 returns to `ID;` (`ID020;`).
pub const TS480_ID: u16 = 20;

/// Lowest and highest frequencies the TS-480 accepts on its VFOs.
pub const MIN_FREQ_HZ: u64 = 30_000;
pub const MAX_FREQ_HZ: u64 = 60_000_000;

/// Operating mode, as used by the `MD` command and the `IF` answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Mode {
    Lsb,
    Usb,
    Cw,
    Fm,
    Am,
    Fsk,
    CwR,
    FskR,
}

impl Mode {
    /// Modes offered as buttons in the UI, in display order.
    pub const BUTTONS: [Mode; 5] = [Mode::Lsb, Mode::Usb, Mode::Cw, Mode::Am, Mode::Fm];

    pub fn from_code(code: u8) -> Option<Mode> {
        Some(match code {
            b'1' => Mode::Lsb,
            b'2' => Mode::Usb,
            b'3' => Mode::Cw,
            b'4' => Mode::Fm,
            b'5' => Mode::Am,
            b'6' => Mode::Fsk,
            b'7' => Mode::CwR,
            b'9' => Mode::FskR,
            _ => return None,
        })
    }

    pub fn code(self) -> char {
        match self {
            Mode::Lsb => '1',
            Mode::Usb => '2',
            Mode::Cw => '3',
            Mode::Fm => '4',
            Mode::Am => '5',
            Mode::Fsk => '6',
            Mode::CwR => '7',
            Mode::FskR => '9',
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Lsb => "LSB",
            Mode::Usb => "USB",
            Mode::Cw => "CW",
            Mode::Fm => "FM",
            Mode::Am => "AM",
            Mode::Fsk => "FSK",
            Mode::CwR => "CW-R",
            Mode::FskR => "FSK-R",
        }
    }

    pub fn is_ssb(self) -> bool {
        matches!(self, Mode::Lsb | Mode::Usb)
    }

    /// The conventional sideband for SSB voice on a frequency: LSB below
    /// 10 MHz, USB above (60 m is the exception and uses USB).
    pub fn ssb_for(freq_hz: u64) -> Mode {
        if freq_hz < 10_000_000 && !(5_250_000..5_450_000).contains(&freq_hz) { Mode::Lsb } else { Mode::Usb }
    }
}

/// Which VFO or memory the rig is receiving on (`IF` answer, P10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VfoSel {
    A,
    B,
    Memory,
}

/// The rig's combined status, answered to `IF;`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IfStatus {
    /// Frequency of the receive VFO, in Hz.
    pub freq_hz: u64,
    /// RIT/XIT offset in Hz (applies only when RIT or XIT is on).
    pub rit_xit_hz: i32,
    pub rit_on: bool,
    pub xit_on: bool,
    pub transmitting: bool,
    pub mode: Option<Mode>,
    pub vfo: VfoSel,
    pub split: bool,
}

/// A parsed answer (or unsolicited message) from the rig.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    Id(u16),
    FreqA(u64),
    FreqB(u64),
    Mode(Mode),
    Status(IfStatus),
    /// DSP filter high-cut index (`SH`).
    HighCut(u8),
    /// DSP filter low-cut index (`SL`).
    LowCut(u8),
    /// Transmit power setting in watts (`PC`).
    Power(u16),
    /// `?;` — the last command was not accepted.
    Rejected,
    /// `E;` or `O;` — the rig reported a communication error or overflow.
    CommError,
    /// Anything else: a valid frame Seventy3 does not use.
    Other(String),
}

/// Parses one frame, without its trailing `;`.
pub fn parse(frame: &str) -> Answer {
    let frame = frame.trim_matches(|c: char| c.is_ascii_whitespace() && c != ' ');
    match frame {
        "?" => return Answer::Rejected,
        "E" | "O" => return Answer::CommError,
        _ => {}
    }
    let (prefix, body) = match (frame.get(..2), frame.get(2..)) {
        (Some(p), Some(b)) => (p, b),
        _ => return Answer::Other(frame.to_owned()),
    };
    let parsed = match prefix {
        "ID" => body.parse().ok().map(Answer::Id),
        "FA" => parse_freq(body).map(Answer::FreqA),
        "FB" => parse_freq(body).map(Answer::FreqB),
        "MD" => body.bytes().next().and_then(Mode::from_code).map(Answer::Mode),
        "IF" => parse_if(body).map(Answer::Status),
        "SH" => body.parse().ok().map(Answer::HighCut),
        "SL" => body.parse().ok().map(Answer::LowCut),
        "PC" => body.parse().ok().map(Answer::Power),
        _ => None,
    };
    parsed.unwrap_or_else(|| Answer::Other(frame.to_owned()))
}

fn parse_freq(digits: &str) -> Option<u64> {
    if digits.len() != 11 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Parses the 35-character body of an `IF` answer:
///
/// ```text
/// IF 00014250000 _____ +0000 0 0 0 00 0 2 0 0 0 0 00 0 ;
///    P1 freq     P2    P3    P4P5P6 P7 P8P9...
/// ```
///
/// | field | chars | meaning                       |
/// |-------|-------|-------------------------------|
/// | P1    | 0..11 | frequency, Hz                 |
/// | P2    | 11..16| (unused on the TS-480)        |
/// | P3    | 16..21| RIT/XIT offset, `±dddd` Hz    |
/// | P4    | 21    | RIT on                        |
/// | P5    | 22    | XIT on                        |
/// | P6,P7 | 23..26| memory channel                |
/// | P8    | 26    | 0 receive, 1 transmit         |
/// | P9    | 27    | mode (as `MD`)                |
/// | P10   | 28    | 0 VFO A, 1 VFO B, 2 memory    |
/// | P11   | 29    | scan                          |
/// | P12   | 30    | split                         |
/// | P13-15| 31..35| tone settings                 |
fn parse_if(body: &str) -> Option<IfStatus> {
    let b = body.as_bytes();
    if b.len() != 35 {
        return None;
    }
    let freq_hz = parse_freq(body.get(0..11)?)?;
    let rit_xit_hz = body.get(16..21).and_then(|s| s.trim().parse::<i32>().ok()).unwrap_or(0);
    let flag = |i: usize| b[i] == b'1';
    let vfo = match b[28] {
        b'1' => VfoSel::B,
        b'2' => VfoSel::Memory,
        _ => VfoSel::A,
    };
    Some(IfStatus {
        freq_hz,
        rit_xit_hz,
        rit_on: flag(21),
        xit_on: flag(22),
        transmitting: flag(26),
        mode: Mode::from_code(b[27]),
        vfo,
        split: flag(30),
    })
}

/// Formats an `IF` answer body the way the TS-480 does. Used by the
/// simulator and the tests.
pub fn format_if(s: &IfStatus) -> String {
    let vfo = match s.vfo {
        VfoSel::A => '0',
        VfoSel::B => '1',
        VfoSel::Memory => '2',
    };
    let bit = |v: bool| if v { '1' } else { '0' };
    format!(
        "IF{:011}     {:+05}{}{}000{}{}{}0{}0000;",
        s.freq_hz,
        s.rit_xit_hz,
        bit(s.rit_on),
        bit(s.xit_on),
        bit(s.transmitting),
        s.mode.map_or('0', Mode::code),
        vfo,
        bit(s.split),
    )
}

/// Command strings sent to the rig.
pub mod cmd {
    use super::{MAX_FREQ_HZ, MIN_FREQ_HZ, Mode};

    pub const READ_ID: &str = "ID;";
    pub const READ_STATUS: &str = "IF;";
    pub const READ_FREQ_B: &str = "FB;";
    pub const READ_HIGH_CUT: &str = "SH;";
    pub const READ_LOW_CUT: &str = "SL;";
    pub const READ_POWER: &str = "PC;";
    /// Turns auto-information off, so the rig only speaks when asked.
    pub const AUTO_INFO_OFF: &str = "AI0;";
    /// Transmit, taking audio from the microphone connector.
    pub const TX_MIC: &str = "TX0;";
    /// Transmit with the rig's default source (used if `TX0;` is rejected).
    pub const TX_PLAIN: &str = "TX;";
    pub const RX: &str = "RX;";

    /// Sets VFO A. The frequency is clamped to the rig's tuning range.
    pub fn set_freq_a(hz: u64) -> String {
        format!("FA{:011};", hz.clamp(MIN_FREQ_HZ, MAX_FREQ_HZ))
    }

    pub fn set_mode(mode: Mode) -> String {
        format!("MD{};", mode.code())
    }
}

/// The DSP filter passband, relative to the dial frequency, in Hz. For USB
/// the passband is `dial + low ..= dial + high`; for LSB it is mirrored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Passband {
    pub low_hz: f64,
    pub high_hz: f64,
}

/// SSB high-cut settings selected by `SH00`..`SH11`.
const SSB_HIGH_CUT_HZ: [f64; 12] =
    [1400.0, 1600.0, 1800.0, 2000.0, 2200.0, 2400.0, 2600.0, 2800.0, 3000.0, 3400.0, 4000.0, 5000.0];
/// SSB low-cut settings selected by `SL00`..`SL11`.
const SSB_LOW_CUT_HZ: [f64; 12] = [0.0, 50.0, 100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0, 1000.0];

/// The passband to draw for a mode, as offsets from the dial frequency
/// (negative = below). For SSB it uses the rig's `SH`/`SL` settings when
/// known; other modes use the TS-480's typical default widths.
pub fn passband(mode: Mode, high_cut: Option<u8>, low_cut: Option<u8>) -> Passband {
    match mode {
        Mode::Usb | Mode::Lsb => {
            let high = high_cut.and_then(|i| SSB_HIGH_CUT_HZ.get(i as usize).copied()).unwrap_or(2600.0);
            let low = low_cut.and_then(|i| SSB_LOW_CUT_HZ.get(i as usize).copied()).unwrap_or(300.0);
            if mode == Mode::Usb {
                Passband { low_hz: low, high_hz: high }
            } else {
                Passband { low_hz: -high, high_hz: -low }
            }
        }
        Mode::Cw | Mode::CwR => Passband { low_hz: -250.0, high_hz: 250.0 },
        Mode::Fsk | Mode::FskR => Passband { low_hz: -250.0, high_hz: 250.0 },
        Mode::Am => Passband { low_hz: -3000.0, high_hz: 3000.0 },
        Mode::Fm => Passband { low_hz: -6000.0, high_hz: 6000.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_if_answer() {
        // 14.250 MHz, RIT +150 Hz on, receive, USB, VFO A, no split.
        let frame = "IF00014250000     +015010000020000000";
        assert_eq!(frame.len(), 37);
        let answer = parse(frame);
        let Answer::Status(s) = answer else {
            panic!("expected status, got {answer:?}");
        };
        assert_eq!(s.freq_hz, 14_250_000);
        assert_eq!(s.rit_xit_hz, 150);
        assert!(s.rit_on);
        assert!(!s.xit_on);
        assert!(!s.transmitting);
        assert_eq!(s.mode, Some(Mode::Usb));
        assert_eq!(s.vfo, VfoSel::A);
        assert!(!s.split);
    }

    #[test]
    fn parses_transmit_split_and_negative_rit() {
        // 7.185 MHz, RIT/XIT -50 Hz (both off), transmitting, LSB, VFO A, split.
        let frame = "IF00007185000     -005000000110010000";
        assert_eq!(frame.len(), 37);
        let Answer::Status(s) = parse(frame) else { panic!() };
        assert!(!s.rit_on && !s.xit_on);
        assert_eq!(s.freq_hz, 7_185_000);
        assert_eq!(s.rit_xit_hz, -50);
        assert!(s.transmitting);
        assert_eq!(s.mode, Some(Mode::Lsb));
        assert!(s.split);
    }

    #[test]
    fn format_and_parse_round_trip() {
        let s = IfStatus {
            freq_hz: 21_300_500,
            rit_xit_hz: -120,
            rit_on: false,
            xit_on: true,
            transmitting: true,
            mode: Some(Mode::Am),
            vfo: VfoSel::B,
            split: true,
        };
        let frame = format_if(&s);
        assert_eq!(frame.len(), 38, "{frame}");
        assert_eq!(parse(frame.trim_end_matches(';')), Answer::Status(s));
    }

    #[test]
    fn rejects_malformed_frames() {
        assert!(matches!(parse("IF0001425"), Answer::Other(_)));
        assert!(matches!(parse("FA123"), Answer::Other(_)));
        assert!(matches!(parse("FA0001425000x"), Answer::Other(_)));
        assert!(matches!(parse("MDx"), Answer::Other(_)));
        assert!(matches!(parse(""), Answer::Other(_)));
        assert!(matches!(parse("F"), Answer::Other(_)));
    }

    #[test]
    fn parses_simple_answers() {
        assert_eq!(parse("ID020"), Answer::Id(TS480_ID));
        assert_eq!(parse("FA00007074000"), Answer::FreqA(7_074_000));
        assert_eq!(parse("FB00014074000"), Answer::FreqB(14_074_000));
        assert_eq!(parse("MD1"), Answer::Mode(Mode::Lsb));
        assert_eq!(parse("SH05"), Answer::HighCut(5));
        assert_eq!(parse("SL03"), Answer::LowCut(3));
        assert_eq!(parse("PC100"), Answer::Power(100));
        assert_eq!(parse("?"), Answer::Rejected);
        assert_eq!(parse("E"), Answer::CommError);
        assert_eq!(parse("O"), Answer::CommError);
    }

    #[test]
    fn formats_commands() {
        assert_eq!(cmd::set_freq_a(14_285_000), "FA00014285000;");
        assert_eq!(cmd::set_freq_a(1), "FA00000030000;");
        assert_eq!(cmd::set_freq_a(99_000_000), "FA00060000000;");
        assert_eq!(cmd::set_mode(Mode::Usb), "MD2;");
        for m in [Mode::Lsb, Mode::Usb, Mode::Cw, Mode::Fm, Mode::Am, Mode::Fsk, Mode::CwR, Mode::FskR] {
            assert_eq!(Mode::from_code(m.code() as u8), Some(m));
        }
    }

    #[test]
    fn ssb_sideband_convention() {
        assert_eq!(Mode::ssb_for(3_850_000), Mode::Lsb);
        assert_eq!(Mode::ssb_for(7_200_000), Mode::Lsb);
        assert_eq!(Mode::ssb_for(5_357_000), Mode::Usb);
        assert_eq!(Mode::ssb_for(14_285_000), Mode::Usb);
        assert_eq!(Mode::ssb_for(50_125_000), Mode::Usb);
    }

    #[test]
    fn passband_follows_sideband_and_filter() {
        let usb = passband(Mode::Usb, Some(5), Some(4));
        assert_eq!(usb, Passband { low_hz: 300.0, high_hz: 2400.0 });
        let lsb = passband(Mode::Lsb, Some(5), Some(4));
        assert_eq!(lsb, Passband { low_hz: -2400.0, high_hz: -300.0 });
        // Out-of-range index falls back to the default.
        assert_eq!(passband(Mode::Usb, Some(40), None).high_hz, 2600.0);
    }
}
