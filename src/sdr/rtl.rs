//! The RTL-SDR dongle on the TS-480's first IF.
//!
//! On Windows the dongle needs the WinUSB driver (install it once with Zadig);
//! rtl-sdr-rs then talks to it directly, so there is no DLL to ship.

use rtl_sdr_rs::{AsyncReadControlHandle, AsyncReadEvent, AsyncReadHandle, RtlSdr, TunerGain};

use super::{IqSource, ReadStats, SdrSettings};
use crate::dsp::Complex32;

pub struct RtlSource {
    reader: AsyncReadHandle,
    control: AsyncReadControlHandle,
    description: String,
    gains: Vec<i32>,
}

fn gain(tenths_db: Option<i32>) -> TunerGain {
    match tenths_db {
        Some(g) => TunerGain::Manual(g),
        None => TunerGain::Auto,
    }
}

impl RtlSource {
    pub fn open(settings: &SdrSettings, tuner_hz: u32) -> Result<RtlSource, String> {
        let explain = |e: rtl_sdr_rs::error::RtlsdrError| {
            let mut msg = format!("RTL-SDR: {e}");
            if cfg!(windows) {
                msg.push_str(". If it is plugged in, install the WinUSB driver for it with Zadig.");
            }
            msg
        };
        let mut sdr = RtlSdr::open_with_index(settings.device_index).map_err(explain)?;
        sdr.set_sample_rate(settings.sample_rate).map_err(explain)?;
        sdr.set_center_freq(tuner_hz).map_err(explain)?;
        let gains = sdr.get_tuner_gains().unwrap_or_default();
        // Snap a manual gain to one the tuner supports.
        let wanted = settings.gain_tenths_db.map(|g| gains.iter().copied().min_by_key(|x| (x - g).abs()).unwrap_or(g));
        sdr.set_tuner_gain(gain(wanted)).map_err(explain)?;
        sdr.reset_buffer().map_err(explain)?;
        let tuner = sdr.get_tuner_id().map(str::to_owned).unwrap_or_else(|_| "unknown tuner".into());
        let reader = sdr.into_async_reader(0, 0).map_err(explain)?;
        let control = reader.control_handle();
        Ok(RtlSource { reader, control, description: format!("RTL-SDR ({tuner})"), gains })
    }
}

impl IqSource for RtlSource {
    fn describe(&self) -> String {
        self.description.clone()
    }

    fn gains_tenths_db(&self) -> Vec<i32> {
        self.gains.clone()
    }

    fn read(&mut self, out: &mut Vec<Complex32>) -> Result<ReadStats, String> {
        loop {
            match self.reader.recv() {
                Some(Ok(AsyncReadEvent::Samples { data, .. })) => {
                    out.clear();
                    out.reserve(data.len() / 2);
                    let mut clipped = 0;
                    let (pairs, _) = data.as_chunks::<2>();
                    for &[i, q] in pairs {
                        if i == 0 || i == 255 || q == 0 || q == 255 {
                            clipped += 1;
                        }
                        out.push(Complex32::new((f32::from(i) - 127.4) / 128.0, (f32::from(q) - 127.4) / 128.0));
                    }
                    return Ok(ReadStats { clipped });
                }
                Some(Ok(_)) => continue,
                Some(Err(e)) => return Err(format!("RTL-SDR stopped: {e}")),
                None => return Err("RTL-SDR stream ended (unplugged?)".into()),
            }
        }
    }

    fn tune(&mut self, hz: u32) -> Result<(), String> {
        self.control.set_center_freq(hz).map(|_| ()).map_err(|e| format!("RTL-SDR retune: {e}"))
    }

    fn set_gain(&mut self, tenths_db: Option<i32>) -> Result<(), String> {
        let snapped = tenths_db.map(|g| self.gains.iter().copied().min_by_key(|x| (x - g).abs()).unwrap_or(g));
        self.control.set_tuner_gain(gain(snapped)).map(|_| ()).map_err(|e| format!("RTL-SDR gain: {e}"))
    }
}

impl Drop for RtlSource {
    fn drop(&mut self) {
        self.reader.stop();
    }
}
