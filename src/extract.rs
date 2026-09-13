//! Pure path extraction from agent-pane output.
//!
//! Reads a raw terminal pane capture (newest lines last), strips TUI chrome,
//! and returns the file paths mentioned in it as [`ScrapedPath`]s ordered
//! newest-first. This module performs no I/O of its own: existence is checked
//! through an injected closure so it stays deterministic and testable.

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
    c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | '\\' | '~' | '@' | ':')
}

/// Trim surrounding punctuation (quotes, brackets, commas, …) from a token.
fn trim_edges(s: &str) -> &str {
    s.trim_matches(|c: char| !is_path_char(c))
}

/// Peel one trailing `:<digits>` group off `s`, returning the remainder and the
/// parsed number, or `None` if the tail after the last `:` is not all digits.
fn strip_trailing_number(s: &str) -> Option<(&str, u32)> {
    let colon = s.rfind(':')?;
    let tail = &s[colon + 1..];
    if tail.is_empty() || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((&s[..colon], tail.parse().ok()?))
}

/// Decide whether `tok` is path-shaped and split off any trailing line number.
///
/// Returns `(path, line)` on success. Applies an extension-or-slash heuristic so
/// prose like `and/or` is ignored while `src/main.rs` is kept.
pub(crate) fn parse_token(tok: &str) -> Option<(&str, Option<u32>)> {
    // A `scheme://` token is a URL, not a file path (`file://` links are handled
    // by the open-link handler, other URLs go to the browser). The old
    // split-on-first-colon parse rejected these implicitly; the right-to-left
    // parse below would otherwise accept `http://host/x.rs` as a path.
    if tok.contains("://") {
        return None;
    }
    // Peel numeric suffixes from the right (`path:line:col` -> `path`, line) so a
    // Windows drive prefix such as `C:` is never mistaken for a line separator.
    let (mut path, mut line) = (tok, None);
    if let Some((rest, first)) = strip_trailing_number(path) {
        if let Some((rest2, real_line)) = strip_trailing_number(rest) {
            // `path:line:col` — `first` was the column, keep the line.
            path = rest2;
            line = Some(real_line);
        } else {
            // `path:line` — `first` was the line.
            path = rest;
            line = Some(first);
        }
    }

    // Must look like a path: contain a separator and either be absolute /
    // home-relative, an explicit `./`|`../` reference, or carry a file
    // extension in its final segment.
    if !path.contains('/') && !path.contains('\\') {
        return None;
    }
    let is_abs = Path::new(path).is_absolute() || path.starts_with('~');
    let is_dotslash = path.starts_with("./")
        || path.starts_with("../")
        || path.starts_with(r".\")
        || path.starts_with(r"..\");
    let last = path.rsplit(|c| c == '/' || c == '\\').next().unwrap_or("");
    let has_ext = last.contains('.');
    if !(is_abs || is_dotslash || has_ext) {
        return None;
    }

    Some((path, line))
}

/// Resolve a path token to an absolute, lexically-normalized [`PathBuf`].
pub(crate) fn resolve(path: &str, cwd: &Path) -> PathBuf {
    let expanded = if let Some(rest) = path.strip_prefix('~') {
        let home = std::env::var("HOME").unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    fn always(_: &Path) -> bool {
        true
    }

    fn normalized(path: &str) -> String {
        #[cfg(windows)]
        let path = path.replace('/', "\\");
        #[cfg(not(windows))]
        let path = path.to_owned();
        Path::new(&path).to_string_lossy().into_owned()
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
                path: normalized("/tmp/a/b.rs"),
                line: Some(42)
            }]
        );
    }

    #[test]
    fn resolves_relative_against_cwd() {
        let c = extract("modified src/main.rs", Path::new("/repo"), &always);
        assert_eq!(c[0].path, normalized("/repo/src/main.rs"));
    }

    #[test]
    fn strips_box_chrome() {
        let c = extract("│ ● /tmp/x.py │", Path::new("/"), &always);
        assert_eq!(c[0].path, normalized("/tmp/x.py"));
    }

    #[test]
    fn dedupes_keeping_newest_and_orders_newest_first() {
        let text = "/tmp/old.rs\n/tmp/a.rs\n/tmp/old.rs:9";
        let c = extract(text, Path::new("/"), &always);
        assert_eq!(
            c[0],
            ScrapedPath {
                path: normalized("/tmp/old.rs"),
                line: Some(9)
            }
        );
        assert_eq!(c[1].path, normalized("/tmp/a.rs"));
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn drops_nonexistent() {
        let c = extract("/tmp/ghost.rs", Path::new("/"), &|_| false);
        assert!(c.is_empty());
    }

    #[test]
    fn ignores_scheme_urls() {
        // A bare http(s) URL is not a file path candidate, even though its final
        // segment carries an extension.
        assert_eq!(parse_token("http://foo/bar.rs"), None);
        assert_eq!(parse_token("https://example.com/a/b.py:12"), None);
        // A real path is still parsed, including line:col.
        assert_eq!(
            parse_token("src/main.rs:42:7"),
            Some(("src/main.rs", Some(42)))
        );
        assert_eq!(
            parse_token("src/main.rs:42"),
            Some(("src/main.rs", Some(42)))
        );
        assert_eq!(parse_token("src/main.rs"), Some(("src/main.rs", None)));
    }

    #[test]
    fn tilde_expands() {
        std::env::set_var("HOME", "/home/u");
        let c = extract("see ~/notes.md", Path::new("/"), &always);
        assert_eq!(c[0].path, normalized("/home/u/notes.md"));
    }

    #[cfg(windows)]
    #[test]
    fn extracts_windows_drive_path_with_line() {
        let c = extract(
            r"changed C:\repo\src\main.rs:42",
            Path::new(r"C:\repo"),
            &always,
        );
        assert_eq!(
            c[0].path,
            Path::new(r"C:\repo\src\main.rs").to_string_lossy()
        );
        assert_eq!(c[0].line, Some(42));
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
