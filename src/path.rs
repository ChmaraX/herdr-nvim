//! PATH augmentation for spawned children (`nvim`, `herdr`).
//!
//! herdr may launch the plugin binary with a minimal PATH — most notably when
//! herdr itself was started from a macOS GUI (Finder/launchd), which hands
//! processes only `/usr/bin:/bin:/usr/sbin:/sbin`. In that case bare-name
//! spawns like `Command::new("nvim")` (`config::default_nvim_bin`) and
//! `Command::new("herdr")` (`herdr::CliHerdr`) fail with "No such file or
//! directory" because Homebrew (`/opt/homebrew/bin`), `~/.local/bin`, etc. are
//! not on PATH.
//!
//! On Windows the Herdr app's core gives its terminals the PATH stored in the
//! registry (the machine's and the person's), while a mod's `exec` runs with
//! the app's own PATH, from when the app started. A Neovim installed since
//! (winget, scoop, an installer) is on the one and not the other, so the
//! process that checks for nvim and the pane that runs it disagree. Each
//! process adds the registry's PATH, read now, after its own.
//!
//! This used to be handled by a `herdr/run.sh` wrapper that exported a wider
//! PATH before exec'ing the binary. The manifest now invokes the binary
//! directly (so the same command works on Windows, which has no `sh`), so the
//! binary augments its own PATH instead — once, at the very start of `main`.

#[cfg(any(unix, windows))]
use std::env;
use std::path::{Path, PathBuf};

/// Build the PATH search list: common install locations (and the directory of
/// herdr's own binary, if known) prepended to whatever PATH we inherited, with
/// duplicates removed while preserving first-seen order.
///
/// Pure and platform-independent so it can be unit-tested anywhere; only
/// [`augment_path`] applies it, and only on Unix (hence unused in non-Unix
/// non-test builds).
#[cfg_attr(not(any(unix, test)), allow(dead_code))]
pub(crate) fn augmented_path_dirs(
    existing: &[PathBuf],
    home: Option<&Path>,
    herdr_bin: Option<&Path>,
) -> Vec<PathBuf> {
    let mut prepend: Vec<PathBuf> = Vec::new();

    // herdr injects HERDR_BIN_PATH pointing at its own executable; ensuring its
    // directory is on PATH makes `Command::new("herdr")` resolve regardless of
    // where the user installed herdr.
    if let Some(parent) = herdr_bin.and_then(Path::parent) {
        if !parent.as_os_str().is_empty() {
            prepend.push(parent.to_path_buf());
        }
    }

    for dir in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        prepend.push(PathBuf::from(dir));
    }

    // Homebrew on Linux. Its prefix depends on how it was installed: the
    // tarball install (the default on Linux) uses `~/.linuxbrew`, the
    // system-wide "linuxbrew" one uses `/home/linuxbrew/.linuxbrew`. Neither is
    // on the minimal PATH that a herdr server inherits when it is started
    // from a systemd unit or over SSH, so a bare `nvim` spawn fails there.
    // `$HOME/.linuxbrew/bin` is added below, with the other home dirs.
    prepend.push(PathBuf::from("/home/linuxbrew/.linuxbrew/bin"));

    if let Some(home) = home {
        prepend.push(home.join(".local/bin"));
        prepend.push(home.join(".cargo/bin"));
        prepend.push(home.join(".linuxbrew/bin"));
    }

    let mut combined: Vec<PathBuf> = Vec::new();
    for dir in prepend.into_iter().chain(existing.iter().cloned()) {
        if !combined.contains(&dir) {
            combined.push(dir);
        }
    }
    combined
}

/// `existing`, then each of `extra` it doesn't have yet (Windows: compared
/// without case or a trailing `\`).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn appended_path_dirs(existing: &[PathBuf], extra: &[PathBuf]) -> Vec<PathBuf> {
    let key = |dir: &PathBuf| {
        dir.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    let mut combined: Vec<PathBuf> = Vec::new();
    for dir in existing.iter().chain(extra) {
        if dir.as_os_str().is_empty() || combined.iter().any(|seen| key(seen) == key(dir)) {
            continue;
        }
        combined.push(dir.clone());
    }
    combined
}

/// Unix: prepend common install locations to this process's PATH so
/// bare-name child spawns (`nvim`, `herdr`) resolve even under a minimal
/// inherited PATH. Windows: append the registry's PATH, as it is now.
///
/// Must run before any threads are spawned (call it first in `main`).
pub(crate) fn augment_path() {
    #[cfg(windows)]
    {
        let existing: Vec<PathBuf> = env::var_os("PATH")
            .map(|path| env::split_paths(&path).collect())
            .unwrap_or_default();
        let dirs = appended_path_dirs(&existing, &registry::path_dirs());
        if let Ok(joined) = env::join_paths(dirs) {
            // Safe: before any thread (see above).
            env::set_var("PATH", joined);
        }
    }
    #[cfg(unix)]
    {
        let existing: Vec<PathBuf> = env::var_os("PATH")
            .map(|path| env::split_paths(&path).collect())
            .unwrap_or_default();
        let home = crate::home::home_dir();
        let herdr_bin = env::var_os("HERDR_BIN_PATH").map(PathBuf::from);

        let dirs = augmented_path_dirs(&existing, home.as_deref(), herdr_bin.as_deref());
        if let Ok(joined) = env::join_paths(dirs) {
            // Safe: `main` calls this before spawning any threads, so there is
            // no concurrent access to the process environment.
            env::set_var("PATH", joined);
        }
    }
}

/// The PATH Windows stores for new processes: the machine's, then the
/// person's, `%VARIABLES%` expanded.
#[cfg(windows)]
mod registry {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, ptr};

    use windows_sys::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ,
        RRF_RT_REG_SZ,
    };

    pub(super) fn path_dirs() -> Vec<PathBuf> {
        [
            (
                HKEY_LOCAL_MACHINE,
                r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            ),
            (HKEY_CURRENT_USER, "Environment"),
        ]
        .into_iter()
        .filter_map(|(root, key)| read(root, key, "Path"))
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .collect()
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    /// A string value, `REG_EXPAND_SZ` expanded (RegGetValueW does that).
    fn read(root: HKEY, key: &str, name: &str) -> Option<OsString> {
        let (key, name) = (wide(key), wide(name));
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
        let mut bytes: u32 = 0;
        // SAFETY: a size query with no buffer; both strings end in NUL.
        let status = unsafe {
            RegGetValueW(
                root,
                key.as_ptr(),
                name.as_ptr(),
                flags,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut bytes,
            )
        };
        if status != 0 || bytes == 0 {
            return None;
        }
        let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
        // SAFETY: `buffer` holds `bytes` bytes, as the call is told.
        let status = unsafe {
            RegGetValueW(
                root,
                key.as_ptr(),
                name.as_ptr(),
                flags,
                ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if status != 0 {
            return None;
        }
        let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        Some(OsString::from_wide(&buffer[..len]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn prepends_common_dirs_and_keeps_existing() {
        let existing = [p("/custom/bin")];
        let dirs = augmented_path_dirs(&existing, None, None);
        assert_eq!(dirs.first(), Some(&p("/opt/homebrew/bin")));
        assert!(dirs.contains(&p("/usr/local/bin")));
        // Inherited entries survive, after the prepended ones.
        assert_eq!(dirs.last(), Some(&p("/custom/bin")));
    }

    #[test]
    fn includes_home_dirs_when_home_is_set() {
        let dirs = augmented_path_dirs(&[], Some(Path::new("/home/u")), None);
        assert!(dirs.contains(&p("/home/u/.local/bin")));
        assert!(dirs.contains(&p("/home/u/.cargo/bin")));
    }

    #[test]
    fn includes_linuxbrew_dirs() {
        // Homebrew on Linux installs into a prefix that is not on the minimal
        // PATH a herdr server inherits when started from a systemd unit or
        // over SSH. Without these, a bare `nvim` spawn fails with "No such
        // file or directory".
        let dirs = augmented_path_dirs(&[], Some(Path::new("/home/u")), None);
        assert!(dirs.contains(&p("/home/linuxbrew/.linuxbrew/bin")));
        assert!(dirs.contains(&p("/home/u/.linuxbrew/bin")));
    }

    #[test]
    fn prepends_herdr_bin_dir_first() {
        let dirs = augmented_path_dirs(&[], None, Some(Path::new("/opt/herdr/bin/herdr")));
        assert_eq!(dirs.first(), Some(&p("/opt/herdr/bin")));
    }

    #[test]
    fn the_registry_path_comes_after_and_only_once() {
        let existing = [p(r"C:\Tools\bin"), p(r"C:\Windows\system32")];
        let extra = [
            p(r"c:\windows\System32\"),
            p(r"C:\Program Files\Neovim\bin"),
            p(""),
        ];
        assert_eq!(
            appended_path_dirs(&existing, &extra),
            [
                p(r"C:\Tools\bin"),
                p(r"C:\Windows\system32"),
                p(r"C:\Program Files\Neovim\bin")
            ]
        );
    }

    #[test]
    fn dedupes_without_losing_order() {
        // A common dir already inherited must not appear twice.
        let existing = [p("/usr/bin"), p("/custom/bin")];
        let dirs = augmented_path_dirs(&existing, None, None);
        let usr_bin_count = dirs.iter().filter(|d| **d == p("/usr/bin")).count();
        assert_eq!(usr_bin_count, 1, "dirs: {dirs:?}");
        assert!(dirs.contains(&p("/custom/bin")));
    }
}
