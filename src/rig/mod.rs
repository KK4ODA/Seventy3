//! Rig control: a background thread that owns the CAT port, keeps a snapshot
//! of the rig's state up to date by polling, and applies commands from the UI.
//!
//! Transmit safety is enforced here rather than in the UI:
//! * a transmit timeout unkeys the rig after a configurable time;
//! * dropping the [`RigHandle`] (normal exit or a panic unwinding through the
//!   UI) sends `RX;` before the port is closed.

pub mod protocol;
pub mod serial;
pub mod sim;

use std::io;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use protocol::{Answer, Mode, cmd};

/// A byte stream to the rig: a serial port, or the simulator.
pub trait Transport: Send {
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
    /// Returns the bytes available now, waiting a few milliseconds at most.
    /// `Ok(0)` means nothing arrived in that time.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

/// Opens the transport. Called on the rig thread, again after each failure.
pub type Opener = Box<dyn FnMut() -> io::Result<Box<dyn Transport>> + Send>;

/// Called whenever the rig state changes, to wake the UI.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone)]
pub struct RigOptions {
    /// How often to read the rig's status.
    pub poll_interval: Duration,
    /// Unkey automatically after this long in transmit. `None` disables it.
    pub tx_timeout: Option<Duration>,
}

impl Default for RigOptions {
    fn default() -> Self {
        RigOptions { poll_interval: Duration::from_millis(100), tx_timeout: Some(Duration::from_secs(180)) }
    }
}

/// What the UI knows about the rig.
#[derive(Debug, Clone, Default)]
pub struct RigState {
    /// The port is open and the rig has answered recently.
    pub connected: bool,
    /// Value answered to `ID;` (20 for a TS-480).
    pub rig_id: Option<u16>,
    /// Receive frequency, Hz. Zero until the first status answer.
    pub freq_hz: u64,
    pub freq_b_hz: Option<u64>,
    pub mode: Option<Mode>,
    pub split: bool,
    pub rit_on: bool,
    pub xit_on: bool,
    pub rit_xit_hz: i32,
    /// The rig reports it is transmitting (from any source: our PTT, the
    /// microphone's PTT, a foot switch...).
    pub transmitting: bool,
    /// Seventy3 has keyed the rig and not yet unkeyed it.
    pub ptt_engaged: bool,
    /// When Seventy3 keyed the rig.
    pub ptt_since: Option<Instant>,
    /// The transmit timeout unkeyed the rig. Cleared by the next PTT.
    pub tx_timed_out: bool,
    pub high_cut: Option<u8>,
    pub low_cut: Option<u8>,
    pub power_w: Option<u16>,
    /// Most recent problem, for the status bar.
    pub error: Option<String>,
}

impl RigState {
    pub fn passband(&self) -> Option<protocol::Passband> {
        self.mode.map(|m| protocol::passband(m, self.high_cut, self.low_cut))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RigCommand {
    SetFreq(u64),
    SetMode(Mode),
    Ptt(bool),
    Shutdown,
}

/// Handle to the rig thread. Dropping it unkeys the rig and stops the thread.
pub struct RigHandle {
    tx: Sender<RigCommand>,
    state: Arc<Mutex<RigState>>,
    thread: Option<JoinHandle<()>>,
}

impl RigHandle {
    pub fn spawn(opener: Opener, options: RigOptions, notify: Notify) -> RigHandle {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(RigState::default()));
        let thread_state = state.clone();
        let thread = std::thread::Builder::new()
            .name("rig".into())
            .spawn(move || run(opener, rx, thread_state, options, notify))
            .expect("spawn rig thread");
        RigHandle { tx, state, thread: Some(thread) }
    }

    pub fn state(&self) -> RigState {
        lock(&self.state).clone()
    }

    pub fn send(&self, command: RigCommand) {
        // The thread only stops when told to, so a send can't fail while
        // the handle is alive.
        let _ = self.tx.send(command);
    }

    pub fn set_freq(&self, hz: u64) {
        self.send(RigCommand::SetFreq(hz));
    }

    pub fn set_mode(&self, mode: Mode) {
        self.send(RigCommand::SetMode(mode));
    }

    pub fn ptt(&self, on: bool) {
        self.send(RigCommand::Ptt(on));
    }
}

impl Drop for RigHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(RigCommand::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn lock(state: &Mutex<RigState>) -> MutexGuard<'_, RigState> {
    // A poisoned lock only means another thread panicked mid-update; the
    // snapshot is still usable.
    state.lock().unwrap_or_else(|e| e.into_inner())
}

const RECONNECT_DELAY: Duration = Duration::from_secs(3);
/// A read command with no answer after this long is abandoned.
const ANSWER_TIMEOUT: Duration = Duration::from_millis(500);
/// How long polling pauses after `TX0;` to hear whether it was rejected.
const TX0_VERDICT_WAIT: Duration = Duration::from_millis(300);
/// With no answers at all for this long, the rig is shown as disconnected.
const SILENCE_TIMEOUT: Duration = Duration::from_secs(3);

enum Exit {
    Shutdown,
    Lost(io::Error),
}

fn run(mut opener: Opener, rx: Receiver<RigCommand>, state: Arc<Mutex<RigState>>, options: RigOptions, notify: Notify) {
    loop {
        match opener() {
            Ok(transport) => {
                lock(&state).error = None;
                notify();
                let mut session = Session::new(transport, &state, &options, &notify);
                match session.run(&rx) {
                    Exit::Shutdown => return,
                    Exit::Lost(e) => {
                        log::warn!("CAT port lost: {e}");
                        let mut s = lock(&state);
                        s.connected = false;
                        s.ptt_engaged = false;
                        s.error = Some(format!("CAT port: {e}"));
                    }
                }
            }
            Err(e) => {
                let mut s = lock(&state);
                s.connected = false;
                s.error = Some(format!("Can't open CAT port: {e}"));
            }
        }
        notify();
        // Wait before retrying, but stay responsive to shutdown.
        let deadline = Instant::now() + RECONNECT_DELAY;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match rx.recv_timeout(left) {
                Ok(RigCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                // PTT and tuning requests are meaningless with no rig; drop them.
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => break,
            }
        }
    }
}

/// Read commands polled in rotation. `IF` carries most of what we need.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Query {
    Id,
    Status,
    FreqB,
    HighCut,
    LowCut,
    Power,
}

impl Query {
    fn command(self) -> &'static str {
        match self {
            Query::Id => cmd::READ_ID,
            Query::Status => cmd::READ_STATUS,
            Query::FreqB => cmd::READ_FREQ_B,
            Query::HighCut => cmd::READ_HIGH_CUT,
            Query::LowCut => cmd::READ_LOW_CUT,
            Query::Power => cmd::READ_POWER,
        }
    }
}

struct Session<'a> {
    port: Box<dyn Transport>,
    state: &'a Mutex<RigState>,
    options: &'a RigOptions,
    notify: &'a Notify,
    rx_buf: Vec<u8>,
    /// The read command awaiting an answer, and when it was sent.
    pending: Option<(Query, Instant)>,
    next_poll: Instant,
    poll_count: u64,
    last_heard: Option<Instant>,
    /// When the frequency/mode was last set by us. Status answers to queries
    /// sent before then are stale for those fields.
    last_freq_set: Option<Instant>,
    last_mode_set: Option<Instant>,
    /// The last command written, for attributing a `?;`.
    last_written: Option<(String, Instant)>,
    /// `TX0;` was rejected; key with plain `TX;` instead.
    plain_tx: bool,
    /// `TX0;` was just sent and polling is held off briefly, so that a `?;`
    /// arriving now can only be the rig's verdict on it.
    tx0_probe: Option<Instant>,
    ptt: bool,
    ptt_since: Option<Instant>,
}

impl<'a> Session<'a> {
    fn new(port: Box<dyn Transport>, state: &'a Mutex<RigState>, options: &'a RigOptions, notify: &'a Notify) -> Self {
        Session {
            port,
            state,
            options,
            notify,
            rx_buf: Vec::with_capacity(256),
            pending: None,
            next_poll: Instant::now(),
            poll_count: 0,
            last_heard: None,
            last_freq_set: None,
            last_mode_set: None,
            last_written: None,
            plain_tx: false,
            tx0_probe: None,
            ptt: false,
            ptt_since: None,
        }
    }

    fn run(&mut self, rx: &Receiver<RigCommand>) -> Exit {
        // Quiet the rig's unsolicited reports, then identify it.
        if let Err(e) = self.write(cmd::AUTO_INFO_OFF) {
            return Exit::Lost(e);
        }
        if let Err(e) = self.query(Query::Id) {
            return Exit::Lost(e);
        }
        let mut buf = [0u8; 256];
        loop {
            // 1. Commands from the UI. Consecutive tuning requests are
            //    coalesced so a fast-spinning wheel can't back up the port.
            let mut freq = None;
            let mut mode = None;
            let mut ptt = None;
            loop {
                match rx.try_recv() {
                    Ok(RigCommand::SetFreq(hz)) => freq = Some(hz),
                    Ok(RigCommand::SetMode(m)) => mode = Some(m),
                    Ok(RigCommand::Ptt(on)) => ptt = Some(on),
                    Ok(RigCommand::Shutdown) | Err(TryRecvError::Disconnected) => {
                        self.unkey_for_exit();
                        return Exit::Shutdown;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            let result = self
                .apply_ptt(ptt)
                .and_then(|_| self.apply_mode(mode))
                .and_then(|_| self.apply_freq(freq))
                .and_then(|_| self.check_tx_timeout())
                .and_then(|_| self.poll());
            if let Err(e) = result {
                return Exit::Lost(e);
            }

            // 2. Whatever the rig sent.
            match self.port.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => self.receive(&buf[..n]),
                Err(e) => return Exit::Lost(e),
            }

            // 3. Notice a rig that stopped answering (powered off, cable out).
            let silent = self.last_heard.is_none_or(|t| t.elapsed() > SILENCE_TIMEOUT);
            let mut s = lock(self.state);
            if silent && s.connected {
                s.connected = false;
                s.error = Some("Rig not answering".into());
                drop(s);
                (self.notify)();
            } else if silent && s.error.is_none() {
                s.error = Some("Waiting for the rig to answer".into());
            }
        }
    }

    fn write(&mut self, command: &str) -> io::Result<()> {
        log::trace!("CAT > {command}");
        self.port.write_all(command.as_bytes())?;
        self.last_written = Some((command.to_owned(), Instant::now()));
        Ok(())
    }

    fn query(&mut self, q: Query) -> io::Result<()> {
        self.write(q.command())?;
        self.pending = Some((q, Instant::now()));
        Ok(())
    }

    fn apply_freq(&mut self, freq: Option<u64>) -> io::Result<()> {
        let Some(hz) = freq else { return Ok(()) };
        let hz = hz.clamp(protocol::MIN_FREQ_HZ, protocol::MAX_FREQ_HZ);
        self.write(&cmd::set_freq_a(hz))?;
        self.last_freq_set = Some(Instant::now());
        lock(self.state).freq_hz = hz;
        (self.notify)();
        // Confirm promptly.
        self.next_poll = Instant::now();
        Ok(())
    }

    fn apply_mode(&mut self, mode: Option<Mode>) -> io::Result<()> {
        let Some(m) = mode else { return Ok(()) };
        self.write(&cmd::set_mode(m))?;
        self.last_mode_set = Some(Instant::now());
        lock(self.state).mode = Some(m);
        (self.notify)();
        self.next_poll = Instant::now();
        Ok(())
    }

    fn apply_ptt(&mut self, ptt: Option<bool>) -> io::Result<()> {
        let Some(on) = ptt else { return Ok(()) };
        if on {
            if self.plain_tx {
                self.write(cmd::TX_PLAIN)?;
            } else {
                self.write(cmd::TX_MIC)?;
                self.tx0_probe = Some(Instant::now());
            }
            self.ptt = true;
            self.ptt_since = Some(Instant::now());
        } else {
            self.write(cmd::RX)?;
            self.ptt = false;
            self.ptt_since = None;
        }
        let mut s = lock(self.state);
        s.ptt_engaged = self.ptt;
        s.ptt_since = self.ptt_since;
        if on {
            s.tx_timed_out = false;
        }
        drop(s);
        (self.notify)();
        self.next_poll = Instant::now();
        Ok(())
    }

    fn check_tx_timeout(&mut self) -> io::Result<()> {
        let (Some(limit), Some(since)) = (self.options.tx_timeout, self.ptt_since) else {
            return Ok(());
        };
        if since.elapsed() < limit {
            return Ok(());
        }
        log::warn!("Transmit timeout after {limit:?}; unkeying");
        self.apply_ptt(Some(false))?;
        lock(self.state).tx_timed_out = true;
        (self.notify)();
        Ok(())
    }

    fn poll(&mut self) -> io::Result<()> {
        if let Some(sent) = self.tx0_probe {
            if sent.elapsed() < TX0_VERDICT_WAIT {
                return Ok(());
            }
            self.tx0_probe = None;
        }
        if let Some((_, sent)) = self.pending {
            if sent.elapsed() < ANSWER_TIMEOUT {
                return Ok(());
            }
            self.pending = None;
        }
        let now = Instant::now();
        if now < self.next_poll {
            return Ok(());
        }
        self.next_poll = now + self.options.poll_interval;
        self.poll_count += 1;
        let ssb = lock(self.state).mode.is_some_and(Mode::is_ssb);
        // IF every time; the slower-changing settings now and then.
        let q = match self.poll_count % 20 {
            5 if ssb => Query::HighCut,
            10 if ssb => Query::LowCut,
            15 => Query::FreqB,
            0 if self.poll_count.is_multiple_of(100) => Query::Power,
            _ => Query::Status,
        };
        self.query(q)
    }

    /// Splits incoming bytes into `;`-terminated frames and applies them.
    fn receive(&mut self, bytes: &[u8]) {
        self.rx_buf.extend_from_slice(bytes);
        while let Some(end) = self.rx_buf.iter().position(|&b| b == b';') {
            let frame: Vec<u8> = self.rx_buf.drain(..=end).collect();
            let text = String::from_utf8_lossy(&frame[..frame.len() - 1]).into_owned();
            log::trace!("CAT < {text};");
            self.apply_answer(protocol::parse(&text));
        }
        // A rig that never sends ';' (wrong baud rate) mustn't grow this forever.
        if self.rx_buf.len() > 1024 {
            self.rx_buf.clear();
        }
    }

    fn apply_answer(&mut self, answer: Answer) {
        let query = self.pending.take();
        if matches!(answer, Answer::CommError | Answer::Other(_)) {
            // Not an answer to our query; keep waiting for it.
            self.pending = query;
        }
        if matches!(answer, Answer::Other(_)) {
            // Usually line noise from a wrong baud rate; say so if nothing
            // valid has been heard yet.
            if self.last_heard.is_none() {
                lock(self.state).error = Some("Unrecognized data from the rig. Check the baud rate.".into());
            }
            return;
        }
        let now = Instant::now();
        self.last_heard = Some(now);
        let query_sent = query.map(|(_, t)| t);
        let stale = |set: Option<Instant>| match (set, query_sent) {
            (Some(set), Some(sent)) => sent < set,
            (Some(set), None) => now.duration_since(set) < ANSWER_TIMEOUT,
            _ => false,
        };
        let freq_stale = stale(self.last_freq_set);
        let mode_stale = stale(self.last_mode_set);

        let mut s = lock(self.state);
        let was_connected = s.connected;
        s.connected = true;
        if was_connected != s.connected || s.error.is_some() {
            s.error = None;
        }
        match answer {
            Answer::Id(id) => {
                s.rig_id = Some(id);
                if id != protocol::TS480_ID {
                    log::warn!("Rig identifies as ID{id:03}, not a TS-480 (ID020)");
                }
            }
            Answer::Status(st) => {
                if !freq_stale {
                    s.freq_hz = st.freq_hz;
                }
                if !mode_stale {
                    s.mode = st.mode.or(s.mode);
                }
                s.split = st.split;
                s.rit_on = st.rit_on;
                s.xit_on = st.xit_on;
                s.rit_xit_hz = st.rit_xit_hz;
                s.transmitting = st.transmitting;
            }
            Answer::FreqA(hz) => {
                if !freq_stale {
                    s.freq_hz = hz;
                }
            }
            Answer::FreqB(hz) => s.freq_b_hz = Some(hz),
            Answer::Mode(m) => {
                if !mode_stale {
                    s.mode = Some(m);
                }
            }
            Answer::HighCut(i) => s.high_cut = Some(i),
            Answer::LowCut(i) => s.low_cut = Some(i),
            Answer::Power(w) => s.power_w = Some(w),
            Answer::Rejected => {
                let last = self.last_written.as_ref().map(|(c, _)| c.as_str());
                if self.tx0_probe.take().is_some() {
                    // Some firmware only knows plain TX;. Retry with it.
                    log::info!("TX0; rejected, falling back to TX;");
                    self.plain_tx = true;
                    drop(s);
                    if self.ptt {
                        let _ = self.write(cmd::TX_PLAIN);
                    }
                    (self.notify)();
                    return;
                }
                log::debug!("Rig rejected {last:?}");
            }
            Answer::CommError | Answer::Other(_) => {}
        }
        drop(s);
        (self.notify)();
    }

    /// Best effort: never leave the rig keyed when Seventy3 stops.
    fn unkey_for_exit(&mut self) {
        let keyed = self.ptt || lock(self.state).ptt_engaged;
        if keyed {
            log::info!("Unkeying before exit");
            let _ = self.write(cmd::RX);
            let mut s = lock(self.state);
            s.ptt_engaged = false;
            s.ptt_since = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sim::{SimRig, SimShared};
    use super::*;
    use std::sync::atomic::Ordering;

    fn sim_handle(options: RigOptions) -> (RigHandle, Arc<SimShared>) {
        let shared = Arc::new(SimShared::new(14_285_000));
        let s = shared.clone();
        let opener: Opener = Box::new(move || Ok(Box::new(SimRig::new(s.clone())) as Box<dyn Transport>));
        (RigHandle::spawn(opener, options, Arc::new(|| {})), shared)
    }

    fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn fast() -> RigOptions {
        RigOptions { poll_interval: Duration::from_millis(10), tx_timeout: None }
    }

    #[test]
    fn connects_and_reads_status() {
        let (rig, _shared) = sim_handle(fast());
        wait_for("status", || {
            let s = rig.state();
            s.connected && s.freq_hz == 14_285_000 && s.rig_id == Some(protocol::TS480_ID)
        });
        assert_eq!(rig.state().mode, Some(Mode::Usb));
    }

    #[test]
    fn tunes_and_changes_mode() {
        let (rig, shared) = sim_handle(fast());
        wait_for("connect", || rig.state().connected);
        rig.set_freq(7_185_000);
        rig.set_mode(Mode::Lsb);
        // Optimistic update is immediate-ish; the rig itself must follow.
        wait_for("rig retuned", || shared.freq_hz.load(Ordering::Relaxed) == 7_185_000);
        wait_for("mode", || rig.state().mode == Some(Mode::Lsb));
        assert_eq!(rig.state().freq_hz, 7_185_000);
    }

    #[test]
    fn rig_side_tuning_is_reflected() {
        let (rig, shared) = sim_handle(fast());
        wait_for("connect", || rig.state().connected);
        // Someone turns the big knob.
        shared.freq_hz.store(14_300_000, Ordering::Relaxed);
        wait_for("knob", || rig.state().freq_hz == 14_300_000);
    }

    #[test]
    fn ptt_keys_and_unkeys() {
        let (rig, shared) = sim_handle(fast());
        wait_for("connect", || rig.state().connected);
        rig.ptt(true);
        wait_for("tx", || shared.transmitting.load(Ordering::Relaxed));
        wait_for("tx reported", || rig.state().transmitting);
        assert!(rig.state().ptt_engaged);
        rig.ptt(false);
        wait_for("rx", || !shared.transmitting.load(Ordering::Relaxed));
        wait_for("rx reported", || !rig.state().transmitting);
    }

    #[test]
    fn tx_timeout_unkeys() {
        let (rig, shared) = sim_handle(RigOptions {
            poll_interval: Duration::from_millis(10),
            tx_timeout: Some(Duration::from_millis(150)),
        });
        wait_for("connect", || rig.state().connected);
        rig.ptt(true);
        wait_for("tx", || shared.transmitting.load(Ordering::Relaxed));
        wait_for("timeout", || !shared.transmitting.load(Ordering::Relaxed));
        let s = rig.state();
        assert!(s.tx_timed_out);
        assert!(!s.ptt_engaged);
    }

    #[test]
    fn dropping_the_handle_unkeys() {
        let (rig, shared) = sim_handle(fast());
        wait_for("connect", || rig.state().connected);
        rig.ptt(true);
        wait_for("tx", || shared.transmitting.load(Ordering::Relaxed));
        drop(rig);
        assert!(!shared.transmitting.load(Ordering::Relaxed));
    }

    #[test]
    fn falls_back_to_plain_tx() {
        let shared = Arc::new(SimShared::new(14_285_000));
        shared.reject_tx0.store(true, Ordering::Relaxed);
        let s = shared.clone();
        let opener: Opener = Box::new(move || Ok(Box::new(SimRig::new(s.clone())) as Box<dyn Transport>));
        let rig = RigHandle::spawn(opener, fast(), Arc::new(|| {}));
        wait_for("connect", || rig.state().connected);
        rig.ptt(true);
        wait_for("tx via TX;", || shared.transmitting.load(Ordering::Relaxed));
        rig.ptt(false);
        wait_for("rx", || !shared.transmitting.load(Ordering::Relaxed));
    }

    #[test]
    fn open_failure_is_reported_and_retried() {
        let attempts = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let a = attempts.clone();
        let opener: Opener = Box::new(move || {
            a.fetch_add(1, Ordering::Relaxed);
            Err(io::Error::new(io::ErrorKind::NotFound, "COM9 not found"))
        });
        let rig = RigHandle::spawn(opener, fast(), Arc::new(|| {}));
        wait_for("error", || rig.state().error.is_some());
        let s = rig.state();
        assert!(!s.connected);
        assert!(s.error.unwrap().contains("COM9"));
        drop(rig); // must not hang in the retry wait
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }
}
