//! The process's arguments, as commands read them: `std::env::args()` minus
//! the app's `--app-home H --app-bin B` prefix, which `main` takes off first
//! (`app::env`). Commands read their arguments here, never from
//! `std::env::args()`.

use std::{ffi::OsString, sync::OnceLock};

static ARGS: OnceLock<Vec<String>> = OnceLock::new();

/// Called once by `main`, before any command runs.
pub fn init(args: Vec<OsString>) {
    let args = args
        .into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    ARGS.set(args).expect("argv::init runs once");
}

/// `std::env::args()` without the app prefix.
pub fn args() -> impl Iterator<Item = String> {
    ARGS.get()
        .cloned()
        .unwrap_or_else(|| std::env::args().collect())
        .into_iter()
}
