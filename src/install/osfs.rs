//! File moves that hold up on Windows, where an open file or a running exe
//! blocks renaming or deleting it and the folders above it for a while.

use std::{
    fs, io,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

/// How long a rename keeps trying while something holds the path (on
/// Windows a launcher run lasts milliseconds, at most the 25 s deadline of a
/// short command).
const RETRY_FOR: Duration = Duration::from_secs(5);

/// A name no other run uses: `<prefix><pid>-<nanos>`.
pub fn unique(prefix: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    format!("{prefix}{}-{nanos}", std::process::id())
}

/// Whether an error means another process holds the path for now (Windows
/// sharing violations and access denied), as opposed to a real failure.
fn held(error: &io::Error) -> bool {
    cfg!(windows)
        && (error.kind() == io::ErrorKind::PermissionDenied || error.raw_os_error() == Some(32))
}

/// `fs::rename`, retried for a few seconds while the source or target is held.
pub fn rename_retrying(from: &Path, to: &Path) -> io::Result<()> {
    let started = Instant::now();
    loop {
        match fs::rename(from, to) {
            Err(error) if held(&error) && started.elapsed() < RETRY_FOR => {
                thread::sleep(Duration::from_millis(100));
            }
            other => return other,
        }
    }
}

/// Removes a folder (or file) whole, or not at all as far as its name goes:
/// it is first renamed to a `.trash-*` sibling, which Windows refuses while
/// an exe inside runs (`Err`, nothing changed), then deleted. A delete that
/// fails halfway leaves only the `.trash-*` name, which `sweep` takes later.
pub fn remove_tree(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    let parent = path.parent().unwrap_or(Path::new("."));
    let trash = parent.join(unique(".trash-"));
    fs::rename(path, &trash)?;
    remove_any(&trash)
}

fn remove_any(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Deletes what earlier runs left in `dir`: entries whose names start with
/// one of `prefixes` (`.trash-`, `.new.`). Best effort.
pub fn sweep(dir: &Path, prefixes: &[&str]) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if prefixes.iter().any(|prefix| name.starts_with(prefix)) {
            let _ = remove_any(&entry.path());
        }
    }
}

/// Writes `bytes` to `path` through a temp file renamed over it, so a reader
/// sees the old file or the new one, never half of one.
pub fn replace_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let temp = path.with_file_name(unique(&format!(".{name}.")));
    fs::write(&temp, bytes)?;
    rename_retrying(&temp, path).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// Puts the program `from` at `to` unless the same bytes are there. Windows
/// can't overwrite a running exe, but it can rename one: the running one is
/// moved aside (`.<name>.old-*`, deleted by a later `sweep`).
pub fn replace_exe(from: &Path, to: &Path) -> io::Result<()> {
    if fs::read(to).ok() == Some(fs::read(from)?) {
        return Ok(());
    }
    let name = to.file_name().unwrap_or_default().to_string_lossy();
    let temp = to.with_file_name(unique(&format!(".{name}.new-")));
    fs::copy(from, &temp)?;
    let placed = match fs::rename(&temp, to) {
        Err(error) if held(&error) => {
            fs::rename(to, to.with_file_name(unique(&format!(".{name}.old-"))))
                .and_then(|()| fs::rename(&temp, to))
        }
        other => other,
    };
    placed.inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

/// Copies the tree at `from` to `to` (which must not exist): folders and
/// files; anything else is skipped.
pub fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Every file under `root`, as `/`-separated paths relative to it, sorted
/// bytewise (as `LC_ALL=C sort` does).
pub fn files_under(root: &Path) -> io::Result<Vec<String>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                walk(root, &path, out)?;
            } else {
                let rel: PathBuf = path.strip_prefix(root).unwrap_or(&path).into();
                let parts: Vec<String> = rel
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect();
                out.push(parts.join("/"));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}
