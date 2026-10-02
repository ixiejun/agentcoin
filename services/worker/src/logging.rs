//! The worker's logger (spec `market/public-worker` "工作者服务": logs never hold data or
//! results).
//!
//! Only the worker's own lines are written, and they carry jobs, units, counts and durations,
//! never items, texts or results. Library lines are dropped whatever the level, so no HTTP
//! library can trace a body.

use std::sync::Mutex;

/// Target of every line the worker logs.
pub const TARGET: &str = "ac-worker";

/// Where log lines go.
pub enum Sink {
    /// Standard error.
    Stderr,
    /// A shared buffer (tests).
    Buffer(&'static Mutex<Vec<String>>),
}

struct Logger(Sink);

impl log::Log for Logger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target() == TARGET && metadata.level() <= log::max_level()
    }

    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format!("{} {}", record.level(), record.args());
        match &self.0 {
            Sink::Stderr => eprintln!("{line}"),
            Sink::Buffer(b) => {
                if let Ok(mut v) = b.lock() {
                    v.push(line);
                }
            }
        }
    }

    fn flush(&self) {}
}

/// Installs the logger once; later calls are ignored.
pub fn init(level: log::LevelFilter, sink: Sink) {
    if log::set_boxed_logger(Box::new(Logger(sink))).is_ok() {
        log::set_max_level(level);
    }
}
