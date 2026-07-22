//! Pure path extraction from agent-pane output.
//!
//! Reads a raw terminal pane capture (newest lines last), strips TUI chrome,
//! and returns the file paths mentioned in it as [`ScrapedPath`]s ordered
//! newest-first. This module performs no I/O of its own: existence is checked
//! through an injected closure so it stays deterministic and testable.

// Wired into the file-bridge picker in a later M3 task; unused until then.
#![allow(dead_code)]

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A file path discovered in agent output, with an optional 1-based line
/// number. Used only for the scrape fallback layer (see
/// candidates::build_candidates).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScrapedPath {
    pub path: String,
    pub line: Option<u32>,
}

/// Extract file-path candidates from `text`.
///
/// * `text` — raw pane read, newest lines last.
/// * `cwd` — pane foreground cwd, used to resolve relative paths.
/// * `exists` — injectable existence check; only paths for which it returns
///   `true` are kept (callers should return `true` only for real files).
///
/// Returns candidates deduped by resolved path (keeping the newest occurrence's
/// line number), ordered newest-first (reverse document order).
pub fn extract(text: &str, cwd: &Path, exists: &dyn Fn(&Path) -> bool) -> Vec<ScrapedPath> {
    // TUI chrome characters that never belong to a path.
    const CHROME: [char; 13] = [
        '│', '┃', '╭', '╮', '╰', '╯', '─', '━', '═', '├', '┤', '●', '∴',
    ];

    let mut all: Vec<ScrapedPath> = Vec::new();
    for line in text.lines() {
        let cleaned = line.replace(CHROME, " ");
        for raw in cleaned.split_whitespace() {
            let tok = trim_edges(raw);
            if tok.is_empty() {
                continue;
            }
            if let Some((path, lineno)) = parse_token(tok) {
                let resolved = resolve(path, cwd);
                if exists(&resolved) {
                    all.push(ScrapedPath {
                        path: resolved.to_string_lossy().into_owned(),
                        line: lineno,
                    });
                }
            }
        }
    }

    // Dedupe keeping the last (newest) occurrence, and return newest-first by
    // walking document order in reverse.
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for cand in all.into_iter().rev() {
        if seen.insert(cand.path.clone()) {
            out.push(cand);
        }
    }
    out
}

/// True for characters that may appear inside a path token (including the `:`
/// used by the trailing `:line[:col]` suffix).
fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | '~' | '@' | ':')
}

/// Trim surrounding punctuation (quotes, brackets, commas, …) from a token.
fn trim_edges(s: &str) -> &str {
    s.trim_matches(|c: char| !is_path_char(c))
}

/// Decide whether `tok` is path-shaped and split off any trailing line number.
///
/// Returns `(path, line)` on success. Applies an extension-or-slash heuristic so
/// prose like `and/or` is ignored while `src/main.rs` is kept.
pub(crate) fn parse_token(tok: &str) -> Option<(&str, Option<u32>)> {
    let mut parts = tok.split(':');
    let path = parts.next()?;

    // Must look like a path: contain a separator and either be absolute /
    // home-relative, an explicit `./`|`../` reference, or carry a file
    // extension in its final segment.
    if !path.contains('/') {
        return None;
    }
    let is_abs = path.starts_with('/') || path.starts_with('~');
    let is_dotslash = path.starts_with("./") || path.starts_with("../");
    let last = path.rsplit('/').next().unwrap_or("");
    let has_ext = last.contains('.');
    if !(is_abs || is_dotslash || has_ext) {
        return None;
    }

    let line = parts.next().and_then(|s| {
        if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
            s.parse::<u32>().ok()
        } else {
            None
        }
    });

    Some((path, line))
}

/// Resolve a path token to an absolute, lexically-normalized [`PathBuf`].
pub(crate) fn resolve(path: &str, cwd: &Path) -> PathBuf {
    let expanded = if let Some(rest) = path.strip_prefix('~') {
        let home = std::env::var("HOME").unwrap_or_default();
        PathBuf::from(format!("{home}{rest}"))
    } else if path.starts_with('/') {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    fn always(_: &Path) -> bool {
        true
    }

    #[test]
    fn extracts_absolute_with_line() {
        let c = extract(
            "error at /tmp/a/b.rs:42:7 in build",
            Path::new("/x"),
            &always,
        );
        assert_eq!(
            c,
            vec![ScrapedPath {
                path: "/tmp/a/b.rs".into(),
                line: Some(42)
            }]
        );
    }

    #[test]
    fn resolves_relative_against_cwd() {
        let c = extract("modified src/main.rs", Path::new("/repo"), &always);
        assert_eq!(c[0].path, "/repo/src/main.rs");
    }

    #[test]
    fn strips_box_chrome() {
        let c = extract("│ ● /tmp/x.py │", Path::new("/"), &always);
        assert_eq!(c[0].path, "/tmp/x.py");
    }

    #[test]
    fn dedupes_keeping_newest_and_orders_newest_first() {
        let text = "/tmp/old.rs\n/tmp/a.rs\n/tmp/old.rs:9";
        let c = extract(text, Path::new("/"), &always);
        assert_eq!(
            c[0],
            ScrapedPath {
                path: "/tmp/old.rs".into(),
                line: Some(9)
            }
        );
        assert_eq!(c[1].path, "/tmp/a.rs");
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn drops_nonexistent() {
        let c = extract("/tmp/ghost.rs", Path::new("/"), &|_| false);
        assert!(c.is_empty());
    }

    #[test]
    fn tilde_expands() {
        std::env::set_var("HOME", "/home/u");
        let c = extract("see ~/notes.md", Path::new("/"), &always);
        assert_eq!(c[0].path, "/home/u/notes.md");
    }

    #[test]
    fn real_pi_fixture_yields_candidates() {
        let text = include_str!("../tests/fixtures/agent_output_pi.txt");
        // existence check: only require extraction to find path-shaped tokens
        let c = extract(text, Path::new("/"), &always);
        assert!(
            !c.is_empty(),
            "no candidates extracted from real agent output"
        );
    }
}
