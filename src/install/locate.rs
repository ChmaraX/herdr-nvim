//! Finding the Herdr app to install into: its home folder and its
//! `herdr-app` binary, which must answer `status` for that home (the app is
//! open).

use std::{
    path::Path,
    process::{Command, Stdio},
};

use anyhow::{bail, Result};
use serde_json::Value;

use crate::app::env::AppEnv;

/// `home`: `--home`, else `$HERDR_APP_HOME`, else `~/.config/herdr-app`; it
/// must exist. `app_bin`: `--app-bin`, else `$HERDR_APP_BIN`, else the first
/// candidate (`candidates`) that answers `status` for that home.
pub fn locate(home: Option<&Path>, app_bin: Option<&Path>) -> Result<AppEnv> {
    let home = match home {
        Some(home) => home.to_path_buf(),
        None => crate::app::env::home_from_env(),
    };
    if !home.is_dir() {
        bail!("no Herdr app home at {}", home.display());
    }
    let home = crate::pathkey::canonical(&home);
    let given = app_bin
        .map(Path::to_path_buf)
        .or_else(crate::app::env::app_bin_from_env);
    if let Some(bin) = given {
        if answers(&bin, &home) {
            return Ok(AppEnv { home, app_bin: bin });
        }
        bail!(
            "{} does not answer for {}: open the Herdr app first",
            bin.display(),
            home.display()
        );
    }
    let candidates = crate::app::discover::app_bins();
    if let Some(bin) = candidates.iter().find(|bin| answers(bin, &home)) {
        return Ok(AppEnv {
            home,
            app_bin: bin.clone(),
        });
    }
    if candidates.is_empty() {
        bail!("cannot find the Herdr app; pass --app-bin <path to herdr-app>");
    }
    bail!(
        "open the Herdr app first (home: {}; tried {})",
        home.display(),
        candidates
            .iter()
            .map(|bin| bin.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// `herdr-app status` for `home` printed a JSON object: this is the app,
/// open on that home. (A program that merely has the name, or the herdr
/// TUI, prints no JSON.)
fn answers(bin: &Path, home: &Path) -> bool {
    Command::new(bin)
        .arg("status")
        .env("HERDR_APP_HOME", home)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| serde_json::from_slice::<Value>(&output.stdout).ok())
        .is_some_and(|status| status.is_object())
}
