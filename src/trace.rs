//! `HERDR_NVIM_TRACE=<file>`: each step of a command appended to `file`
//! with the milliseconds since the process started, for finding where time
//! goes in a command run by the app. Unset (the default), it does nothing.

use std::{io::Write, sync::OnceLock, time::Instant};

fn start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Marks the process start; call first thing.
pub(crate) fn begin() {
    start();
}

pub(crate) fn step(label: &str) {
    let Some(path) = std::env::var_os("HERDR_NVIM_TRACE") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(
            file,
            "{:>6} ms  [{}] {label}",
            start().elapsed().as_millis(),
            std::process::id()
        );
    }
}
