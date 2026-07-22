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
    style::Print,
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use serde::{Deserialize, Serialize};

use crate::candidates::Candidate;

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

    if let Some(chosen) = run_overlay(&handoff.candidates)? {
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
fn run_overlay(cands: &[Candidate]) -> Result<Option<usize>> {
    if cands.is_empty() {
        return Ok(None);
    }

    let _guard = TerminalGuard::enter()?;
    let mut query = String::new();
    let mut cursor = 0usize; // row within the current match list

    loop {
        let matches = filter(cands, &query);
        if cursor >= matches.len() {
            cursor = matches.len().saturating_sub(1);
        }
        render(cands, &matches, cursor, &query)?;

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

/// Draw the match list (with a `> ` cursor row) and a `filter: <query>` footer.
fn render(cands: &[Candidate], matches: &[FilterMatch], cursor: usize, query: &str) -> Result<()> {
    let mut out = io::stdout();
    queue!(out, Clear(ClearType::All), MoveTo(0, 0)).context("clearing screen")?;
    for (row, m) in matches.iter().enumerate() {
        let marker = if row == cursor { "> " } else { "  " };
        queue!(
            out,
            MoveTo(0, row as u16),
            Print(format!("{marker}{}", cands[m.index].path)),
        )
        .context("drawing candidate row")?;
    }
    let footer_row = matches.len() as u16 + 1;
    queue!(
        out,
        MoveTo(0, footer_row),
        Print(format!("filter: {query}"))
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
        }
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
}
