//! A simulated TS-480 that speaks the CAT protocol, for demo mode and tests.
//!
//! [`SimShared`] holds the simulated rig's front-panel state so other parts
//! of the demo (the synthetic IF signal) can follow its VFO, and tests can
//! "turn the knob".

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use super::Transport;
use super::protocol::{self, IfStatus, Mode, VfoSel};

pub struct SimShared {
    pub freq_hz: AtomicU64,
    pub freq_b_hz: AtomicU64,
    /// Mode code as used by `MD`.
    pub mode: AtomicU8,
    pub transmitting: AtomicBool,
    pub high_cut: AtomicU8,
    pub low_cut: AtomicU8,
    /// Test hook: answer `TX0;` with `?;` like firmware that lacks it.
    pub reject_tx0: AtomicBool,
}

impl SimShared {
    pub fn new(freq_hz: u64) -> Self {
        SimShared {
            freq_hz: AtomicU64::new(freq_hz),
            freq_b_hz: AtomicU64::new(7_074_000),
            mode: AtomicU8::new(Mode::ssb_for(freq_hz).code() as u8),
            transmitting: AtomicBool::new(false),
            high_cut: AtomicU8::new(7),
            low_cut: AtomicU8::new(4),
            reject_tx0: AtomicBool::new(false),
        }
    }

    pub fn mode(&self) -> Mode {
        Mode::from_code(self.mode.load(Ordering::Relaxed)).unwrap_or(Mode::Usb)
    }
}

pub struct SimRig {
    shared: Arc<SimShared>,
    inbox: Vec<u8>,
    outbox: VecDeque<u8>,
}

impl SimRig {
    pub fn new(shared: Arc<SimShared>) -> Self {
        SimRig { shared, inbox: Vec::new(), outbox: VecDeque::new() }
    }

    fn reply(&mut self, s: &str) {
        self.outbox.extend(s.bytes());
    }

    fn handle(&mut self, frame: &str) {
        let sh = self.shared.clone();
        let (prefix, arg) = frame.split_at(frame.len().min(2));
        match (prefix, arg) {
            ("ID", "") => self.reply("ID020;"),
            ("AI", _) => {}
            ("IF", "") => {
                let st = IfStatus {
                    freq_hz: sh.freq_hz.load(Ordering::Relaxed),
                    rit_xit_hz: 0,
                    rit_on: false,
                    xit_on: false,
                    transmitting: sh.transmitting.load(Ordering::Relaxed),
                    mode: Some(sh.mode()),
                    vfo: VfoSel::A,
                    split: false,
                };
                let s = protocol::format_if(&st);
                self.reply(&s);
            }
            ("FA", "") => {
                let s = format!("FA{:011};", sh.freq_hz.load(Ordering::Relaxed));
                self.reply(&s);
            }
            ("FA", digits) => match digits.parse::<u64>() {
                Ok(hz) if digits.len() == 11 => sh.freq_hz.store(hz, Ordering::Relaxed),
                _ => self.reply("?;"),
            },
            ("FB", "") => {
                let s = format!("FB{:011};", sh.freq_b_hz.load(Ordering::Relaxed));
                self.reply(&s);
            }
            ("MD", "") => {
                let s = format!("MD{};", sh.mode().code());
                self.reply(&s);
            }
            ("MD", code) => match code.bytes().next().and_then(Mode::from_code) {
                Some(m) if code.len() == 1 => sh.mode.store(m.code() as u8, Ordering::Relaxed),
                _ => self.reply("?;"),
            },
            ("TX", "0") if sh.reject_tx0.load(Ordering::Relaxed) => self.reply("?;"),
            ("TX", "" | "0" | "1") => sh.transmitting.store(true, Ordering::Relaxed),
            ("RX", "") => sh.transmitting.store(false, Ordering::Relaxed),
            ("SH", "") => {
                let s = format!("SH{:02};", sh.high_cut.load(Ordering::Relaxed));
                self.reply(&s);
            }
            ("SL", "") => {
                let s = format!("SL{:02};", sh.low_cut.load(Ordering::Relaxed));
                self.reply(&s);
            }
            ("PC", "") => self.reply("PC100;"),
            _ => self.reply("?;"),
        }
    }
}

impl Transport for SimRig {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.inbox.extend_from_slice(data);
        while let Some(end) = self.inbox.iter().position(|&b| b == b';') {
            let frame: Vec<u8> = self.inbox.drain(..=end).collect();
            let text = String::from_utf8_lossy(&frame[..frame.len() - 1]).into_owned();
            self.handle(&text);
        }
        Ok(())
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.outbox.is_empty() {
            // Behave like a serial port with a short read timeout.
            std::thread::sleep(Duration::from_millis(2));
            return Ok(0);
        }
        let n = buf.len().min(self.outbox.len());
        for (dst, src) in buf.iter_mut().zip(self.outbox.drain(..n)) {
            *dst = src;
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exchange(rig: &mut SimRig, out: &str) -> String {
        rig.write_all(out.as_bytes()).unwrap();
        let mut buf = [0u8; 128];
        let n = rig.read(&mut buf).unwrap();
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    #[test]
    fn answers_like_a_ts480() {
        let shared = Arc::new(SimShared::new(14_074_000));
        let mut rig = SimRig::new(shared.clone());
        assert_eq!(exchange(&mut rig, "ID;"), "ID020;");
        assert_eq!(exchange(&mut rig, "FA;"), "FA00014074000;");
        assert_eq!(exchange(&mut rig, "FA00007030000;"), "");
        assert_eq!(shared.freq_hz.load(Ordering::Relaxed), 7_030_000);
        assert_eq!(exchange(&mut rig, "MD3;"), "");
        assert_eq!(exchange(&mut rig, "MD;"), "MD3;");
        assert_eq!(exchange(&mut rig, "XX;"), "?;");
        let status = exchange(&mut rig, "IF;");
        assert_eq!(status.len(), 38);
        assert!(status.starts_with("IF00007030000"));
    }
}
