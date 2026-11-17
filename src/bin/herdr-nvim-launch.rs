//! The app mod's launcher, installed as `<home>/mods/herdr-nvim/bin/herdr-nvim`
//! (`.exe` on Windows): the path the mods and other mods run. It reads the
//! live generation's record (`launch_record`) and runs that runtime's
//! herdr-nvim with the app context in front of its own arguments:
//! `<runtime>/bin/herdr-nvim --app-home <home> --app-bin <app_bin> <args…>`.
//! No shell parses anything on the way. Unix replaces itself with it; Windows
//! has no exec, so it waits and passes the exit code on.

#[path = "../app/launch_record.rs"]
#[allow(dead_code)]
mod launch_record;
#[path = "../stdio_inherit.rs"]
mod stdio_inherit;

use std::{env, path::PathBuf, process::Command};

use launch_record::{InstallRecord, BIN_FLAG, HOME_FLAG};

fn main() {
    // The runtime gets duplicates of these; nothing else may (a daemon the
    // runtime starts would hold the app's exec pipes).
    stdio_inherit::keep_std_handles_to_self();
    let record = match mod_dir().and_then(|dir| InstallRecord::read_live(&dir)) {
        Ok(record) => record,
        Err(why) => {
            eprintln!("herdr-nvim: {why}; run the herdr-nvim installer again");
            std::process::exit(1);
        }
    };
    let mut command = Command::new(record.runtime_exe());
    command
        .arg(HOME_FLAG)
        .arg(&record.home)
        .arg(BIN_FLAG)
        .arg(&record.app_bin)
        .args(env::args_os().skip(1))
        // Set by the bash era's wrapper and inherited by its nvims' callbacks:
        // it would send this runtime to that old one's Lua.
        .env_remove("HERDR_NVIM_PLUGIN_ROOT");
    let why = run(command);
    eprintln!(
        "herdr-nvim: cannot run {}: {why}; run the herdr-nvim installer again",
        record.runtime_exe().display()
    );
    std::process::exit(1);
}

/// `<mod>/bin/herdr-nvim` → `<mod>`.
fn mod_dir() -> Result<PathBuf, String> {
    let exe = env::current_exe().map_err(|e| format!("cannot find myself: {e}"))?;
    exe.parent()
        .and_then(|bin| bin.parent())
        .map(PathBuf::from)
        .ok_or_else(|| format!("{} is not in a mod's bin folder", exe.display()))
}

#[cfg(unix)]
fn run(mut command: Command) -> std::io::Error {
    use std::os::unix::process::CommandExt;
    command.exec()
}

#[cfg(not(unix))]
fn run(mut command: Command) -> std::io::Error {
    match command.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(error) => error,
    }
}
