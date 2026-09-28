//! The adapter's logger (spec evm/eth-rpc "适配器的边界").
//!
//! Only the adapter's own lines are written: method names, durations and error classes. Lines of
//! the libraries underneath (the JSON-RPC server traces whole requests at trace level) are
//! dropped whatever the level.

/// Target of every line the adapter logs.
pub const TARGET: &str = "eth-rpc";

/// Whether a line is written: the adapter's own lines at or below the configured level.
pub fn shown(metadata: &log::Metadata<'_>) -> bool {
    metadata.target() == TARGET && metadata.level() <= log::max_level()
}

/// Writes the adapter's log lines to standard error.
pub struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        shown(metadata)
    }

    fn log(&self, record: &log::Record<'_>) {
        if shown(record.metadata()) {
            eprintln!("{} {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}
