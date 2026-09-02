//! `pick-file` orchestration — the file bridge tying M1/M2/M3 together.
//!
//! Two phases share one entry point ([`pick_file_cmd`]), selected by argv:
//!
//! * **Phase 1 (action)** — `herdr-nvim pick-file`. Resolves the target agent
//!   pane, reads its recent output, extracts file-path [`Candidate`]s, and (if
//!   any) writes a [`Handoff`] to a temp file and opens the picker popup.
//! * **Phase 2 (finisher)** — `herdr-nvim pick-file --finish <handoff>`. Spawned
//!   detached by the picker once the user picks. Ensures the sidebar is open in
//!   the invoking tab, opens the chosen file in that tab's nvim daemon, focuses
//!   the sidebar, and deletes the handoff file.
//!
//! Only the pure selection rule [`target_agent_pane`] is unit tested; the two
//! phases are exercised live in Task 4's end-to-end run.

use std::{
    collections::HashSet,
    env,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};

use crate::{
    candidates::{self, BuildInput, Candidate},
    config, daemon, extract, gitscan,
    herdr::{self, CliHerdr, Herdr},
    maneuver::{self, Ctx},
    picker::Handoff,
    sessions, state,
};

const PLUGIN_ID: &str = "chmarax.herdr-nvim";

/// Entry point for the `pick-file` subcommand. Dispatches to the finisher when
/// invoked as `pick-file --finish <handoff>`, otherwise runs the action phase.
pub fn pick_file_cmd() -> Result<()> {
    match finish_handoff_path(env::args()) {
        Some(handoff) => finish_pick(&handoff),
        None => start_pick(),
    }
}

/// Extract the handoff path from a `--finish <path>` argument pair, if present.
fn finish_handoff_path(args: impl Iterator<Item = String>) -> Option<String> {
    let mut args = args;
    while let Some(arg) = args.next() {
        if arg == "--finish" {
            return args.next();
        }
    }
    None
}

/// Clamp the configured scrape depth to [`herdr::PaneScroll::cheap_read_limit`],
/// so `pane read` never triggers herdr's slow, user-visible app-scroll
/// fallback (~9s of pi's chat scrolling into the past at the default 300
/// lines). Nothing is lost: deep file history still comes from session mining
/// and git. `None` (older herdr without the scroll block in `pane get`) keeps
/// the configured value unchanged.
fn effective_scan_lines(configured: u32, scroll: Option<herdr::PaneScroll>) -> u32 {
    scroll.map_or(configured, |scroll| {
        configured.min(scroll.cheap_read_limit())
    })
}

/// Selection rule for which agent pane to read.
///
/// * If the currently `focused` pane is itself an agent, use it.
/// * Otherwise prefer an agent in the *same tab* as the focused pane -- when
///   you trigger the picker from the nvim sidebar, the agent you mean is its
///   tab-mate, not some unrelated first agent elsewhere in the workspace (that
///   other agent's cwd would otherwise drive the git/repo-wide file search, so
///   picking it silently searches the wrong repo).
/// * Otherwise fall back to the first agent pane in `workspace`.
/// * If the workspace has no agent panes, error.
pub fn target_agent_pane(
    h: &mut dyn Herdr,
    workspace: &str,
    tab: &str,
    focused: &str,
) -> Result<String> {
    let agents = h.agents(workspace)?;
    if let Some(agent) = agents.iter().find(|agent| agent.pane_id == focused) {
        return Ok(agent.pane_id.clone());
    }
    if let Some(agent) = agents.iter().find(|agent| agent.tab_id == tab) {
        return Ok(agent.pane_id.clone());
    }
    agents
        .into_iter()
        .next()
        .map(|agent| agent.pane_id)
        .with_context(|| format!("no agent panes found in workspace {workspace}"))
}

/// Phase 1: read the target agent pane, gather candidates from the three
/// read-only layers (session mining, git worktree status, scrape), and open
/// the picker.
fn start_pick() -> Result<()> {
    let mut herdr = CliHerdr;
    let ctx = maneuver::read_ctx(&mut herdr)?;
    let config = config::load();

    let target = target_agent_pane(&mut herdr, &ctx.workspace, &ctx.tab, &ctx.focused_pane)?;
    let snapshot = herdr.pane_snapshot(&target)?;
    let lines = effective_scan_lines(config.picker.scan_lines, snapshot.scroll);
    let text = herdr.read_pane(&target, lines)?;

    let cwd = snapshot.cwd;
    let candidates = gather_candidates(snapshot.agent_session.as_ref(), &text, &cwd, &|path| {
        path.is_file()
    });

    if candidates.is_empty() {
        notify_no_candidates();
        return Ok(());
    }

    let handoff = Handoff {
        candidates,
        chosen: None,
        workspace: ctx.workspace.clone(),
        tab: ctx.tab.clone(),
        focused_pane: ctx.focused_pane.clone(),
        cwd: cwd.to_string_lossy().into_owned(),
        max_files: config.picker.max_files,
    };
    let handoff_path = write_handoff(&handoff)?;
    open_picker(&handoff_path)?;
    Ok(())
}

/// Gather candidates from all three layers given the pane's already-fetched
/// `agent_session` (see `Herdr::pane_snapshot`, which folds the `pane get`
/// call this used to make itself into the one `start_pick` already made for
/// the cwd). Never fails: any session-file read error, JSON parse failure,
/// or git command failure degrades to the remaining layers (brief) -- this
/// function's return type is `Vec<Candidate>`, not `Result`, on purpose.
fn gather_candidates(
    agent_session: Option<&herdr::AgentSession>,
    scrape_text: &str,
    cwd: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> Vec<Candidate> {
    let exists_str = |p: &str| {
        let path = Path::new(p);
        if path.is_absolute() {
            exists(path)
        } else {
            exists(&cwd.join(path))
        }
    };

    let session_text = agent_session.and_then(|s| sessions::load_session_text(s, cwd));
    let agent_name = agent_session.map(|s| s.agent.as_str()).unwrap_or("");
    let mined = session_text
        .as_deref()
        .map(|text| sessions::mine_session(agent_name, text))
        .unwrap_or_else(|| sessions::mine_session("", ""));

    let toplevel = gitscan::toplevel(cwd);
    let git_dirty = toplevel
        .as_deref()
        .and_then(|top| gitscan::dirty_paths(top).ok())
        .unwrap_or_default();
    let git_committed = match (&toplevel, mined.first_op_unix) {
        (Some(top), Some(since)) => gitscan::committed_since(top, since).unwrap_or_default(),
        _ => HashSet::new(),
    };
    let in_git_worktree = |path: &str| {
        toplevel
            .as_deref()
            .map(|top| Path::new(path).starts_with(top))
            .unwrap_or(false)
    };
    let git_mtime_unix = |path: &str| {
        std::fs::metadata(path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
    };
    // One bulk `git diff HEAD --numstat` for the whole worktree, not one
    // `git diff`/`git diff --cached` pair per dirty file.
    let diff_stats = toplevel
        .as_deref()
        .and_then(|top| gitscan::diff_numstat_by_path(top).ok())
        .unwrap_or_default();
    // Whole-worktree file list, used only to widen search once the user types
    // (the default view stays session-only). Empty for non-git cwds.
    let repo_files = toplevel
        .as_deref()
        .and_then(|top| gitscan::list_repo_files(top).ok())
        .unwrap_or_default();

    let scraped = extract::extract(scrape_text, cwd, exists);

    candidates::build_candidates(BuildInput {
        mined_touches: &mined.touches,
        first_op_unix: mined.first_op_unix,
        git_dirty: &git_dirty,
        git_committed_in_session: &git_committed,
        in_git_worktree: &in_git_worktree,
        git_mtime_unix: &git_mtime_unix,
        diff_stats: &diff_stats,
        scraped_mentioned: &scraped,
        repo_files: &repo_files,
        exists: &exists_str,
    })
}

/// Phase 2: act on the user's selection from the handoff file.
fn finish_pick(handoff_path: &str) -> Result<()> {
    let raw = std::fs::read_to_string(handoff_path)
        .with_context(|| format!("reading handoff {handoff_path}"))?;
    let handoff: Handoff =
        serde_json::from_str(&raw).with_context(|| format!("parsing handoff {handoff_path}"))?;

    // No selection (overlay dismissed): nothing to do but clean up.
    let Some(chosen) = handoff.chosen else {
        cleanup_handoff(handoff_path);
        return Ok(());
    };
    let candidate = handoff
        .candidates
        .get(chosen)
        .with_context(|| format!("handoff chosen index {chosen} out of range"))?;

    let ctx = Ctx {
        workspace: handoff.workspace.clone(),
        tab: handoff.tab.clone(),
        focused_pane: handoff.focused_pane.clone(),
        // Reuse the same cwd already resolved (and carried in the handoff)
        // for the picker's own path-display purposes -- not a second,
        // independent cwd lookup.
        cwd: PathBuf::from(&handoff.cwd),
    };
    let mut herdr = CliHerdr;

    open_in_sidebar(&mut herdr, &ctx, &candidate.path, candidate.line)?;

    cleanup_handoff(handoff_path);
    Ok(())
}

/// Ensure the sidebar is open in `ctx.tab`, open `path` (optionally at
/// `line`) in that tab's nvim daemon, and focus the sidebar. Shared by the
/// pick-file finisher above and (from a later task) `open-link`'s Ctrl+click
/// handler — one behavior, two entry points.
pub(crate) fn open_in_sidebar(
    h: &mut dyn Herdr,
    ctx: &Ctx,
    path: &str,
    line: Option<u32>,
) -> Result<()> {
    // Bring the tab's daemon up *before* opening the sidebar. The sidebar pane
    // runs its own `ensure_daemon`; doing ours first (to completion) means the
    // sidebar reuses the already-healthy daemon rather than racing to
    // spawn/bind a competing one on the same socket.
    let plugin_root = daemon::plugin_root()?;
    let config = config::load();
    let socket = daemon::ensure_daemon(&ctx.tab, &plugin_root, &config, &ctx.cwd)?;

    let sidebar = ensure_sidebar_open(h, ctx)?;

    open_in_nvim(&socket, path, line, &config.sidebar)?;
    focus_pane(&sidebar);
    Ok(())
}

/// Idempotently ensure a sidebar is open in `ctx.tab`, returning its pane id.
///
/// If state already records a live sidebar for this tab, it is already open —
/// return it untouched (calling `maneuver::toggle` here would *close* it).
/// Otherwise (no state, a dead sidebar, or a mid-open checkpoint) delegate to
/// `maneuver::toggle`, which follows its own open/recover semantics and leaves
/// a fresh sidebar in `ctx.tab`.
fn ensure_sidebar_open(h: &mut dyn Herdr, ctx: &Ctx) -> Result<String> {
    if let Some(sidebar) = maneuver::live_open_sidebar(h, &ctx.tab)? {
        return Ok(sidebar);
    }

    maneuver::toggle(h, ctx)?;
    let opened = state::load(&ctx.tab)?
        .context("sidebar state missing after opening")?
        .sidebar_pane
        .context("sidebar pane id missing after opening")?;
    Ok(opened)
}

/// Open `path` (optionally at `line`) in the headless nvim daemon on `socket`.
///
/// Line jumping is done with `--remote-expr cursor(...)` rather than the tempting
/// `--remote +<line> <path>`: nvim treats `--remote +5` as a *filename* (it opens
/// a buffer literally named `+5` and leaves the cursor on line 1), so `+<line>`
/// only works when *launching* nvim, not against a running server. `--remote-expr`
/// is also mode-independent (works even mid-insert) and needs no path escaping.
fn open_in_nvim(
    socket: &Path,
    path: &str,
    line: Option<u32>,
    sidebar: &crate::config::Sidebar,
) -> Result<()> {
    // Open (or focus) the file. `--remote` takes the path as a clean argv, so a
    // path with spaces needs no escaping.
    let status = daemon::nvim_cmd(sidebar)
        .arg("--server")
        .arg(socket)
        .arg("--remote")
        .arg(path)
        .status()
        .context("failed to run nvim --server --remote")?;
    if !status.success() {
        bail!("nvim --remote failed to open {path}");
    }
    // Jump to the line, if known.
    if let Some(line) = line {
        let status = daemon::nvim_cmd(sidebar)
            .arg("--server")
            .arg(socket)
            .arg("--remote-expr")
            .arg(format!("cursor({line}, 1)"))
            .status()
            .context("failed to run nvim --server --remote-expr")?;
        if !status.success() {
            bail!("nvim --remote-expr failed to jump to line {line} in {path}");
        }
    }
    Ok(())
}

/// Focus the sidebar pane so the user lands in nvim. Best-effort: the file is
/// already loaded in the daemon regardless, so a focus failure is non-fatal.
fn focus_pane(pane: &str) {
    let result = Command::new("herdr")
        .args(["agent", "focus", pane])
        .status();
    match result {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("herdr-nvim: could not focus sidebar pane {pane} (exit {status})"),
        Err(error) => eprintln!("herdr-nvim: could not focus sidebar pane {pane}: {error}"),
    }
}

/// Open the picker as a floating popup pane, passing the handoff path via the
/// environment.
fn open_picker(handoff_path: &str) -> Result<()> {
    let status = Command::new("herdr")
        .args([
            "plugin",
            "pane",
            "open",
            "--plugin",
            PLUGIN_ID,
            "--entrypoint",
            "picker",
            "--placement",
            "popup",
            "--width",
            "80",
            "--height",
            "20",
            "--env",
            &format!("HERDR_NVIM_HANDOFF={handoff_path}"),
        ])
        .status()
        .context("failed to run herdr plugin pane open")?;
    if !status.success() {
        bail!("herdr plugin pane open failed (exit {status})");
    }
    Ok(())
}

/// Show a herdr notification (best-effort, falling back to stderr) that no file
/// paths were found. Callers exit 0 after this — an empty result is not an error.
fn notify_no_candidates() {
    let shown = Command::new("herdr")
        .args([
            "notification",
            "show",
            "herdr-nvim",
            "--body",
            "No file paths found in agent output",
        ])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !shown {
        eprintln!("herdr-nvim: no file paths found in agent output");
    }
}

/// Serialize `handoff` to a uniquely-named temp file, returning its path.
fn write_handoff(handoff: &Handoff) -> Result<String> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let path = env::temp_dir().join(format!(
        "herdr-nvim-handoff-{}-{}.json",
        std::process::id(),
        nanos
    ));
    let encoded = serde_json::to_string(handoff).context("serializing handoff")?;
    std::fs::write(&path, encoded)
        .with_context(|| format!("writing handoff {}", path.display()))?;
    Ok(path.to_string_lossy().into_owned())
}

/// Delete the handoff temp file, ignoring a missing file.
fn cleanup_handoff(handoff_path: &str) {
    match std::fs::remove_file(handoff_path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => eprintln!("herdr-nvim: could not remove handoff {handoff_path}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::herdr::{AgentInfo, MockHerdr};

    fn agent(pane_id: &str, tab_id: &str, focused: bool) -> AgentInfo {
        AgentInfo {
            pane_id: pane_id.into(),
            tab_id: tab_id.into(),
            focused,
        }
    }

    #[test]
    fn focused_pane_that_is_an_agent_is_used() {
        let mut h = MockHerdr {
            agents_results: VecDeque::from([Ok(vec![
                agent("wA:p1", "wA:t1", false),
                agent("wA:p2", "wA:t1", true),
            ])]),
            ..Default::default()
        };
        assert_eq!(
            target_agent_pane(&mut h, "wA", "wA:t1", "wA:p2").unwrap(),
            "wA:p2"
        );
        assert_eq!(h.ops, ["agents wA"]);
    }

    #[test]
    fn non_agent_focus_prefers_agent_in_the_same_tab() {
        let mut h = MockHerdr {
            agents_results: VecDeque::from([Ok(vec![
                // First-in-workspace agent, but in a different tab (e.g. another
                // repo) -- must NOT win over the tab-mate below.
                agent("wA:p1", "wA:t9", false),
                agent("wA:p2", "wA:t1", false),
            ])]),
            ..Default::default()
        };
        // Focused pane wA:sidebar isn't an agent, but it lives in tab wA:t1, so
        // its tab-mate agent wA:p2 is chosen over the first-listed wA:p1.
        assert_eq!(
            target_agent_pane(&mut h, "wA", "wA:t1", "wA:sidebar").unwrap(),
            "wA:p2"
        );
    }

    #[test]
    fn non_agent_focus_falls_back_to_first_agent_when_no_tab_match() {
        let mut h = MockHerdr {
            agents_results: VecDeque::from([Ok(vec![
                agent("wA:p1", "wA:t3", false),
                agent("wA:p2", "wA:t4", false),
            ])]),
            ..Default::default()
        };
        // No agent in the focused pane's tab (wA:t9), so the first agent wins.
        assert_eq!(
            target_agent_pane(&mut h, "wA", "wA:t9", "wA:p9").unwrap(),
            "wA:p1"
        );
    }

    #[test]
    fn no_agents_is_an_error() {
        let mut h = MockHerdr {
            agents_results: VecDeque::from([Ok(vec![])]),
            ..Default::default()
        };
        assert!(target_agent_pane(&mut h, "wA", "wA:t1", "wA:p9").is_err());
    }

    #[test]
    fn effective_scan_lines_clamps_to_viewport_for_alt_screen_panes() {
        // pi pane: no host scrollback, so anything past the viewport would
        // trigger herdr's slow app-scroll fallback.
        let scroll = crate::herdr::PaneScroll {
            viewport_rows: 73,
            max_offset_from_bottom: 0,
        };
        assert_eq!(effective_scan_lines(300, Some(scroll)), 73);
    }

    #[test]
    fn effective_scan_lines_allows_host_scrollback_when_present() {
        let scroll = crate::herdr::PaneScroll {
            viewport_rows: 34,
            max_offset_from_bottom: 3459,
        };
        assert_eq!(effective_scan_lines(300, Some(scroll)), 300);
    }

    #[test]
    fn effective_scan_lines_keeps_configured_value_without_scroll_info() {
        assert_eq!(effective_scan_lines(300, None), 300);
    }

    #[test]
    fn effective_scan_lines_never_raises_a_small_configured_value() {
        let scroll = crate::herdr::PaneScroll {
            viewport_rows: 73,
            max_offset_from_bottom: 500,
        };
        assert_eq!(effective_scan_lines(50, Some(scroll)), 50);
    }

    #[test]
    fn parses_finish_handoff_argument() {
        let args = ["herdr-nvim", "pick-file", "--finish", "/tmp/h.json"]
            .into_iter()
            .map(String::from);
        assert_eq!(finish_handoff_path(args).as_deref(), Some("/tmp/h.json"));
    }

    #[test]
    fn no_finish_argument_is_phase_one() {
        let args = ["herdr-nvim", "pick-file"].into_iter().map(String::from);
        assert!(finish_handoff_path(args).is_none());
    }

    #[test]
    fn gather_candidates_falls_back_to_scrape_when_no_agent_session() {
        let text = "see /tmp/does-not-exist-ever.rs";
        let cwd = std::path::Path::new("/tmp");
        let out = gather_candidates(None, text, cwd, &|_| false);
        assert!(out.is_empty());
    }
}
