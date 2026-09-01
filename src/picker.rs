//! Overlay list UI for the file bridge.
//!
//! Reads a handoff JSON file (path in `$HERDR_NVIM_HANDOFF`) written by the
//! bridge, renders a framed crossterm overlay of a single flat,
//! recency-ordered [`Candidate`] list, lets the user fuzzy-filter/navigate/
//! pick one, writes the chosen index back into the handoff, and spawns a
//! detached finisher.
//!
//! The default (empty-query) view shows only session-touched candidates,
//! capped to `handoff.max_files`; the moment the user types, matching widens
//! to every candidate (including the repo-wide pool the bridge appended) and
//! is ranked by a fuzzy score. The pure halves (`fuzzy_match`, `display_path`,
//! `ellipsize_prefix`, `default_view`, `visible_count`, `scroll_window`,
//! `fmt_age`) are unit tested; the interactive loop and `render` are exercised
//! live.

use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{
        disable_raw_mode, enable_raw_mode, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use serde::{Deserialize, Serialize};

use crate::candidates::Candidate;
use crate::fff::{FffHit, FffIndex};

/// Environment variable holding the path to the handoff JSON file.
const HANDOFF_ENV: &str = "HERDR_NVIM_HANDOFF";

/// The plugin's accent, matching the annotation rail in `lua/herdr-nvim/ui.lua`
/// so the picker reads as part of the same product.
const AMBER: Color = Color::Rgb {
    r: 0xd7,
    g: 0xa6,
    b: 0x5f,
};

/// Handoff document exchanged between the bridge and the picker overlay.
#[derive(Serialize, Deserialize)]
pub struct Handoff {
    pub candidates: Vec<Candidate>,
    /// Index into `candidates`; written back by the picker on Enter.
    pub chosen: Option<usize>,
    pub workspace: String,
    /// Tab the pick-file action was invoked from; the finisher opens/reuses the
    /// sidebar in this tab.
    pub tab: String,
    /// Pane focused when the action was invoked (used as the `Ctx` focused pane).
    pub focused_pane: String,
    /// Pane's foreground cwd, so the picker can render smart/relative paths.
    pub cwd: String,
    /// Max entries the default (empty-query) view shows, from
    /// `config.picker.max_files`.
    pub max_files: u32,
}

/// A candidate that survived filtering, plus how well it matched.
pub struct FilterMatch {
    pub index: usize,
    /// Fuzzy score; higher is better. `0` for empty-query (session) rows.
    pub score: i32,
    /// Byte spans **into the candidate's display path** that matched the query,
    /// already merged over runs of consecutive matched characters. Empty for
    /// the empty-query view.
    pub highlights: Vec<(usize, usize)>,
}

/// Fuzzy-match `needle` against `hay` (a display path), returning `None` if
/// `needle` is not a subsequence of `hay` (case-insensitive), or `Some((score,
/// spans))` where a higher score is a better match and `spans` are byte ranges
/// into `hay` to highlight.
///
/// Scoring rewards, per matched character: runs of *consecutive* matches, a
/// match at a word boundary (start, or after `/ _ - . space`), and a match in
/// the filename tail (after the last `/`) -- so typing `main` ranks
/// `src/main.rs` above `domain/other.rs`. Uses a greedy left-to-right
/// two-pointer walk (complete for subsequence detection; the score is a
/// heuristic over that alignment). Byte offsets come from `char_indices`, so
/// every span boundary is a valid char boundary even under Unicode case
/// folding that changes byte length (e.g. the Kelvin sign).
pub fn fuzzy_match(hay: &str, needle: &str) -> Option<(i32, Vec<(usize, usize)>)> {
    if needle.is_empty() {
        return Some((0, Vec::new()));
    }
    let hay_chars: Vec<(usize, char)> = hay.char_indices().collect();
    let needle_lower: Vec<char> = needle.chars().flat_map(|c| c.to_lowercase()).collect();
    let name_start = hay.rfind('/').map(|i| i + 1).unwrap_or(0);

    let mut ni = 0usize;
    let mut score = 0i32;
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut prev_matched_hi: Option<usize> = None;

    for hi in 0..hay_chars.len() {
        if ni >= needle_lower.len() {
            break;
        }
        let (byte, ch) = hay_chars[hi];
        if ch.to_lowercase().next() != Some(needle_lower[ni]) {
            continue;
        }
        let end = hay_chars.get(hi + 1).map_or(hay.len(), |&(b, _)| b);

        let consecutive = prev_matched_hi == Some(hi.wrapping_sub(1)) && hi > 0;
        // Consecutive outweighs the word-boundary bonus below so an exact
        // contiguous run (`abc` in `abc.rs`) beats a boundary-scattered one
        // (`a_b_c.rs`).
        score += if consecutive { 12 } else { 1 };
        let boundary = byte == 0
            || matches!(
                hay[..byte].chars().next_back(),
                Some('/' | '_' | '-' | '.' | ' ')
            );
        if boundary {
            score += 10;
        }
        if byte >= name_start {
            score += 3;
        }

        match spans.last_mut() {
            Some(last) if last.1 == byte => last.1 = end,
            _ => spans.push((byte, end)),
        }
        prev_matched_hi = Some(hi);
        ni += 1;
    }

    if ni == needle_lower.len() {
        // Prefer tighter matches: a small penalty for noisier (longer) paths.
        score -= (hay_chars.len() as i32) / 40;
        Some((score, spans))
    } else {
        None
    }
}

/// Display form of an absolute `path`: relative to `cwd` if inside it,
/// `~`-shortened if inside `home`, else absolute. No width handling -- the
/// renderer ellipsizes the directory prefix to fit. Pure, unit tested.
pub fn display_path(path: &str, cwd: &str, home: Option<&str>) -> String {
    if let Some(rest) = path.strip_prefix(&format!("{cwd}/")) {
        rest.to_owned()
    } else if let Some(rest) = home.and_then(|h| path.strip_prefix(&format!("{h}/"))) {
        format!("~/{rest}")
    } else {
        path.to_owned()
    }
}

/// Shorten a directory `prefix` (the part before the filename) to at most
/// `budget` columns by ellipsizing its *middle* with `…`, so both the top of
/// the tree and the immediate parent dir stay visible. Returns `prefix`
/// unchanged when it already fits. Pure, unit tested.
pub fn ellipsize_prefix(prefix: &str, budget: usize) -> String {
    let chars: Vec<char> = prefix.chars().collect();
    if chars.len() <= budget || budget <= 1 {
        return prefix.to_owned();
    }
    let keep = budget.saturating_sub(1).max(1); // reserve one column for `…`
    let head = keep / 2;
    let tail = keep - head;
    if chars.len() > head + tail {
        let head_s: String = chars[..head].iter().collect();
        let tail_s: String = chars[chars.len() - tail..].iter().collect();
        format!("{head_s}…{tail_s}")
    } else {
        prefix.to_owned()
    }
}

/// Human relative age of `then` as of `now` (both Unix seconds): `now`, `5m`,
/// `3h`, `2d`. Pure, unit tested.
pub fn fmt_age(now: u64, then: u64) -> String {
    let secs = now.saturating_sub(then);
    if secs < 60 {
        "now".to_owned()
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// The default (empty-query) view: only session candidates, in their given
/// (recency) order, unscored. Pure, unit tested.
pub fn default_view(cands: &[Candidate]) -> Vec<FilterMatch> {
    cands
        .iter()
        .enumerate()
        .filter(|(_, c)| c.session)
        .map(|(index, _)| FilterMatch {
            index,
            score: 0,
            highlights: Vec::new(),
        })
        .collect()
}

/// Fuzzy-rank only the session (agent-touched) candidates for `query`.
/// Always the first tier shown; the fff backend supplies the fallback tier.
pub fn session_tier_matches(
    cands: &[Candidate],
    displays: &[String],
    query: &str,
) -> Vec<FilterMatch> {
    let mut matches: Vec<FilterMatch> = displays
        .iter()
        .enumerate()
        .filter(|(index, _)| cands[*index].session)
        .filter_map(|(index, display)| {
            fuzzy_match(display, query).map(|(score, highlights)| FilterMatch {
                index,
                score,
                highlights,
            })
        })
        .collect();
    matches.sort_by(|a, b| b.score.cmp(&a.score).then(a.index.cmp(&b.index)));
    matches
}

/// Append the fff repo-wide tier after the session tier, resolving each hit
/// to its candidate index by path. Hits already in the session tier are
/// dropped (agent tier wins); hits outside the candidate pool are skipped
/// (the handoff protocol is index-based). Highlight spans are kept only when
/// they land on char boundaries of the display string, otherwise dropped.
pub fn merge_fff_tier(
    cands: &[Candidate],
    displays: &[String],
    session: Vec<FilterMatch>,
    fff_hits: Vec<FffHit>,
) -> Vec<FilterMatch> {
    use std::collections::{HashMap, HashSet};

    let by_path: HashMap<&str, usize> = cands
        .iter()
        .enumerate()
        .map(|(i, c)| (c.path.as_str(), i))
        .collect();
    let taken: HashSet<usize> = session.iter().map(|m| m.index).collect();

    let mut out = session;
    for hit in fff_hits {
        let Some(&index) = by_path.get(hit.path.as_str()) else {
            continue;
        };
        if taken.contains(&index) {
            continue;
        }
        let display = &displays[index];
        let spans_ok = hit.highlights.iter().all(|&(s, e)| {
            s < e
                && e <= display.len()
                && display.is_char_boundary(s)
                && display.is_char_boundary(e)
        });
        out.push(FilterMatch {
            index,
            score: 0,
            highlights: if spans_ok { hit.highlights } else { Vec::new() },
        });
    }
    out
}

/// Ranked matches for a non-empty `query`: session candidates first, then
/// the fff backend's repo-wide results appended and deduped.
pub fn compute_matches_fff(
    cands: &[Candidate],
    displays: &[String],
    query: &str,
    fff: &FffIndex,
) -> Vec<FilterMatch> {
    let session = session_tier_matches(cands, displays, query);
    merge_fff_tier(cands, displays, session, fff.search(query))
}

/// How many matches the view shows: capped to `max_files` when `query` is
/// empty (the session default view), or every match once the user types. Pure,
/// unit tested.
pub fn visible_count(total_matches: usize, query: &str, max_files: u32) -> usize {
    if query.is_empty() {
        total_matches.min(max_files as usize)
    } else {
        total_matches
    }
}

/// Compute the visible window `[first, first+count)` of a `total`-length list
/// given the terminal `viewport_rows` and the current `cursor` index, keeping
/// `cursor` in view. Pure, unit tested.
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
pub fn picker_cmd() -> Result<()> {
    let handoff_path =
        std::env::var(HANDOFF_ENV).with_context(|| format!("{HANDOFF_ENV} is not set"))?;
    let raw = std::fs::read_to_string(&handoff_path)
        .with_context(|| format!("reading handoff {handoff_path}"))?;
    let mut handoff: Handoff =
        serde_json::from_str(&raw).with_context(|| format!("parsing handoff {handoff_path}"))?;

    let home = std::env::var("HOME").ok();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // `None` on init failure leaves the fuzzy fallback tier empty.
    let config = crate::config::load();
    let fff = FffIndex::open(&handoff.cwd, config.picker.frecency);
    if fff.is_none() {
        eprintln!("herdr-nvim: fff index failed to initialize; fuzzy fallback tier is empty");
    }
    if let Some(chosen) = run_overlay(
        &handoff.candidates,
        &handoff.cwd,
        home.as_deref(),
        handoff.max_files,
        now,
        fff.as_ref(),
    )? {
        handoff.chosen = Some(chosen);
        let encoded = serde_json::to_string(&handoff)?;
        std::fs::write(&handoff_path, encoded)
            .with_context(|| format!("writing handoff {handoff_path}"))?;
        spawn_finish(&handoff_path)?;
    } else {
        // Dismissed: no finisher will run to delete the handoff, so remove it
        // here to avoid leaking the temp file.
        let _ = std::fs::remove_file(&handoff_path);
    }
    Ok(())
}

/// Restores the terminal on every exit path, including panics and early
/// returns.
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
/// or `None` if the user dismissed it (Esc/Ctrl-c) or there is nothing to pick.
fn run_overlay(
    cands: &[Candidate],
    cwd: &str,
    home: Option<&str>,
    max_files: u32,
    now: u64,
    fff: Option<&FffIndex>,
) -> Result<Option<usize>> {
    if cands.is_empty() {
        return Ok(None);
    }
    // Display paths depend only on cwd/home, not the query -- compute once.
    let displays: Vec<String> = cands
        .iter()
        .map(|c| display_path(&c.path, cwd, home))
        .collect();
    let session_total = cands.iter().filter(|c| c.session).count();

    let _guard = TerminalGuard::enter()?;
    let mut query = String::new();
    let mut cursor = 0usize;

    loop {
        let matches = match (fff, query.is_empty()) {
            (_, true) => default_view(cands),
            (Some(index), false) => compute_matches_fff(cands, &displays, &query, index),
            (None, false) => session_tier_matches(cands, &displays, &query),
        };
        let shown = visible_count(matches.len(), &query, max_files);
        let visible = &matches[..shown];
        if cursor >= visible.len() {
            cursor = visible.len().saturating_sub(1);
        }
        let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 20));
        let viewport = viewport_rows(rows);
        render(
            cands,
            &displays,
            visible,
            cursor,
            &query,
            session_total,
            now,
            cols,
            rows,
        )?;

        let Event::Key(key) = event::read().context("reading terminal event")? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last = visible.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Enter => return Ok(visible.get(cursor).map(|m| m.index)),
            KeyCode::Up => cursor = cursor.saturating_sub(1),
            KeyCode::Down => cursor = (cursor + 1).min(last),
            KeyCode::PageUp => cursor = cursor.saturating_sub(viewport),
            KeyCode::PageDown => cursor = (cursor + viewport).min(last),
            KeyCode::Home => cursor = 0,
            KeyCode::End => cursor = last,
            KeyCode::Backspace => {
                query.pop();
                cursor = 0;
            }
            // Ctrl chords: navigation + editing that never reach the query, so
            // every *printable* key below is free to be typed (no more `q`/`j`/
            // `k` swallowed by nav).
            KeyCode::Char('c') if ctrl => return Ok(None),
            KeyCode::Char('p') if ctrl => cursor = cursor.saturating_sub(1),
            KeyCode::Char('n') if ctrl => cursor = (cursor + 1).min(last),
            KeyCode::Char('a') if ctrl => cursor = 0,
            KeyCode::Char('e') if ctrl => cursor = last,
            KeyCode::Char('u') if ctrl => {
                query.clear();
                cursor = 0;
            }
            KeyCode::Char(_) if ctrl => {}
            KeyCode::Char(c) => {
                query.push(c);
                cursor = 0;
            }
            _ => {}
        }
    }
}

/// Columns of blank inset on the left and right of the content, so it doesn't
/// sit flush against Herdr's popup frame.
const PAD_X: u16 = 2;

/// The chrome rows the list must leave room for: a blank top-pad row, the query
/// line, a blank gap row, the keybind-hint line, and a blank bottom-pad row.
const CHROME_ROWS: u16 = 5;

/// First screen row of the candidate list (top pad + query + gap = rows 0,1,2).
const LIST_TOP: u16 = 3;

/// Number of candidate rows that fit once the top/query/gap and hint/bottom-pad
/// chrome is subtracted (Herdr's popup draws the surrounding frame and title,
/// so the picker itself is borderless).
fn viewport_rows(rows: u16) -> usize {
    (rows.saturating_sub(CHROME_ROWS)).max(1) as usize
}

/// One styled run of text in the right-hand metadata cluster.
struct Seg {
    text: String,
    color: Option<Color>,
    dim: bool,
}

/// Draw the borderless overlay: a query line up top, the scrollable candidate
/// rows, and a keybind-hint line at the bottom. The surrounding frame and the
/// "open file" title come from Herdr's popup chrome, so the picker draws no box
/// or title of its own (that used to double up). Not unit tested: crossterm
/// interaction, exercised live.
#[allow(clippy::too_many_arguments)]
fn render(
    cands: &[Candidate],
    displays: &[String],
    visible: &[FilterMatch],
    cursor: usize,
    query: &str,
    session_total: usize,
    now: u64,
    cols: u16,
    rows: u16,
) -> Result<()> {
    let mut out = io::stdout();
    queue!(out, Clear(ClearType::All)).context("clearing screen")?;

    let cols = cols.max(8) as usize;
    // Drawable width between the left and right insets.
    let width = cols.saturating_sub(2 * PAD_X as usize);

    // Query line (row 1: one blank top-pad row above it), with a dim count
    // right-aligned.
    let count = if query.is_empty() {
        format!("{session_total} files")
    } else {
        format!("{} matches", visible.len())
    };
    let prompt = format!("› {query}");
    queue!(
        out,
        MoveTo(PAD_X, 1),
        SetForegroundColor(AMBER),
        Print("› "),
        ResetColor,
        Print(query)
    )
    .context("query line")?;
    let count_col = cols.saturating_sub(count.chars().count() + PAD_X as usize);
    if count_col > prompt.chars().count() + PAD_X as usize {
        queue!(
            out,
            MoveTo(count_col as u16, 1),
            SetAttribute(Attribute::Dim),
            Print(&count),
            SetAttribute(Attribute::Reset)
        )
        .context("count label")?;
    }

    // Candidate rows, starting below the query + a blank gap row.
    let viewport = viewport_rows(rows);
    let (first, count_rows) = scroll_window(cursor, visible.len(), viewport);
    for screen in 0..viewport {
        let row = LIST_TOP + screen as u16;
        if screen < count_rows {
            let m = &visible[first + screen];
            queue!(out, MoveTo(PAD_X, row)).context("row move")?;
            draw_candidate(
                &mut out,
                cands,
                displays,
                m,
                first + screen == cursor,
                now,
                width,
            )?;
        }
    }

    // Keybind hint on the second-to-last row (one blank bottom-pad row below).
    let hint = "↑↓ move  ⏎ open  ^u clear  esc close";
    queue!(
        out,
        MoveTo(PAD_X, rows.saturating_sub(2)),
        SetAttribute(Attribute::Dim),
        Print(hint),
        SetAttribute(Attribute::Reset)
    )
    .context("hint line")?;

    out.flush().context("flushing overlay")?;
    Ok(())
}

/// Draw one candidate row: a selection bar, the (highlighted, ellipsized) path
/// on the left, and a right-aligned metadata cluster (diff stat / `new` badge,
/// then relative age). Assumes the cursor is already at column 0 of the row.
fn draw_candidate(
    out: &mut impl Write,
    cands: &[Candidate],
    displays: &[String],
    m: &FilterMatch,
    selected: bool,
    now: u64,
    width: usize,
) -> Result<()> {
    let cand = &cands[m.index];
    let display = &displays[m.index];

    // Selection bar: amber ▌ + space, or two blanks.
    if selected {
        queue!(
            out,
            SetForegroundColor(AMBER),
            Print('▌'),
            ResetColor,
            Print(' ')
        )
        .context("selection bar")?;
    } else {
        queue!(out, Print("  ")).context("row indent")?;
    }

    // Region after the 2-column bar, shared by path + right-aligned metadata.
    let region = width.saturating_sub(2);

    // Build the right-hand metadata cluster.
    let mut segs: Vec<Seg> = Vec::new();
    if let Some((added, removed)) = cand.diff_stat {
        segs.push(Seg {
            text: format!("+{added}"),
            color: Some(Color::Green),
            dim: false,
        });
        segs.push(Seg {
            text: " ".to_owned(),
            color: None,
            dim: false,
        });
        segs.push(Seg {
            text: format!("-{removed}"),
            color: Some(Color::Red),
            dim: false,
        });
    } else if cand.newly_created {
        segs.push(Seg {
            text: "new".to_owned(),
            color: Some(AMBER),
            dim: false,
        });
    }
    if let Some(ts) = cand.touched_unix {
        if !segs.is_empty() {
            segs.push(Seg {
                text: "  ".to_owned(),
                color: None,
                dim: false,
            });
        }
        segs.push(Seg {
            text: fmt_age(now, ts),
            color: None,
            dim: true,
        });
    }
    let cluster_w: usize = segs.iter().map(|s| s.text.chars().count()).sum();

    // Path gets whatever the cluster doesn't need, minus a one-column gap.
    let path_budget = region.saturating_sub(cluster_w + 1).max(1);
    let path_w = draw_path(out, display, &m.highlights, selected, path_budget)?;

    // Fill the gap so the cluster sits flush right, then draw it.
    let gap = region.saturating_sub(path_w + cluster_w);
    queue!(out, Print(" ".repeat(gap))).context("cluster gap")?;
    for seg in &segs {
        if seg.dim {
            queue!(out, SetAttribute(Attribute::Dim)).context("seg dim")?;
        }
        if let Some(color) = seg.color {
            queue!(out, SetForegroundColor(color)).context("seg color")?;
        }
        queue!(out, Print(&seg.text)).context("seg text")?;
        queue!(out, ResetColor, SetAttribute(Attribute::Reset)).context("seg reset")?;
    }
    Ok(())
}

/// Draw `display` into at most `budget` columns: dim directory prefix (middle-
/// ellipsized if needed) + filename tail, with fuzzy-match `highlights`
/// (amber+bold) applied. Returns the number of columns actually drawn.
fn draw_path(
    out: &mut impl Write,
    display: &str,
    highlights: &[(usize, usize)],
    selected: bool,
    budget: usize,
) -> Result<usize> {
    let name_start = display.rfind('/').map(|i| i + 1).unwrap_or(0);
    let width = display.chars().count();

    if width <= budget {
        draw_spans(out, display, 0, highlights, name_start, selected)?;
        return Ok(width);
    }
    // Too wide: ellipsize the prefix; the filename tail is never truncated.
    let prefix = &display[..name_start];
    let name = &display[name_start..];
    let name_w = name.chars().count();
    let prefix_budget = budget.saturating_sub(name_w).max(1);
    let ellipsized = ellipsize_prefix(prefix, prefix_budget);
    // Ellipsized prefix loses its byte<->span mapping, so render it plainly dim
    // (highlights inside an elided directory are dropped -- an accepted
    // simplification; the filename tail keeps its highlights).
    queue!(
        out,
        SetAttribute(Attribute::Dim),
        Print(&ellipsized),
        SetAttribute(Attribute::Reset)
    )
    .context("dim ellipsized prefix")?;
    draw_spans(out, name, name_start, highlights, name_start, selected)?;
    Ok(ellipsized.chars().count() + name_w)
}

/// Print `text` char-by-char, applying styles: matched chars (per `highlights`,
/// whose byte ranges are in the *full display path* offset by `base`) render
/// amber + bold; unmatched chars in the directory prefix (before `name_start`)
/// render dim; unmatched filename chars render bold when `selected`, else
/// plain.
fn draw_spans(
    out: &mut impl Write,
    text: &str,
    base: usize,
    highlights: &[(usize, usize)],
    name_start: usize,
    selected: bool,
) -> Result<()> {
    for (local, ch) in text.char_indices() {
        let global = base + local;
        let matched = highlights.iter().any(|&(s, e)| global >= s && global < e);
        let in_name = global >= name_start;

        if matched {
            queue!(
                out,
                SetForegroundColor(AMBER),
                SetAttribute(Attribute::Bold)
            )
            .context("hl on")?;
        } else if !in_name {
            queue!(out, SetAttribute(Attribute::Dim)).context("dim on")?;
        } else if selected {
            queue!(out, SetAttribute(Attribute::Bold)).context("bold on")?;
        }
        queue!(out, Print(ch)).context("path char")?;
        if matched || !in_name || selected {
            queue!(out, ResetColor, SetAttribute(Attribute::Reset)).context("style reset")?;
        }
    }
    Ok(())
}

/// Spawn the detached finisher that acts on the written selection.
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
            is_edit: false,
            newly_created: false,
            session: true,
            touched_unix: None,
            diff_stat: None,
        }
    }

    fn repo_cand(path: &str) -> Candidate {
        Candidate {
            session: false,
            ..cand(path)
        }
    }

    fn displays_of(cands: &[Candidate], cwd: &str) -> Vec<String> {
        cands
            .iter()
            .map(|c| display_path(&c.path, cwd, None))
            .collect()
    }

    // --- fuzzy_match ------------------------------------------------------

    #[test]
    fn fuzzy_matches_subsequence_not_just_substring() {
        // "mnrs" is a subsequence of "main.rs" but not a substring.
        assert!(fuzzy_match("src/main.rs", "mnrs").is_some());
    }

    #[test]
    fn fuzzy_rejects_out_of_order() {
        assert!(fuzzy_match("src/main.rs", "rsmain").is_none());
    }

    #[test]
    fn fuzzy_is_case_insensitive() {
        assert!(fuzzy_match("src/Main.rs", "MAIN").is_some());
    }

    #[test]
    fn fuzzy_empty_needle_matches_with_no_spans() {
        let (score, spans) = fuzzy_match("anything", "").unwrap();
        assert_eq!(score, 0);
        assert!(spans.is_empty());
    }

    #[test]
    fn fuzzy_prefers_filename_over_directory_match() {
        // "main" appears in the filename of A and the directory of B; A wins.
        let a = fuzzy_match("src/main.rs", "main").unwrap().0;
        let b = fuzzy_match("main/util.rs", "main").unwrap().0;
        assert!(a > b, "filename match ({a}) should outrank dir match ({b})");
    }

    #[test]
    fn fuzzy_prefers_consecutive_over_scattered() {
        let consecutive = fuzzy_match("abc.rs", "abc").unwrap().0;
        let scattered = fuzzy_match("a_b_c.rs", "abc").unwrap().0;
        assert!(consecutive > scattered);
    }

    #[test]
    fn fuzzy_spans_are_char_boundaries_under_case_folding() {
        // U+212A KELVIN SIGN is 3 bytes, lowercases to 1-byte 'k'.
        let path = "/\u{212A}main.rs";
        let (_, spans) = fuzzy_match(path, "kmain").expect("kelvin should match");
        for (s, e) in spans {
            assert!(path.is_char_boundary(s));
            assert!(path.is_char_boundary(e));
        }
    }

    // --- default_view / session_tier_matches -------------------------------

    #[test]
    fn empty_query_shows_only_session_candidates() {
        let cands = vec![cand("/r/a.rs"), repo_cand("/r/b.rs")];
        let m = default_view(&cands);
        assert_eq!(m.len(), 1, "repo-only candidates hidden from default view");
        assert_eq!(m[0].index, 0);
    }

    #[test]
    fn matches_sorted_by_score_desc() {
        let cands = vec![cand("/r/domain/other.rs"), cand("/r/src/main.rs")];
        let displays = displays_of(&cands, "/r");
        let m = session_tier_matches(&cands, &displays, "main");
        // main.rs (filename hit) must rank before domain/ (dir hit).
        assert_eq!(m[0].index, 1);
    }

    #[test]
    fn equal_score_breaks_toward_lower_index_recency() {
        let cands = vec![cand("/r/a/x.rs"), cand("/r/b/x.rs")];
        let displays = displays_of(&cands, "/r");
        let m = session_tier_matches(&cands, &displays, "x.rs");
        assert_eq!(m[0].index, 0, "ties keep original (recency) order");
    }

    // --- display_path / ellipsize_prefix ---------------------------------

    #[test]
    fn display_path_relative_to_cwd() {
        assert_eq!(
            display_path("/repo/src/main.rs", "/repo", None),
            "src/main.rs"
        );
    }

    #[test]
    fn display_path_uses_tilde_for_home() {
        assert_eq!(
            display_path("/home/u/.config/foo.toml", "/repo", Some("/home/u")),
            "~/.config/foo.toml"
        );
    }

    #[test]
    fn display_path_absolute_when_outside_cwd_and_home() {
        assert_eq!(
            display_path("/var/log/x.log", "/repo", Some("/home/u")),
            "/var/log/x.log"
        );
    }

    #[test]
    fn ellipsize_prefix_keeps_head_and_tail() {
        let out = ellipsize_prefix("a/b/c/d/e/f/", 6);
        assert!(out.chars().count() <= 6, "got {out:?}");
        assert!(out.contains('…'));
    }

    #[test]
    fn ellipsize_prefix_leaves_short_prefix_untouched() {
        assert_eq!(ellipsize_prefix("src/", 20), "src/");
    }

    // --- fmt_age ----------------------------------------------------------

    #[test]
    fn fmt_age_buckets() {
        assert_eq!(fmt_age(100, 100), "now");
        assert_eq!(fmt_age(100 + 30, 100), "now");
        assert_eq!(fmt_age(100 + 120, 100), "2m");
        assert_eq!(fmt_age(100 + 3 * 3600, 100), "3h");
        assert_eq!(fmt_age(100 + 2 * 86_400, 100), "2d");
    }

    // --- visible_count / scroll_window ------------------------------------

    #[test]
    fn visible_count_caps_default_view_to_max_files() {
        assert_eq!(visible_count(50, "", 20), 20);
    }

    #[test]
    fn visible_count_uncapped_when_query_is_non_empty() {
        assert_eq!(visible_count(50, "main", 20), 50);
    }

    #[test]
    fn visible_count_never_exceeds_actual_match_count() {
        assert_eq!(visible_count(5, "", 20), 5);
    }

    #[test]
    fn scroll_window_fits_when_total_within_viewport() {
        assert_eq!(scroll_window(0, 5, 10), (0, 5));
    }

    #[test]
    fn scroll_window_keeps_cursor_visible_scrolling_down() {
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

    // --- merge_fff_tier ---------------------------------------------------

    fn hit(path: &str, highlights: Vec<(usize, usize)>) -> FffHit {
        FffHit {
            path: path.into(),
            highlights,
        }
    }

    #[test]
    fn fff_tier_appends_after_session_tier() {
        let cands = vec![cand("/r/touched.rs"), repo_cand("/r/repo_only.rs")];
        let displays = displays_of(&cands, "/r");
        let session = session_tier_matches(&cands, &displays, "rs");
        assert_eq!(session.len(), 1, "session tier only ranks session cands");
        let merged = merge_fff_tier(
            &cands,
            &displays,
            session,
            vec![hit("/r/repo_only.rs", vec![(0, 2)])],
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].index, 0, "agent tier always first");
        assert_eq!(merged[1].index, 1);
    }

    #[test]
    fn fff_hits_for_session_matched_paths_are_deduped() {
        let cands = vec![cand("/r/touched.rs")];
        let displays = displays_of(&cands, "/r");
        let session = session_tier_matches(&cands, &displays, "touched");
        let merged = merge_fff_tier(
            &cands,
            &displays,
            session,
            vec![hit("/r/touched.rs", vec![(0, 7)])],
        );
        assert_eq!(merged.len(), 1, "no duplicate row for an agent-tier path");
        assert_eq!(merged[0].index, 0);
    }

    #[test]
    fn fff_hit_rescues_session_path_the_builtin_matcher_missed() {
        // The built-in matcher can't do multi-term; fff can.
        let cands = vec![cand("/r/cargo/config.toml")];
        let displays = displays_of(&cands, "/r");
        let session = session_tier_matches(&cands, &displays, "cargo toml");
        assert!(session.is_empty(), "builtin matcher can't do multi-term");
        let merged = merge_fff_tier(
            &cands,
            &displays,
            session,
            vec![hit("/r/cargo/config.toml", vec![(0, 5)])],
        );
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].index, 0);
    }

    #[test]
    fn fff_hits_outside_the_candidate_pool_are_skipped() {
        let cands = vec![cand("/r/a.rs")];
        let displays = displays_of(&cands, "/r");
        let merged = merge_fff_tier(
            &cands,
            &displays,
            Vec::new(),
            vec![hit("/r/not_in_handoff.rs", vec![])],
        );
        assert!(merged.is_empty(), "index-based handoff can't represent it");
    }

    #[test]
    fn fff_bad_highlight_spans_are_dropped_not_rendered() {
        let cands = vec![repo_cand("/r/a.rs")];
        let displays = displays_of(&cands, "/r"); // display "a.rs", 4 bytes
        let merged = merge_fff_tier(
            &cands,
            &displays,
            Vec::new(),
            vec![hit("/r/a.rs", vec![(0, 99)])],
        );
        assert_eq!(merged.len(), 1);
        assert!(
            merged[0].highlights.is_empty(),
            "out-of-range spans dropped, row kept"
        );
    }

    /// Indexes this repo through the real fff backend and checks the tier
    /// contract: agent tier first, results resolved to candidate indices, deduped.
    #[test]
    fn fff_end_to_end_over_this_repo() {
        let cwd = env!("CARGO_MANIFEST_DIR").to_owned();
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&cwd)
            .arg("ls-files")
            .output()
            .expect("git ls-files");
        // Session tier: picker.rs itself. Repo tier: everything tracked.
        let mut cands = vec![cand(&format!("{cwd}/src/picker.rs"))];
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let abs = format!("{cwd}/{line}");
            if abs != cands[0].path {
                cands.push(repo_cand(&abs));
            }
        }
        let displays = displays_of(&cands, &cwd);
        let fff = FffIndex::open(&cwd, true).expect("fff index should open");

        let matches = compute_matches_fff(&cands, &displays, "picker", &fff);
        assert!(!matches.is_empty(), "fff found nothing for 'picker'");
        assert_eq!(
            matches[0].index, 0,
            "agent-touched src/picker.rs must rank first"
        );
        let mut seen = std::collections::HashSet::new();
        for m in &matches {
            assert!(seen.insert(m.index), "duplicate candidate in results");
        }

        // Multi-term query the built-in matcher can't do at all.
        let multi = compute_matches_fff(&cands, &displays, "cargo toml", &fff);
        assert!(
            multi.iter().any(|m| displays[m.index] == "Cargo.toml"),
            "multi-term 'cargo toml' should surface Cargo.toml via fff"
        );
    }
}
