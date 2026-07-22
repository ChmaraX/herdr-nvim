//! Overlay list UI for the file bridge.
//!
//! Reads a handoff JSON file (path in `$HERDR_NVIM_HANDOFF`) written by the
//! bridge, renders a full-screen crossterm overlay of the [`Candidate`]s, lets
//! the user filter/navigate/pick one, writes the chosen index back into the
//! handoff, and spawns a detached finisher. The pure `filter` half is unit
//! tested; the interactive loop is exercised live in a later M3 task.

// Wired into main.rs's subcommand dispatch (and bridge.rs) in a later M3 task.
#![allow(dead_code)]

use std::io::{self, Write};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEventKind},
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use serde::{Deserialize, Serialize};

use crate::candidates::{self, Candidate};

/// Environment variable holding the path to the handoff JSON file.
const HANDOFF_ENV: &str = "HERDR_NVIM_HANDOFF";

/// Handoff document exchanged between the bridge and the picker overlay.
#[derive(Serialize, Deserialize)]
pub struct Handoff {
    pub candidates: Vec<Candidate>,
    /// Index into `candidates`; written back by the picker on Enter.
    pub chosen: Option<usize>,
    pub workspace: String,
    /// Tab the pick-file action was invoked from; the finisher opens/reuses the
    /// sidebar in this tab (phase 2 threads this into a `maneuver::Ctx`).
    pub tab: String,
    /// Pane focused when the action was invoked (used as the `Ctx` focused pane).
    pub focused_pane: String,
    /// Pane's foreground cwd, so the picker can render smart/relative paths.
    pub cwd: String,
}

pub struct FilterMatch {
    pub index: usize,
    pub highlight: Option<(usize, usize)>,
}

/// Return the candidates whose full `path` contains `query` as a
/// case-insensitive substring (whole-path match, not just the filename
/// tail -- v2 behavior per the brief). An empty query matches everything
/// with no highlight. Pure: no I/O, deterministic, unit tested.
pub fn filter(cands: &[Candidate], query: &str) -> Vec<FilterMatch> {
    if query.is_empty() {
        return (0..cands.len())
            .map(|index| FilterMatch {
                index,
                highlight: None,
            })
            .collect();
    }
    let needle = query.to_lowercase();
    cands
        .iter()
        .enumerate()
        .filter_map(|(index, cand)| {
            let lower = cand.path.to_lowercase();
            let start = lower.find(&needle)?;
            Some(FilterMatch {
                index,
                highlight: Some((start, needle.len())),
            })
        })
        .collect()
}

pub struct SmartPath {
    pub dim_prefix: String,
    pub bold_name: String,
}

/// Shorten `path` for display: relative to `cwd` if inside it, `~`-shortened
/// if inside `home`, else left absolute. Splits into a dim directory prefix
/// and a bold filename tail; if the combined length exceeds `max_width`, the
/// *middle* of the prefix is ellipsized (`…`) -- the filename tail is never
/// truncated, since it's the part users scan for.
pub fn smart_path(path: &str, cwd: &str, home: Option<&str>, max_width: usize) -> SmartPath {
    let display_full = if let Some(rest) = path.strip_prefix(&format!("{cwd}/")) {
        rest.to_owned()
    } else if let Some(home) = home {
        if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
            format!("~/{rest}")
        } else {
            path.to_owned()
        }
    } else {
        path.to_owned()
    };

    let (prefix, name) = match display_full.rsplit_once('/') {
        Some((dir, name)) => (format!("{dir}/"), name.to_owned()),
        None => (String::new(), display_full),
    };

    let budget = max_width.saturating_sub(name.chars().count());
    let prefix = if prefix.chars().count() > budget && budget > 1 {
        // Reserve 3 units for the ellipsis: `…` is a single *character* but a
        // 3-byte UTF-8 sequence, and callers (and this fn's own tests) bound
        // the *byte* length of the rendered result, not its char count.
        let keep = budget.saturating_sub(3).max(1);
        let head = keep / 2;
        let tail = keep - head;
        let chars: Vec<char> = prefix.chars().collect();
        if chars.len() > head + tail {
            let head_s: String = chars[..head].iter().collect();
            let tail_s: String = chars[chars.len() - tail..].iter().collect();
            format!("{head_s}…{tail_s}")
        } else {
            prefix
        }
    } else {
        prefix
    };

    SmartPath {
        dim_prefix: prefix,
        bold_name: name,
    }
}

/// Compute the visible window `[first, first+count)` of a `total`-length
/// list given the terminal `viewport_rows` and the current `cursor` index,
/// keeping `cursor` always in view (classic "scroll to keep selection
/// visible" math). Pure function of cursor/height/len -- unit tested.
pub fn scroll_window(cursor: usize, total: usize, viewport_rows: usize) -> (usize, usize) {
    if total <= viewport_rows {
        return (0, total);
    }
    let max_first = total - viewport_rows;
    let first = if cursor < viewport_rows {
        0
    } else {
        cursor + 1 - viewport_rows
    };
    let first = first.min(max_first);
    (first, viewport_rows)
}

/// Entry point for the picker overlay subcommand.
///
/// Reads the handoff at `$HERDR_NVIM_HANDOFF`, renders the overlay, and on a
/// selection writes the chosen index back and spawns a detached finisher
/// (`<self> pick-file --finish <handoff>`). Always returns `Ok(())` so the
/// overlay closes cleanly (Esc/q or empty candidate list just exit).
pub fn picker_cmd() -> Result<()> {
    let handoff_path =
        std::env::var(HANDOFF_ENV).with_context(|| format!("{HANDOFF_ENV} is not set"))?;
    let raw = std::fs::read_to_string(&handoff_path)
        .with_context(|| format!("reading handoff {handoff_path}"))?;
    let mut handoff: Handoff =
        serde_json::from_str(&raw).with_context(|| format!("parsing handoff {handoff_path}"))?;

    let home = std::env::var("HOME").ok();
    if let Some(chosen) = run_overlay(&handoff.candidates, &handoff.cwd, home.as_deref())? {
        handoff.chosen = Some(chosen);
        let encoded = serde_json::to_string(&handoff)?;
        std::fs::write(&handoff_path, encoded)
            .with_context(|| format!("writing handoff {handoff_path}"))?;
        spawn_finish(&handoff_path)?;
    } else {
        // Dismissed (Esc/q, or nothing to pick): no finisher will run to delete
        // the handoff, so remove it here to avoid leaking the temp file.
        let _ = std::fs::remove_file(&handoff_path);
    }
    Ok(())
}

/// Row (within `matches`) to start the cursor on: the first EDITED entry, so
/// Enter with no typing opens the file the agent just worked on. Falls back
/// to row 0 (which may be a MENTIONED entry, or nothing) if there is no
/// EDITED entry in the current match set.
pub fn initial_cursor(matches: &[FilterMatch], cands: &[Candidate]) -> usize {
    matches
        .iter()
        .position(|m| cands[m.index].section == candidates::Section::Edited)
        .unwrap_or(0)
}

/// Restores the terminal (leave alternate screen, show cursor, disable raw
/// mode) on every exit path, including panics and early returns.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().context("enabling raw mode")?;
        execute!(io::stdout(), EnterAlternateScreen, Hide).context("entering alternate screen")?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

/// Run the interactive overlay. Returns `Some(index)` into `cands` on Enter,
/// or `None` if the user dismissed it (Esc/q) or there is nothing to pick.
fn run_overlay(cands: &[Candidate], cwd: &str, home: Option<&str>) -> Result<Option<usize>> {
    if cands.is_empty() {
        return Ok(None);
    }

    let _guard = TerminalGuard::enter()?;
    let mut query = String::new();
    let mut cursor = initial_cursor(&filter(cands, ""), cands);

    loop {
        let matches = filter(cands, &query);
        if cursor >= matches.len() {
            cursor = matches.len().saturating_sub(1);
        }
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 20));
        render(cands, &matches, cursor, &query, cwd, home, cols, rows)?;

        let Event::Key(key) = event::read().context("reading terminal event")? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            KeyCode::Enter => return Ok(matches.get(cursor).map(|m| m.index)),
            KeyCode::Up | KeyCode::Char('k') => cursor = cursor.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                if cursor + 1 < matches.len() {
                    cursor += 1;
                }
            }
            KeyCode::Backspace => {
                query.pop();
                cursor = 0;
            }
            KeyCode::Char(c) => {
                query.push(c);
                cursor = 0;
            }
            _ => {}
        }
    }
}

/// Draw the title bar, sectioned/scrollable/badged match list.
///
/// Layout: row 0 is a title (`touched this session`, centered/bold/dim);
/// row 1 is the filter input (`> {query}`); the last row is a footer
/// (`⏎ open · esc close` ... `matched/total`, right-aligned); the rows
/// between are an ` EDITED`/` MENTIONED` header (only before the first row
/// of a section that actually has visible rows -- an empty section is
/// simply never headered, matching the brief's "empty sections collapse
/// away") followed by that section's candidate rows. `matches` is already
/// EDITED-then-MENTIONED ordered (candidates::build_candidates' invariant,
/// preserved by `filter`), so a single forward scan detects every section
/// boundary.
///
/// Not unit tested: crossterm terminal interaction: exercised live (Task 18).
#[allow(clippy::too_many_arguments)]
fn render(
    cands: &[Candidate],
    matches: &[FilterMatch],
    cursor: usize,
    query: &str,
    cwd: &str,
    home: Option<&str>,
    cols: u16,
    rows: u16,
) -> Result<()> {
    let mut out = io::stdout();
    queue!(out, Clear(ClearType::All), MoveTo(0, 0)).context("clearing screen")?;

    const TITLE: &str = "touched this session";
    let title_pad = (cols as usize).saturating_sub(TITLE.len()) / 2;
    queue!(
        out,
        MoveTo(0, 0),
        SetAttribute(Attribute::Bold),
        SetAttribute(Attribute::Dim),
        Print(format!("{}{TITLE}", " ".repeat(title_pad))),
        SetAttribute(Attribute::Reset)
    )
    .context("drawing title")?;

    queue!(out, MoveTo(0, 1), Print(format!("> {query}"))).context("drawing filter input")?;

    let viewport_rows = (rows as usize).saturating_sub(5).max(1);
    let (first, count) = scroll_window(cursor, matches.len(), viewport_rows);
    let visible = &matches[first..(first + count).min(matches.len())];

    let mut screen_row: u16 = 2;
    let mut last_section: Option<candidates::Section> = None;
    let path_width = (cols as usize).saturating_sub(12).max(4);

    for (offset, m) in visible.iter().enumerate() {
        let row = first + offset;
        let cand = &cands[m.index];
        if last_section != Some(cand.section) {
            let header = match cand.section {
                candidates::Section::Edited => " EDITED",
                candidates::Section::Mentioned => " MENTIONED",
            };
            queue!(out, MoveTo(0, screen_row), Print(header)).context("drawing section header")?;
            screen_row += 1;
            last_section = Some(cand.section);
        }

        let marker = if row == cursor { "> " } else { "  " };
        let sp = smart_path(&cand.path, cwd, home, path_width);

        queue!(out, MoveTo(0, screen_row), Print(marker)).context("drawing row marker")?;

        // The highlight span is a byte range into the raw full path; only
        // apply it when it falls entirely within the displayed filename tail
        // (the common case) -- a match inside the dim/ellipsized prefix is a
        // documented, accepted simplification (see plan Risks): still counts
        // toward matched/total, just renders with no visible highlight.
        let name_start_in_path = cand.path.len().saturating_sub(sp.bold_name.len());
        let name_highlight = m.highlight.and_then(|(start, len)| {
            (start >= name_start_in_path && start + len <= cand.path.len())
                .then(|| (start - name_start_in_path, len))
        });

        queue!(
            out,
            SetAttribute(Attribute::Dim),
            Print(&sp.dim_prefix),
            SetAttribute(Attribute::Reset)
        )
        .context("drawing dim prefix")?;

        match name_highlight {
            Some((hstart, hlen)) if hstart + hlen <= sp.bold_name.len() => {
                let (before, rest) = sp.bold_name.split_at(hstart);
                let (mid, after) = rest.split_at(hlen);
                queue!(
                    out,
                    SetAttribute(Attribute::Bold),
                    Print(before),
                    SetAttribute(Attribute::Reverse),
                    Print(mid),
                    SetAttribute(Attribute::Reset),
                    SetAttribute(Attribute::Bold),
                    Print(after),
                    SetAttribute(Attribute::Reset),
                )
                .context("drawing highlighted filename")?;
            }
            _ => {
                queue!(
                    out,
                    SetAttribute(Attribute::Bold),
                    Print(&sp.bold_name),
                    SetAttribute(Attribute::Reset),
                )
                .context("drawing filename")?;
            }
        }

        let mut suffix = String::new();
        if cand.newly_created {
            suffix.push_str("  new");
        }
        if let Some((added, removed)) = cand.diff_stat {
            suffix.push_str(&format!("  +{added} -{removed}"));
        }
        if !suffix.is_empty() {
            queue!(out, Print(suffix)).context("drawing badge/diff-stat suffix")?;
        }

        screen_row += 1;
    }

    let footer_row = rows.saturating_sub(1);
    let matched = matches.len();
    let total = cands.len();
    let left = "⏎ open · esc close";
    let right = format!("{matched}/{total}");
    let pad = (cols as usize)
        .saturating_sub(left.len() + right.len())
        .max(1);
    queue!(
        out,
        MoveTo(0, footer_row),
        Print(format!("{left}{}{right}", " ".repeat(pad)))
    )
    .context("drawing footer")?;

    out.flush().context("flushing overlay")?;
    Ok(())
}

/// Spawn the detached finisher that acts on the written selection.
///
/// The overlay pane closes the instant this picker process exits, and herdr
/// tears down the pane's whole *session* on close. A plain child would share
/// that session and be killed mid-flight (before it can open the sidebar and
/// load the file). So put the finisher in its own session via setsid(2) — the
/// same rule the nvim daemon relies on to outlive the sidebar pane (see
/// `daemon.rs`) — and detach its stdio from the pane's terminal.
fn spawn_finish(handoff_path: &str) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let exe = std::env::current_exe().context("resolving current executable")?;
    let mut command = Command::new(exe);
    command
        .arg("pick-file")
        .arg("--finish")
        .arg(handoff_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: the pre_exec closure runs post-fork/pre-exec in the child. It only
    // calls setsid(2), which is async-signal-safe and touches no shared memory.
    // A fresh fork is never a process-group leader, so setsid succeeds; we ignore
    // its result either way so a failure can't abort the (valid) exec.
    unsafe {
        command.pre_exec(|| {
            extern "C" {
                fn setsid() -> i32;
            }
            setsid();
            Ok(())
        });
    }
    command.spawn().context("spawning finisher")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(path: &str) -> Candidate {
        Candidate {
            path: path.into(),
            line: None,
            section: crate::candidates::Section::Edited,
            newly_created: false,
            last_edit_unix: None,
            diff_stat: None,
        }
    }

    fn cand_with_section(path: &str, section: crate::candidates::Section) -> Candidate {
        Candidate {
            path: path.into(),
            line: None,
            section,
            newly_created: false,
            last_edit_unix: None,
            diff_stat: None,
        }
    }

    #[test]
    fn initial_cursor_lands_on_first_edited_entry() {
        let cands = vec![
            cand_with_section("/repo/edited.rs", candidates::Section::Edited),
            cand_with_section("/repo/mentioned.rs", candidates::Section::Mentioned),
        ];
        let matches = filter(&cands, "");
        assert_eq!(initial_cursor(&matches, &cands), 0);
    }

    #[test]
    fn initial_cursor_falls_back_to_zero_when_no_edited_entries() {
        let cands = vec![cand_with_section(
            "/repo/mentioned.rs",
            candidates::Section::Mentioned,
        )];
        let matches = filter(&cands, "");
        assert_eq!(initial_cursor(&matches, &cands), 0);
    }

    #[test]
    fn filter_matches_whole_path_not_just_tail() {
        let c = vec![cand("/repo/src/main.rs"), cand("/repo/lib/util.rs")];
        let m = filter(&c, "src");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].index, 0);
    }

    #[test]
    fn filter_is_case_insensitive() {
        let c = vec![cand("/repo/Main.rs")];
        assert_eq!(filter(&c, "MAIN").len(), 1);
    }

    #[test]
    fn empty_query_matches_everything_with_no_highlight() {
        let c = vec![cand("/repo/a.rs"), cand("/repo/b.rs")];
        let m = filter(&c, "");
        assert_eq!(m.len(), 2);
        assert!(m[0].highlight.is_none());
    }

    #[test]
    fn highlight_span_points_at_the_match() {
        let c = vec![cand("/repo/src/main.rs")];
        let m = filter(&c, "main");
        assert_eq!(m[0].highlight, Some((10, 4))); // "main" starts at byte 10 in "/repo/src/main.rs"
    }

    #[test]
    fn no_match_returns_empty() {
        let c = vec![cand("/repo/a.rs")];
        assert!(filter(&c, "zzz").is_empty());
    }

    #[test]
    fn smart_path_relative_to_cwd() {
        let sp = smart_path("/repo/src/main.rs", "/repo", None, 80);
        assert_eq!(sp.dim_prefix, "src/");
        assert_eq!(sp.bold_name, "main.rs");
    }

    #[test]
    fn smart_path_outside_cwd_uses_tilde() {
        let sp = smart_path("/home/u/.config/foo.toml", "/repo", Some("/home/u"), 80);
        assert_eq!(sp.dim_prefix, "~/.config/");
        assert_eq!(sp.bold_name, "foo.toml");
    }

    #[test]
    fn smart_path_outside_cwd_and_home_stays_absolute() {
        let sp = smart_path("/var/log/x.log", "/repo", Some("/home/u"), 80);
        assert_eq!(sp.dim_prefix, "/var/log/");
        assert_eq!(sp.bold_name, "x.log");
    }

    #[test]
    fn smart_path_ellipsizes_long_middle_to_fit_width() {
        let sp = smart_path("/repo/a/b/c/d/e/f/deep/file.rs", "/repo", None, 20);
        let full = format!("{}{}", sp.dim_prefix, sp.bold_name);
        assert!(full.len() <= 20, "got {full:?} ({} bytes)", full.len());
        assert!(full.contains('…'));
        assert!(
            sp.bold_name == "file.rs",
            "filename tail must never be the part elided"
        );
    }

    #[test]
    fn scroll_window_fits_when_total_within_viewport() {
        assert_eq!(scroll_window(0, 5, 10), (0, 5));
    }

    #[test]
    fn scroll_window_keeps_cursor_visible_scrolling_down() {
        // 20 items, 5-row viewport, cursor at 10 -> window must include index 10.
        let (first, count) = scroll_window(10, 20, 5);
        assert!(first <= 10 && 10 < first + count);
        assert_eq!(count, 5);
    }

    #[test]
    fn scroll_window_never_scrolls_past_the_end() {
        let (first, count) = scroll_window(19, 20, 5);
        assert_eq!(first, 15);
        assert_eq!(count, 5);
    }
}
