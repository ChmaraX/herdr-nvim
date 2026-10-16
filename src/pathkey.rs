//! One spelling per file.
//!
//! The picker merges paths from three sources and dedupes them by string:
//! agent session logs (the path as the model wrote it: `src/a.rs`,
//! `./src/a.rs`, `../a.rs`, or through a symlinked directory), git
//! (`<toplevel>/<path>`, with the toplevel's real path), and the pane's text.
//! Two spellings of one file used to become two picker rows, and the agent's
//! row lost git's diff stat. Every path that names a candidate, and the cwd
//! and toplevel those are built from, goes through this module so all
//! sources agree.

use std::path::{Component, Path, PathBuf};

/// `path` as the agent wrote it, made [`absolute`] against `cwd`, then
/// [`canonical`].
pub(crate) fn resolve(path: &str, cwd: &Path) -> String {
    canonical(&absolute(path, cwd))
        .to_string_lossy()
        .into_owned()
}

/// `path` made absolute against `cwd` (`~` expanded) and lexically
/// normalized (`.` and `..` resolved without touching the filesystem).
pub(crate) fn absolute(path: &str, cwd: &Path) -> PathBuf {
    let expanded = if let Some(rest) = path.strip_prefix('~') {
        let home = crate::home::home_dir()
            .map(|home| home.to_string_lossy().into_owned())
            .unwrap_or_default();
        PathBuf::from(format!("{home}{rest}"))
    } else if Path::new(path).is_absolute() || path.starts_with('/') {
        PathBuf::from(path)
    } else {
        cwd.join(path)
    };
    normalize(&expanded)
}

/// Lexically normalize a path (resolve `.` and `..` without touching the fs).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::RootDir | Component::Prefix(_) => out.push(comp.as_os_str()),
            Component::Normal(s) => out.push(s),
        }
    }
    out
}

/// The real path of an existing `path`: symlinks resolved and, on Windows,
/// 8.3 short names and letter case too. `path` unchanged when it does not
/// exist. Windows' `\\?\` verbatim prefix is dropped so the result compares
/// equal to the paths herdr and git report.
pub(crate) fn canonical(path: &Path) -> PathBuf {
    match std::fs::canonicalize(path) {
        Ok(real) => match without_verbatim_prefix(&real.to_string_lossy()) {
            Some(plain) => PathBuf::from(plain),
            None => real,
        },
        Err(_) => path.to_path_buf(),
    }
}

/// Pure: `\\?\C:\x` -> `C:\x` and `\\?\UNC\host\share\x` -> `\\host\share\x`.
/// `None` for anything else (no prefix, or a verbatim form like
/// `\\?\Volume{…}` that has no plain spelling).
fn without_verbatim_prefix(path: &str) -> Option<String> {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return Some(format!(r"\\{rest}"));
    }
    let rest = path.strip_prefix(r"\\?\")?;
    (rest.as_bytes().get(1) == Some(&b':')).then(|| rest.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_the_verbatim_prefix_from_drive_and_unc_paths() {
        assert_eq!(
            without_verbatim_prefix(r"\\?\C:\repo\a.rs").as_deref(),
            Some(r"C:\repo\a.rs")
        );
        assert_eq!(
            without_verbatim_prefix(r"\\?\UNC\host\share\a.rs").as_deref(),
            Some(r"\\host\share\a.rs")
        );
    }

    #[test]
    fn keeps_paths_without_a_plain_spelling() {
        assert_eq!(without_verbatim_prefix("/home/u/a.rs"), None);
        assert_eq!(without_verbatim_prefix(r"C:\repo\a.rs"), None);
        assert_eq!(without_verbatim_prefix(r"\\?\Volume{1234}\a.rs"), None);
    }
}
