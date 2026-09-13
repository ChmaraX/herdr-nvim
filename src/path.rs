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
//! This used to be handled by a `herdr/run.sh` wrapper that exported a wider
//! PATH before exec'ing the binary. The manifest now invokes the binary
//! directly (so the same command works on Windows, which has no `sh`), so the
//! binary augments its own PATH instead — once, at the very start of `main`.

#[cfg(unix)]
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

    if let Some(home) = home {
        prepend.push(home.join(".local/bin"));
        prepend.push(home.join(".cargo/bin"));
    }

    let mut combined: Vec<PathBuf> = Vec::new();
    for dir in prepend.into_iter().chain(existing.iter().cloned()) {
        if !combined.contains(&dir) {
            combined.push(dir);
        }
    }
    combined
}

/// Prepend common install locations to this process's PATH so bare-name child
/// spawns (`nvim`, `herdr`) resolve even under a minimal inherited PATH. No-op
/// on non-Unix platforms, where GUI-launched processes inherit the user PATH
/// and herdr resolves PATHEXT shims itself.
///
/// Must run before any threads are spawned (call it first in `main`).
pub(crate) fn augment_path() {
    #[cfg(unix)]
    {
        let existing: Vec<PathBuf> = env::var_os("PATH")
            .map(|path| env::split_paths(&path).collect())
            .unwrap_or_default();
        let home = env::var_os("HOME").map(PathBuf::from);
        let herdr_bin = env::var_os("HERDR_BIN_PATH").map(PathBuf::from);

        let dirs = augmented_path_dirs(&existing, home.as_deref(), herdr_bin.as_deref());
        if let Ok(joined) = env::join_paths(dirs) {
            // Safe: `main` calls this before spawning any threads, so there is
            // no concurrent access to the process environment.
            env::set_var("PATH", joined);
        }
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
    fn prepends_herdr_bin_dir_first() {
        let dirs = augmented_path_dirs(&[], None, Some(Path::new("/opt/herdr/bin/herdr")));
        assert_eq!(dirs.first(), Some(&p("/opt/herdr/bin")));
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
