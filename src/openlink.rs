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

use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::Result;

use crate::{
    bridge,
    extract::{parse_token, resolve},
    herdr::{CliHerdr, Herdr},
    maneuver::Ctx,
};

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

/// Resolve a parsed clicked `path` to a real file on disk: try it directly
/// against `cwd` first (this also covers absolute and `~`-expanded input,
/// since `extract::resolve` only joins `cwd` for genuinely relative paths),
/// then — for relative input only — against `cwd`'s git toplevel (agents
/// often print repo-root-relative paths from inside a subdirectory). Pure
/// aside from the two injected closures; returns `None` if nothing exists.
pub(crate) fn resolve_click(
    path: &str,
    cwd: &Path,
    exists: &dyn Fn(&Path) -> bool,
    git_toplevel: &dyn Fn(&Path) -> Option<PathBuf>,
) -> Option<PathBuf> {
    let direct = resolve(path, cwd);
    if exists(&direct) {
        return Some(direct);
    }
    if path.starts_with('/') || path.starts_with('~') {
        // extract::resolve ignores cwd for absolute/~ input, so retrying
        // against the toplevel would resolve to this exact same (already
        // failed) path — skip the wasted git shell-out.
        return None;
    }
    let toplevel = git_toplevel(cwd)?;
    let via_toplevel = resolve(path, &toplevel);
    exists(&via_toplevel).then_some(via_toplevel)
}

struct ClickEnv {
    clicked_url: String,
    pane: String,
    workspace: String,
    tab: String,
}

/// Read herdr's link-click environment. Any missing var is "bad env" per the
/// module doc — a silent no-op, not an error (a misclick, or a herdr version
/// that doesn't populate one of these, must never surface a popup).
fn read_click_env() -> Option<ClickEnv> {
    Some(ClickEnv {
        clicked_url: env::var("HERDR_PLUGIN_CLICKED_URL").ok()?,
        pane: env::var("HERDR_PANE_ID").ok()?,
        workspace: env::var("HERDR_WORKSPACE_ID").ok()?,
        tab: env::var("HERDR_TAB_ID").ok()?,
    })
}

/// Entry point for the `open-link` subcommand — invoked by herdr on a
/// Ctrl+click of a `file-path`/`file-url` link match (see the
/// `[[link_handlers]]` entries in `herdr-plugin.toml`).
pub fn open_link_cmd() -> Result<()> {
    let Some(click) = read_click_env() else {
        return Ok(());
    };
    let Some((raw_path, line)) = parse_clicked(&click.clicked_url) else {
        return Ok(());
    };

    let mut herdr = CliHerdr;
    let cwd = herdr.pane_cwd(&click.pane)?;
    let Some(resolved) =
        resolve_click(&raw_path, &cwd, &|p| p.is_file(), &crate::gitscan::toplevel)
    else {
        return Ok(());
    };

    let ctx = Ctx {
        workspace: click.workspace,
        tab: click.tab,
        focused_pane: click.pane,
        // Reuse the same cwd already resolved above for path-resolution --
        // not a second, independent cwd lookup.
        cwd: cwd.clone(),
    };
    bridge::open_in_sidebar(&mut herdr, &ctx, &resolved.to_string_lossy(), line)
}

#[cfg(test)]
mod tests {
    use std::env;

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

    #[test]
    fn resolves_directly_against_cwd_when_it_exists() {
        let exists = |p: &Path| p == Path::new("/repo/src/main.rs");
        let toplevel = |_: &Path| panic!("git_toplevel should not be called when cwd resolves");
        let resolved = resolve_click("src/main.rs", Path::new("/repo"), &exists, &toplevel);
        assert_eq!(resolved, Some(PathBuf::from("/repo/src/main.rs")));
    }

    #[test]
    fn falls_back_to_git_toplevel_for_relative_path() {
        let exists = |p: &Path| p == Path::new("/repo/src/main.rs");
        let toplevel = |_: &Path| Some(PathBuf::from("/repo"));
        let resolved = resolve_click(
            "src/main.rs",
            Path::new("/repo/sub/dir"),
            &exists,
            &toplevel,
        );
        assert_eq!(resolved, Some(PathBuf::from("/repo/src/main.rs")));
    }

    #[test]
    fn returns_none_when_neither_cwd_nor_toplevel_has_it() {
        let exists = |_: &Path| false;
        let toplevel = |_: &Path| Some(PathBuf::from("/repo"));
        assert_eq!(
            resolve_click("src/ghost.rs", Path::new("/repo/sub"), &exists, &toplevel),
            None
        );
    }

    #[test]
    fn absolute_path_never_tries_git_toplevel() {
        let exists = |_: &Path| false;
        let toplevel = |_: &Path| panic!("git_toplevel should not be called for absolute paths");
        assert_eq!(
            resolve_click("/tmp/ghost.rs", Path::new("/repo"), &exists, &toplevel),
            None
        );
    }

    #[test]
    fn tilde_path_expands_via_home_before_exists_check() {
        env::set_var("HOME", "/home/u");
        let exists = |p: &Path| p == Path::new("/home/u/notes.md");
        let toplevel = |_: &Path| panic!("git_toplevel should not be called for ~ paths");
        assert_eq!(
            resolve_click("~/notes.md", Path::new("/repo"), &exists, &toplevel),
            Some(PathBuf::from("/home/u/notes.md"))
        );
    }

    struct ClickEnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    static CLICK_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    const CLICK_ENV_VARS: [&str; 4] = [
        "HERDR_PLUGIN_CLICKED_URL",
        "HERDR_PANE_ID",
        "HERDR_WORKSPACE_ID",
        "HERDR_TAB_ID",
    ];

    impl ClickEnvGuard {
        fn new() -> Self {
            let lock = CLICK_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let saved = CLICK_ENV_VARS
                .iter()
                .map(|&key| (key, env::var_os(key)))
                .collect();
            for key in CLICK_ENV_VARS {
                env::remove_var(key);
            }
            Self { _lock: lock, saved }
        }
    }

    impl Drop for ClickEnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(v) => env::set_var(key, v),
                    None => env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn read_click_env_reads_all_four_vars() {
        let _guard = ClickEnvGuard::new();
        env::set_var("HERDR_PLUGIN_CLICKED_URL", "src/main.rs");
        env::set_var("HERDR_PANE_ID", "wA:p1");
        env::set_var("HERDR_WORKSPACE_ID", "wA");
        env::set_var("HERDR_TAB_ID", "wA:t1");

        let click = read_click_env().expect("all vars set");
        assert_eq!(click.clicked_url, "src/main.rs");
        assert_eq!(click.pane, "wA:p1");
        assert_eq!(click.workspace, "wA");
        assert_eq!(click.tab, "wA:t1");
    }

    #[test]
    fn read_click_env_none_when_clicked_url_missing() {
        let _guard = ClickEnvGuard::new();
        env::set_var("HERDR_PANE_ID", "wA:p1");
        env::set_var("HERDR_WORKSPACE_ID", "wA");
        env::set_var("HERDR_TAB_ID", "wA:t1");

        assert!(read_click_env().is_none());
    }

    #[test]
    fn read_click_env_none_when_tab_missing() {
        let _guard = ClickEnvGuard::new();
        env::set_var("HERDR_PLUGIN_CLICKED_URL", "src/main.rs");
        env::set_var("HERDR_PANE_ID", "wA:p1");
        env::set_var("HERDR_WORKSPACE_ID", "wA");

        assert!(read_click_env().is_none());
    }

    // --- manifest link_handlers table-tests -------------------------------
    // The patterns are read out of the real herdr-plugin.toml (include_str!),
    // so the manifest stays the single source of truth — no drift risk.

    fn manifest_link_pattern(id: &str) -> String {
        let raw = include_str!("../herdr-plugin.toml");
        let doc: toml::Value = raw.parse().expect("herdr-plugin.toml must be valid TOML");
        doc.get("link_handlers")
            .and_then(toml::Value::as_array)
            .expect("herdr-plugin.toml must have a link_handlers array")
            .iter()
            .find(|handler| handler.get("id").and_then(toml::Value::as_str) == Some(id))
            .and_then(|handler| handler.get("pattern"))
            .and_then(toml::Value::as_str)
            .unwrap_or_else(|| panic!("no link_handlers entry with id {id}"))
            .to_owned()
    }

    #[test]
    fn file_path_handler_pattern_matches_realistic_paths() {
        let pattern = manifest_link_pattern("file-path");
        let re = regex::Regex::new(&pattern).expect("file-path pattern must compile");

        let should_match = [
            "src/main.rs",
            "./src/bridge.rs:42",
            "/Users/adam/project/src/lib.rs:10:3",
            "components/Button.tsx",
            "app/(marketing)/page.tsx",
            "app/[slug]/page.tsx",
            "~/sub/notes.md",
            // Has a directory segment and an extension, so the *pattern*
            // matches; resolution (not the regex) is what rejects this —
            // see resolve_click, which requires the file to actually exist.
            "example.com/path.js",
        ];
        for candidate in should_match {
            assert!(re.is_match(candidate), "expected {candidate:?} to match");
        }

        let should_not_match = [
            "Node.js",
            "e.g.",
            "v0.7.0",
            "README.md", // bare filename: no directory segment
            "and/or",    // no file extension
            // Known gap inherited from the approved brief: a home-relative
            // path with NO intermediate directory doesn't match, because the
            // pattern requires >=1 char between the optional leading `~` and
            // the mandatory `/` that precedes the final segment. `~/sub/x.md`
            // (above) matches fine; only the bare `~/x.md` shape doesn't.
            "~/notes.md",
        ];
        for candidate in should_not_match {
            assert!(
                !re.is_match(candidate),
                "expected {candidate:?} to NOT match"
            );
        }
    }

    #[test]
    fn file_url_handler_pattern_matches_file_scheme_only() {
        let pattern = manifest_link_pattern("file-url");
        let re = regex::Regex::new(&pattern).expect("file-url pattern must compile");

        assert!(re.is_match("file:///tmp/a.py"));
        assert!(re.is_match("file://localhost/tmp/a.py"));
        assert!(!re.is_match("https://example.com"));
        assert!(!re.is_match("src/main.rs"));
    }

    #[test]
    fn both_link_handlers_route_to_the_open_link_action() {
        let raw = include_str!("../herdr-plugin.toml");
        let doc: toml::Value = raw.parse().unwrap();
        let handlers = doc["link_handlers"].as_array().unwrap();
        assert_eq!(handlers.len(), 2);
        for handler in handlers {
            assert_eq!(handler["action"].as_str(), Some("open-link"));
        }
    }
}
