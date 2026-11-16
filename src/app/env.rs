//! Which app home and `herdr-app` binary this process serves.
//!
//! It reaches a process in one of two ways:
//! - **argv**: `herdr-nvim --app-home H --app-bin B <command> …`. The mod's
//!   launcher (`bin/herdr-nvim`) and every terminal herdr-nvim starts in the
//!   app (`self_argv`) use it: argv is the one channel that crosses the app's
//!   `exec`, `pane.split` and the core's `terminal.run` alike, on every OS.
//! - **env**: `HERDR_NVIM_HOST=app`, `HERDR_APP_HOME`, `HERDR_APP_BIN`. `main`
//!   exports the argv form, so children (the tab nvims and their Lua
//!   callbacks, `herdr-app`, `hc`) inherit it. Wrappers written by older
//!   installs set it too.
//!
//! The env is the one truth inside a process: `host::choose` reads it.

use std::{
    env,
    ffi::{OsStr, OsString},
    path::PathBuf,
};

use anyhow::{bail, Context, Result};

pub const HOME_FLAG: &str = "--app-home";
pub const BIN_FLAG: &str = "--app-bin";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEnv {
    pub home: PathBuf,
    /// `herdr-app` as given: a path, or a name found on PATH.
    pub app_bin: PathBuf,
}

impl AppEnv {
    /// Takes `--app-home H --app-bin B` off the front of `args` (after
    /// argv[0]). `None` when `args` has no such prefix.
    pub fn take_prefix(args: &mut Vec<OsString>) -> Result<Option<AppEnv>> {
        if args.get(1).map(OsString::as_os_str) != Some(OsStr::new(HOME_FLAG)) {
            return Ok(None);
        }
        let (Some(home), Some(flag), Some(app_bin)) = (args.get(2), args.get(3), args.get(4))
        else {
            bail!("{HOME_FLAG} needs a folder and {BIN_FLAG} a program");
        };
        if flag != BIN_FLAG {
            bail!("{HOME_FLAG} H must be followed by {BIN_FLAG} B");
        }
        let app = AppEnv {
            home: PathBuf::from(home),
            app_bin: PathBuf::from(app_bin),
        };
        args.drain(1..5);
        Ok(Some(app))
    }

    /// The app context this process inherited, if it serves the app.
    pub fn from_env() -> Option<AppEnv> {
        if !crate::host::is_app_value(env::var(crate::host::HOST_ENV).ok().as_deref()) {
            return None;
        }
        Some(AppEnv {
            home: home_from_env(),
            app_bin: env::var_os("HERDR_APP_BIN")
                .filter(|bin| !bin.is_empty())
                .map_or_else(|| PathBuf::from("herdr-app"), PathBuf::from),
        })
    }

    /// Makes this the process's context, for itself and every child. Must
    /// run before any thread starts (`main`, or the installer before it
    /// spawns anything).
    pub fn export(&self) {
        env::set_var(crate::host::HOST_ENV, "app");
        env::set_var("HERDR_APP_HOME", &self.home);
        env::set_var("HERDR_APP_BIN", &self.app_bin);
    }

    /// `--app-home H --app-bin B`, for a runtime exe to serve this context.
    pub fn prefix(&self) -> [OsString; 4] {
        [
            HOME_FLAG.into(),
            self.home.clone().into(),
            BIN_FLAG.into(),
            self.app_bin.clone().into(),
        ]
    }

    /// What a terminal the app or the core starts runs to be this binary in
    /// this context. That terminal gets the core's env, not ours, so the
    /// context goes in argv. It is this runtime's exe, never the mod's
    /// launcher: a long-lived process must not hold the mod folder (Windows
    /// refuses to rename a folder an exe inside runs from).
    pub fn self_argv(&self) -> Result<Vec<String>> {
        let exe = env::current_exe().context("cannot resolve the herdr-nvim binary")?;
        Ok(std::iter::once(exe.into_os_string())
            .chain(self.prefix())
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect())
    }
}

/// The app home: `HERDR_APP_HOME`, else `~/.config/herdr-app`.
pub fn home_from_env() -> PathBuf {
    env::var_os("HERDR_APP_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| crate::home::home_dir().map(|home| home.join(".config/herdr-app")))
        .unwrap_or_else(|| PathBuf::from(".config/herdr-app"))
}

/// [`AppEnv::self_argv`] for the process's own context.
pub fn self_argv() -> Result<Vec<String>> {
    AppEnv::from_env()
        .context("not running for the Herdr app (no --app-home)")?
        .self_argv()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn the_prefix_comes_off_and_the_command_keeps_its_arguments() {
        let mut argv = args(&[
            "herdr-nvim",
            "--app-home",
            "/h o/me",
            "--app-bin",
            "C:\\Program Files\\Herdr\\herdr-app.exe",
            "open",
            "--app-home",
        ]);
        let app = AppEnv::take_prefix(&mut argv).unwrap().unwrap();
        assert_eq!(app.home, PathBuf::from("/h o/me"));
        assert_eq!(
            app.app_bin,
            PathBuf::from("C:\\Program Files\\Herdr\\herdr-app.exe")
        );
        assert_eq!(argv, args(&["herdr-nvim", "open", "--app-home"]));
    }

    #[test]
    fn a_command_without_the_prefix_is_left_alone() {
        let mut argv = args(&["herdr-nvim", "toggle", "--app-home", "x"]);
        assert_eq!(AppEnv::take_prefix(&mut argv).unwrap(), None);
        assert_eq!(argv.len(), 4);
    }

    #[test]
    fn a_half_prefix_is_an_error_not_a_command() {
        for bad in [
            &["herdr-nvim", "--app-home"][..],
            &["herdr-nvim", "--app-home", "/h", "open", "x"][..],
        ] {
            assert!(AppEnv::take_prefix(&mut args(bad)).is_err(), "{bad:?}");
        }
    }
}
