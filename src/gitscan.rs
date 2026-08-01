//! Read-only git helpers shared by the picker's session-mining pipeline
//! (`candidates.rs`) and `open-link`'s path resolution (`openlink.rs`).
//! Every function here only ever shells out to `git status`/`git log`/
//! `git rev-parse` — never a mutating git command.

use std::collections::{HashMap, HashSet};
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

/// Every tracked-or-untracked (but not git-ignored) file in the worktree, as
/// absolute paths. Used as the picker's repo-wide search pool: the default
/// (empty-query) view shows only session-touched files, but once the user
/// types, matching widens to the whole repo. Shells out to `git ls-files`
/// with `--cached --others --exclude-standard` so it honours `.gitignore`
/// exactly like git does, and `-z` so paths with newlines/spaces survive.
pub(crate) fn list_repo_files(toplevel: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(toplevel)
        .arg("ls-files")
        .arg("--cached")
        .arg("--others")
        .arg("--exclude-standard")
        .arg("-z")
        .output()
        .context("failed to run git ls-files")?;
    if !output.status.success() {
        anyhow::bail!("git ls-files failed");
    }
    Ok(parse_ls_files_z(
        &String::from_utf8_lossy(&output.stdout),
        toplevel,
    ))
}

/// Pure: parses NUL-separated `git ls-files -z` stdout into absolute paths,
/// preserving git's output order (roughly alphabetical) and skipping the
/// trailing empty field after the final NUL.
pub(crate) fn parse_ls_files_z(output: &str, toplevel: &Path) -> Vec<String> {
    output
        .split('\0')
        .filter(|entry| !entry.is_empty())
        .map(|entry| toplevel.join(entry).to_string_lossy().into_owned())
        .collect()
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
pub(crate) fn should_keep_edited(
    in_git_worktree: bool,
    dirty: bool,
    committed_in_session: bool,
) -> bool {
    if !in_git_worktree {
        return true;
    }
    dirty || committed_in_session
}

/// Combined (added, removed) line counts for every path with a diff versus
/// `HEAD` in `toplevel`, keyed by absolute path -- one `git diff HEAD
/// --numstat` invocation covers both staged and unstaged changes across the
/// whole worktree, replacing what used to be two `git diff`/`git diff
/// --cached` subprocess spawns *per dirty file* (profiled as the largest
/// remaining per-invocation git cost in the picker's action phase).
///
/// A brand-new repo with zero commits has no `HEAD`, so `git diff HEAD`
/// fails in that case -- treated as "no diff stats available" (an empty
/// map), not an error, so a fresh repo just shows no diff stats rather than
/// breaking the picker.
pub(crate) fn diff_numstat_by_path(toplevel: &Path) -> Result<HashMap<String, (u32, u32)>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(toplevel)
        .arg("diff")
        .arg("HEAD")
        .arg("--numstat")
        .output()
        .context("failed to run git diff HEAD --numstat")?;
    if !output.status.success() {
        return Ok(HashMap::new());
    }
    Ok(parse_diff_numstat_map(
        &String::from_utf8_lossy(&output.stdout),
        toplevel,
    ))
}

/// Pure: parses `git diff HEAD --numstat` stdout into a map of absolute
/// path -> (added, removed). Each line is `<added>\t<removed>\t<path>`
/// (toplevel-relative, joined the same way `parse_status_porcelain` does);
/// binary files use `-` for both counts, which contribute 0 (not an error).
/// A malformed line (fewer than 3 tab-separated fields) is skipped, never
/// fatal.
pub(crate) fn parse_diff_numstat_map(output: &str, toplevel: &Path) -> HashMap<String, (u32, u32)> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let added_str = parts.next()?;
            let removed_str = parts.next()?;
            let path = parts.next()?;
            let added: u32 = added_str.parse().unwrap_or(0);
            let removed: u32 = removed_str.parse().unwrap_or(0);
            Some((
                toplevel.join(path).to_string_lossy().into_owned(),
                (added, removed),
            ))
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

    #[test]
    fn parses_ls_files_z_into_ordered_absolute_paths() {
        // -z output is NUL-separated with a trailing NUL after the last entry.
        let output = "src/main.rs\0lua/foo.lua\0a file with spaces.txt\0";
        let paths = parse_ls_files_z(output, Path::new("/repo"));
        assert_eq!(
            paths,
            vec![
                "/repo/src/main.rs".to_owned(),
                "/repo/lua/foo.lua".to_owned(),
                "/repo/a file with spaces.txt".to_owned(),
            ]
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

    #[test]
    fn diff_numstat_map_keys_by_absolute_path_and_handles_binary() {
        let output = "3\t1\tsrc/a.rs\n0\t5\tsrc/b.rs\n-\t-\tassets/image.png\n";
        let map = parse_diff_numstat_map(output, Path::new("/repo"));
        assert_eq!(map.len(), 3);
        assert_eq!(map.get("/repo/src/a.rs"), Some(&(3, 1)));
        assert_eq!(map.get("/repo/src/b.rs"), Some(&(0, 5)));
        assert_eq!(map.get("/repo/assets/image.png"), Some(&(0, 0)));
    }

    #[test]
    fn diff_numstat_map_empty_output_yields_empty_map() {
        assert!(parse_diff_numstat_map("", Path::new("/repo")).is_empty());
    }
}
