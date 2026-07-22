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

/// Absolute paths touched by any commit in `toplevel` since `since_unix`.
pub(crate) fn committed_since(toplevel: &Path, since_unix: u64) -> Result<HashSet<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(toplevel)
        .arg("log")
        .arg("--name-only")
        .arg("--pretty=format:commit")
        .arg(format!("--since=@{since_unix}"))
        .output()
        .context("failed to run git log --name-only")?;
    if !output.status.success() {
        anyhow::bail!("git log --name-only failed");
    }
    Ok(parse_log_name_only(
        &String::from_utf8_lossy(&output.stdout),
        toplevel,
    ))
}

/// Pure: parses `git log --name-only` stdout into absolute paths. The real
/// shell-out pins `--pretty=format:commit` (a bare `commit` marker line per
/// commit, then a blank line, then one filename per line) so filenames can
/// never collide with commit-message content; this parser also tolerates
/// the default multi-line `commit <hash>` / `Author:` / `Date:` / indented
/// message-body header shape defensively, in case the format ever changes.
pub(crate) fn parse_log_name_only(output: &str, toplevel: &Path) -> HashSet<String> {
    output
        .lines()
        .filter(|line| {
            !line.is_empty()
                && *line != "commit"
                && !line.starts_with("commit ")
                && !line.starts_with("Author:")
                && !line.starts_with("Date:")
                && !line.starts_with("    ")
        })
        .map(|line| toplevel.join(line).to_string_lossy().into_owned())
        .collect()
}

/// The net-change demotion rule (brief, "Net-change demotion"): should a
/// session-edited path stay in EDITED?
/// - Not in any git worktree at all -> always keep (unverifiable).
/// - In a worktree: keep iff currently dirty OR committed during the
///   session; otherwise it was rolled back -> demote to MENTIONED.
pub(crate) fn should_keep_edited(in_git_worktree: bool, dirty: bool, committed_in_session: bool) -> bool {
    if !in_git_worktree {
        return true;
    }
    dirty || committed_in_session
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

    #[test]
    fn parses_name_only_log_across_multiple_commits() {
        // git log --name-only separates commits with a blank line; each commit
        // is a header line (starts with "commit ") followed by metadata lines,
        // a blank line, then one filename per line.
        // Note: Rust's `\<newline>` line-continuation strips all leading
        // whitespace on the following line, so a plain concatenated literal
        // can't preserve the message body's 4-space indent -- use a raw
        // string instead so the indentation this parser depends on survives.
        let output = r#"commit abc123
Author: a
Date:   d


src/a.rs
src/b.rs

commit def456
Author: a
Date:   d

    fix

src/b.rs
src/c.rs
"#;
        let paths = parse_log_name_only(output, Path::new("/repo"));
        assert_eq!(
            paths,
            HashSet::from([
                "/repo/src/a.rs".to_owned(),
                "/repo/src/b.rs".to_owned(),
                "/repo/src/c.rs".to_owned(),
            ])
        );
    }

    #[test]
    fn dirty_file_is_kept() {
        assert!(should_keep_edited(true, true, false));
    }

    #[test]
    fn committed_in_session_is_kept() {
        assert!(should_keep_edited(true, false, true));
    }

    #[test]
    fn clean_and_not_committed_is_demoted() {
        assert!(!should_keep_edited(true, false, false));
    }

    #[test]
    fn non_git_path_always_kept() {
        assert!(should_keep_edited(false, false, false));
    }
}
