//! Finding the Herdr app to install into: its home folder and its
//! `herdr-app` binary, which must answer `status` for that home (the app is
//! open).

use std::{
    env,
    path::{Path, PathBuf},
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
    let given = app_bin.map(Path::to_path_buf).or_else(|| {
        env::var_os("HERDR_APP_BIN")
            .filter(|b| !b.is_empty())
            .map(PathBuf::from)
    });
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
    let candidates = candidates();
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

/// Where the app's CLI may be, most likely first, existing files only.
fn candidates() -> Vec<PathBuf> {
    let mut found = on_path(&format!("herdr-app{}", env::consts::EXE_SUFFIX));
    found.extend(platform_candidates());
    let mut seen = Vec::new();
    found.retain(|bin| {
        let key = crate::pathkey::canonical(bin);
        let new = !seen.contains(&key);
        seen.push(key);
        new
    });
    found
}

/// Every `name` on PATH, in PATH's order.
fn on_path(name: &str) -> Vec<PathBuf> {
    env::split_paths(&env::var_os("PATH").unwrap_or_default())
        .map(|dir| dir.join(name))
        .filter(|path| path.is_file())
        .collect()
}

/// macOS: a Herdr bundle in /Applications or ~/Applications, found by its
/// bundle id (never by running an app that merely has the name).
#[cfg(target_os = "macos")]
fn platform_candidates() -> Vec<PathBuf> {
    let plist = |bundle: &Path, key: &str| {
        Command::new("plutil")
            .args(["-extract", key, "raw"])
            .arg(bundle.join("Contents/Info.plist"))
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
    };
    let mut dirs = vec![PathBuf::from("/Applications")];
    dirs.extend(crate::home::home_dir().map(|home| home.join("Applications")));
    dirs.iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flat_map(|entries| entries.flatten().map(|entry| entry.path()))
        .filter(|bundle| bundle.extension().is_some_and(|ext| ext == "app"))
        .filter(|bundle| plist(bundle, "CFBundleIdentifier").as_deref() == Some("dev.herdr.app"))
        .filter_map(|bundle| {
            let exe = plist(&bundle, "CFBundleExecutable")?;
            Some(bundle.join("Contents/MacOS").join(exe))
        })
        .filter(|bin| bin.is_file())
        .collect()
}

/// Windows (unverified, doc/windows-facts.md): the image of a running Herdr
/// app process and a `herdr-app.exe` beside it, then the per-user install
/// folder. Each is only a candidate; `status` decides.
#[cfg(windows)]
fn platform_candidates() -> Vec<PathBuf> {
    let names = ["herdr-app.exe", "herdr.exe"];
    let is_app = |path: &Path| {
        path.file_name()
            .is_some_and(|name| names.iter().any(|n| name.eq_ignore_ascii_case(n)))
    };
    let mut found = Vec::new();
    for proc in crate::winproc::processes().unwrap_or_default() {
        if is_app(&proc.image) {
            found.push(proc.image.with_file_name("herdr-app.exe"));
            found.push(proc.image.clone());
        }
    }
    if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        for dir in ["Programs\\Herdr", "Programs\\herdr-app", "Herdr"] {
            for name in names {
                found.push(local.join(dir).join(name));
            }
        }
    }
    found.retain(|bin| bin.is_file());
    found
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_candidates() -> Vec<PathBuf> {
    Vec::new()
}
