//! Read-only git helpers shared by the picker's session-mining pipeline
//! (`candidates.rs`) and `open-link`'s path resolution (`openlink.rs`).
//! Every function here only ever shells out to `git status`/`git log`/
//! `git rev-parse` — never a mutating git command.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

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

/// Absolute paths (relative to `toplevel`) with uncommitted changes.
pub(crate) fn dirty_paths(toplevel: &Path) -> Result<HashSet<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(toplevel)
        .arg("status")
        .arg("--porcelain")
        .output()
        .context("failed to run git status --porcelain")?;
    if !output.status.success() {
        anyhow::bail!("git status --porcelain failed");
    }
    Ok(parse_status_porcelain(
        &String::from_utf8_lossy(&output.stdout),
        toplevel,
    ))
}

/// Pure: parses `git status --porcelain` stdout into absolute paths.
/// Each line is `XY PATH` or, for renames, `XY OLD -> NEW` (keeps NEW only).
pub(crate) fn parse_status_porcelain(output: &str, toplevel: &Path) -> HashSet<String> {
    output
        .lines()
        .filter(|line| line.len() > 3)
        .map(|line| {
            let rest = &line[3..];
            let path = rest.rsplit(" -> ").next().unwrap_or(rest);
            toplevel.join(path).to_string_lossy().into_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parses_modified_added_and_renamed_entries() {
        let output = " M src/main.rs\n\
                       A  src/new.rs\n\
                       ?? untracked.txt\n\
                       R  old.rs -> src/renamed.rs\n";
        let paths = parse_status_porcelain(output, Path::new("/repo"));
        assert_eq!(
            paths,
            std::collections::HashSet::from([
                "/repo/src/main.rs".to_owned(),
                "/repo/src/new.rs".to_owned(),
                "/repo/untracked.txt".to_owned(),
                "/repo/src/renamed.rs".to_owned(),
            ])
        );
    }
}
