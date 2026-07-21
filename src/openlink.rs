//! `open-link` — Ctrl+click file paths in agent panes open in the nvim sidebar.
//!
//! herdr linkifies plain text matching the `file-path`/`file-url` patterns in
//! `herdr-plugin.toml`'s `[[link_handlers]]` (checked live against those exact
//! patterns in this module's regex table-test, so there is one source of
//! truth for the patterns: the manifest). Ctrl+click on a match invokes this
//! subcommand with `HERDR_PLUGIN_CLICKED_URL`/`HERDR_PANE_ID`/
//! `HERDR_WORKSPACE_ID`/`HERDR_TAB_ID` in the environment.
//!
//! Flow: parse the clicked text into a `(path, line)` pair (pure,
//! [`parse_clicked`]), resolve it to a real file on disk against the clicked
//! pane's cwd then its git toplevel (pure, `resolve_click` — Task 3), then
//! hand off to the same [`crate::bridge::open_in_sidebar`] the pick-file
//! picker uses (Task 4). Any failure to parse or resolve is a silent no-op
//! (exit 0) — never a popup/notification for a misclick.

use crate::extract::parse_token;

/// Strip trailing sentence punctuation (`.`, `,`) agents' prose often leaves
/// stuck to a path the link regex swept up.
fn strip_trailing_punct(s: &str) -> &str {
    s.trim_end_matches(['.', ','])
}

/// Decode `%XX` percent-escapes (only ones the `file-url` handler's OSC 8
/// links use, e.g. `%20` for a space) into their raw bytes.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 3 <= bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Strip a `file://` prefix (with an optional host component, e.g.
/// `file://localhost/a/b`) and percent-decode the remaining path. Returns
/// `None` for input that isn't a `file://` URL.
fn strip_file_url(clicked: &str) -> Option<String> {
    let rest = clicked.strip_prefix("file://")?;
    let path_part = if rest.starts_with('/') {
        rest
    } else {
        let slash = rest.find('/')?;
        &rest[slash..]
    };
    Some(percent_decode(path_part))
}

/// Parse herdr's clicked link text into a `(path, line)` pair. Pure, no I/O.
///
/// Handles a bare `path[:line[:col]]` token (the `file-path` handler) and a
/// `file://` URL (the `file-url` handler). Trailing sentence punctuation is
/// stripped before parsing. Returns `None` if the cleaned text isn't
/// path-shaped (mirrors `extract::parse_token`'s heuristic: needs a `/` and
/// is absolute/`~`/`./`/has an extension).
pub(crate) fn parse_clicked(text: &str) -> Option<(String, Option<u32>)> {
    let decoded = strip_file_url(text).unwrap_or_else(|| text.to_owned());
    let trimmed = strip_trailing_punct(&decoded);
    let (path, line) = parse_token(trimmed)?;
    Some((path.to_owned(), line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_relative_path_with_line() {
        assert_eq!(
            parse_clicked("src/main.rs:42"),
            Some(("src/main.rs".to_owned(), Some(42)))
        );
    }

    #[test]
    fn plain_path_with_line_and_col_keeps_only_line() {
        assert_eq!(
            parse_clicked("src/main.rs:42:7"),
            Some(("src/main.rs".to_owned(), Some(42)))
        );
    }

    #[test]
    fn tilde_path_left_unexpanded_for_resolve_later() {
        assert_eq!(
            parse_clicked("~/sub/notes.md"),
            Some(("~/sub/notes.md".to_owned(), None))
        );
    }

    #[test]
    fn trailing_period_is_stripped() {
        assert_eq!(
            parse_clicked("src/main.rs."),
            Some(("src/main.rs".to_owned(), None))
        );
    }

    #[test]
    fn trailing_comma_is_stripped() {
        assert_eq!(
            parse_clicked("src/main.rs,"),
            Some(("src/main.rs".to_owned(), None))
        );
    }

    #[test]
    fn bare_filename_without_slash_is_not_path_shaped() {
        assert_eq!(parse_clicked("README.md"), None);
    }

    #[test]
    fn file_url_without_host_strips_prefix() {
        assert_eq!(
            parse_clicked("file:///Users/adam/src/main.rs:10"),
            Some(("/Users/adam/src/main.rs".to_owned(), Some(10)))
        );
    }

    #[test]
    fn file_url_with_host_strips_host_and_prefix() {
        assert_eq!(
            parse_clicked("file://localhost/Users/adam/src/main.rs"),
            Some(("/Users/adam/src/main.rs".to_owned(), None))
        );
    }

    #[test]
    fn file_url_percent_decodes_spaces() {
        assert_eq!(
            parse_clicked("file:///Users/adam/my%20project/main.rs"),
            Some(("/Users/adam/my project/main.rs".to_owned(), None))
        );
    }
}
