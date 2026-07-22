//! Read-only git helpers shared by the picker's session-mining pipeline
//! (`candidates.rs`) and `open-link`'s path resolution (`openlink.rs`).
//! Every function here only ever shells out to `git status`/`git log`/
//! `git rev-parse` — never a mutating git command.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Shell out to `git -C <cwd> rev-parse --show-toplevel`. `None` if `git`
/// fails or isn't on PATH (e.g. `cwd` isn't inside a repo).
pub(crate) fn toplevel(cwd: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| PathBuf::from(trimmed))
}
