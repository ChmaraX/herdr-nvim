//! Where the Herdr app's `herdr-app` may be on this machine. The installer
//! asks each candidate for `status`; a runtime whose recorded `herdr-app`
//! went away (an app update moved it) takes the first that exists.

use std::{env, path::PathBuf};

/// Where the app's CLI may be, most likely first, existing files only:
/// PATH, then where the app installs, then a running app's image. Only
/// candidates: the installer asks each for `status`.
pub(crate) fn app_bins() -> Vec<PathBuf> {
    let mut found = crate::osfs::on_path(&format!("herdr-app{}", env::consts::EXE_SUFFIX));
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

/// macOS: a Herdr bundle in /Applications or ~/Applications, found by its
/// bundle id (never by running an app that merely has the name).
#[cfg(target_os = "macos")]
fn platform_candidates() -> Vec<PathBuf> {
    use std::process::{Command, Stdio};
    let plist = |bundle: &std::path::Path, key: &str| {
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

/// Windows (unverified, doc/windows-facts.md): the per-user install
/// folders first (they outlive an app update), then the image of a running
/// Herdr app process and a `herdr-app.exe` beside it.
#[cfg(windows)]
fn platform_candidates() -> Vec<PathBuf> {
    let names = ["herdr-app.exe", "herdr.exe"];
    let is_app = |path: &std::path::Path| {
        path.file_name()
            .is_some_and(|name| names.iter().any(|n| name.eq_ignore_ascii_case(n)))
    };
    let mut found = Vec::new();
    if let Some(local) = env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        for dir in ["Programs\\Herdr", "Programs\\herdr-app", "Herdr"] {
            for name in names {
                found.push(local.join(dir).join(name));
            }
        }
    }
    for proc in crate::winproc::processes().unwrap_or_default() {
        if is_app(&proc.image) {
            found.push(proc.image.with_file_name("herdr-app.exe"));
            found.push(proc.image.clone());
        }
    }
    found.retain(|bin| bin.is_file());
    found
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_candidates() -> Vec<PathBuf> {
    Vec::new()
}
