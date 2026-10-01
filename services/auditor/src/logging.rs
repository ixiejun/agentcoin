//! The auditor's logger (spec `market/auditor-agent` "复核不记录内容").
//!
//! Only the agent's own lines are written, and they carry request IDs, model IDs, counts, comparison
//! metrics and verdicts, never messages, outputs, tokens or candidates. Library lines are dropped whatever
//! the level, so no HTTP library can trace a body.

use std::sync::Mutex;

/// Target of every line the agent logs.
pub const TARGET: &str = "ac-auditor";

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
