//! A small logger: everything goes to a log file next to the settings (the
//! Windows build has no console) and to stderr.
//!
//! The level comes from `SEVENTY3_LOG` (error, warn, info, debug, trace);
//! the default is info. `trace` logs every CAT command and answer.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use log::{Level, LevelFilter, Log, Metadata, Record};

struct Logger {
    level: LevelFilter,
    file: Option<Mutex<File>>,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level && (metadata.target().starts_with("seventy3") || metadata.level() <= Level::Warn)
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let day = secs % 86_400.0;
        let line = format!(
            "{:02}:{:02}:{:06.3}Z {:<5} {}: {}\n",
            (day / 3600.0) as u32,
            ((day % 3600.0) / 60.0) as u32,
            day % 60.0,
            record.level(),
            record.target(),
            record.args()
        );
        eprint!("{line}");
        if let Some(file) = &self.file
            && let Ok(mut f) = file.lock()
        {
            let _ = f.write_all(line.as_bytes());
        }
    }

    fn flush(&self) {
        if let Some(file) = &self.file
            && let Ok(mut f) = file.lock()
        {
            let _ = f.flush();
        }
    }
}

pub fn log_path() -> Option<PathBuf> {
    crate::config::project_dirs().map(|d| d.data_local_dir().join("seventy3.log"))
}

pub fn init() {
    let level = std::env::var("SEVENTY3_LOG").ok().and_then(|v| v.parse().ok()).unwrap_or(LevelFilter::Info);
    let file = log_path().and_then(|path| {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok()?;
        }
        // Keep one previous log for "what happened last time?".
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
        File::create(&path).ok().map(Mutex::new)
    });
    if log::set_boxed_logger(Box::new(Logger { level, file })).is_ok() {
        log::set_max_level(level);
    }
}
