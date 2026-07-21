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
use std::process::Command;

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

use crate::extract::Candidate;

/// Environment variable holding the path to the handoff JSON file.
const HANDOFF_ENV: &str = "HERDR_NVIM_HANDOFF";

/// Handoff document exchanged between the bridge and the picker overlay.
#[derive(Serialize, Deserialize)]
pub struct Handoff {
    pub candidates: Vec<Candidate>,
    /// Index into `candidates`; written back by the picker on Enter.
    pub chosen: Option<usize>,
    pub workspace: String,
}

/// Return the indices of `cands` whose path tail (final `/`-separated segment)
/// contains `query` as a case-insensitive substring. An empty query matches
/// every candidate. Pure: no I/O, deterministic, unit tested.
pub fn filter(cands: &[Candidate], query: &str) -> Vec<usize> {
    let needle = query.to_lowercase();
    cands
        .iter()
        .enumerate()
        .filter(|(_, cand)| {
            let tail = cand.path.rsplit('/').next().unwrap_or(cand.path.as_str());
            tail.to_lowercase().contains(&needle)
        })
        .map(|(index, _)| index)
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
            KeyCode::Enter => return Ok(matches.get(cursor).copied()),
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
fn render(cands: &[Candidate], matches: &[usize], cursor: usize, query: &str) -> Result<()> {
    let mut out = io::stdout();
    queue!(out, Clear(ClearType::All), MoveTo(0, 0)).context("clearing screen")?;
    for (row, &index) in matches.iter().enumerate() {
        let marker = if row == cursor { "> " } else { "  " };
        queue!(
            out,
            MoveTo(0, row as u16),
            Print(format!("{marker}{}", cands[index].path)),
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
fn spawn_finish(handoff_path: &str) -> Result<()> {
    let exe = std::env::current_exe().context("resolving current executable")?;
    Command::new(exe)
        .arg("pick-file")
        .arg("--finish")
        .arg(handoff_path)
        .spawn()
        .context("spawning finisher")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(path: &str) -> Candidate {
        Candidate {
            path: path.into(),
            line: None,
        }
    }

    #[test]
    fn filter_matches_tail_case_insensitive() {
        let c = vec![cand("/a/Main.rs"), cand("/a/lib.rs")];
        assert_eq!(filter(&c, "main"), vec![0]);
        assert_eq!(filter(&c, ""), vec![0, 1]);
        assert_eq!(filter(&c, ".rs"), vec![0, 1]);
        assert_eq!(filter(&c, "zzz"), Vec::<usize>::new());
    }
}
