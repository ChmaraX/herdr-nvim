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
    path::Path,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};

use crate::{
    candidates::{self, BuildInput, Candidate, GitOnlyEdit},
    config, daemon, extract, gitscan,
    herdr::{self, CliHerdr, Herdr},
    maneuver::{self, Ctx},
    picker::Handoff,
    sessions,
    state::{self, Phase},
};

const PLUGIN_ID: &str = "adamchmara.herdr-nvim";

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

/// Selection rule for which agent pane to read.
///
/// * If the currently `focused` pane is itself an agent, use it.
/// * Otherwise fall back to the first agent pane in `workspace`.
/// * If the workspace has no agent panes, error.
pub fn target_agent_pane(h: &mut dyn Herdr, workspace: &str, focused: &str) -> Result<String> {
    let agents = h.agents(workspace)?;
    if agents.iter().any(|agent| agent.pane_id == focused) {
        return Ok(focused.to_owned());
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
    let ctx = maneuver::read_ctx()?;
    let config = config::load();
    let mut herdr = CliHerdr;

    let target = target_agent_pane(&mut herdr, &ctx.workspace, &ctx.focused_pane)?;
    let (cwd, agent_session) = herdr.pane_snapshot(&target)?;
    let text = herdr.read_pane(&target, config.picker.scan_lines)?;

    let candidates = gather_candidates(agent_session.as_ref(), &text, &cwd, &|path| path.is_file());

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
    let exists_str = |p: &str| exists(Path::new(p));

    let mined = match agent_session {
        Some(session) if session.kind == "path" => match std::fs::read_to_string(&session.value) {
            Ok(text) => sessions::mine_session(&session.agent, &text),
            Err(_) => sessions::mine_session(&session.agent, ""),
        },
        _ => sessions::mine_session("", ""),
    };

    let toplevel = gitscan::toplevel(cwd);
    let git_dirty = toplevel
        .as_deref()
        .and_then(|top| gitscan::dirty_paths(top).ok())
        .unwrap_or_default();
    let git_committed = match (&toplevel, mined.session_start_unix) {
        (Some(top), Some(since)) => gitscan::committed_since(top, since).unwrap_or_default(),
        _ => HashSet::new(),
    };
    let in_git_worktree = |path: &str| {
        toplevel
            .as_deref()
            .map(|top| Path::new(path).starts_with(top))
            .unwrap_or(false)
    };

    // Git-only edits: dirty paths not already covered by session mining --
    // any touch, read or edit alike, since a path we already have session
    // data for must not also get a synthesized duplicate entry (e.g. a bash
    // `sed -i` the agent ran is invisible to tool-call mining and needs one).
    let mined_paths: HashSet<&str> = mined.touches.iter().map(|t| t.path.as_str()).collect();
    let git_only: Vec<GitOnlyEdit> = git_dirty
        .iter()
        .filter(|p| !mined_paths.contains(p.as_str()))
        .map(|p| GitOnlyEdit {
            path: p.clone(),
            mtime_unix: std::fs::metadata(p)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs()),
        })
        .collect();

    let scraped = extract::extract(scrape_text, cwd, exists);

    let mut candidates = candidates::build_candidates(BuildInput {
        mined_touches: &mined.touches,
        session_start_unix: mined.session_start_unix,
        git_dirty: &git_dirty,
        git_committed_in_session: &git_committed,
        in_git_worktree: &in_git_worktree,
        git_only_dirty_not_mined: &git_only,
        scraped_mentioned: &scraped,
        exists: &exists_str,
    });

    if let Some(top) = &toplevel {
        // One bulk `git diff HEAD --numstat` for the whole worktree, not one
        // `git diff`/`git diff --cached` pair per dirty file.
        let diff_stats = gitscan::diff_numstat_by_path(top).unwrap_or_default();
        for candidate in &mut candidates {
            // Only is_edit, currently-dirty, non-newly-created entries get a
            // diff stat (same eligibility rule as before, now keyed off the
            // flag instead of a section): newly-created files already show
            // the `new` badge; an entry kept only because it was committed
            // during the session is now clean (no diff to show); non-git
            // and net-change-demoted files were never dirty-tracked at all.
            if candidate.is_edit && !candidate.newly_created && git_dirty.contains(&candidate.path)
            {
                if let Some(&(added, removed)) = diff_stats.get(&candidate.path) {
                    if added > 0 || removed > 0 {
                        candidate.diff_stat = Some((added, removed));
                    }
                }
            }
        }
    }

    candidates
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
    let socket = daemon::ensure_daemon(&ctx.tab, &plugin_root, &config)?;

    let sidebar_cmd = daemon::sidebar_shell_cmd(&ctx.tab);
    let sidebar = ensure_sidebar_open(h, ctx, &sidebar_cmd)?;

    open_in_nvim(&socket, path, line)?;
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
fn ensure_sidebar_open(h: &mut dyn Herdr, ctx: &Ctx, sidebar_cmd: &str) -> Result<String> {
    if let Some(existing) = state::load(&ctx.tab)? {
        if matches!(existing.phase, Phase::Open) {
            if let Some(sidebar) = existing.sidebar_pane.clone() {
                if h.pane_alive(&sidebar)? {
                    return Ok(sidebar);
                }
            }
        }
    }

    maneuver::toggle(h, ctx, sidebar_cmd)?;
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
fn open_in_nvim(socket: &Path, path: &str, line: Option<u32>) -> Result<()> {
    // Open (or focus) the file. `--remote` takes the path as a clean argv, so a
    // path with spaces needs no escaping.
    let status = Command::new("nvim")
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
        let status = Command::new("nvim")
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

    fn agent(pane_id: &str, focused: bool) -> AgentInfo {
        AgentInfo {
            pane_id: pane_id.into(),
            focused,
        }
    }

    #[test]
    fn focused_pane_that_is_an_agent_is_used() {
        let mut h = MockHerdr {
            agents_results: VecDeque::from([Ok(vec![agent("wA:p1", false), agent("wA:p2", true)])]),
            ..Default::default()
        };
        assert_eq!(target_agent_pane(&mut h, "wA", "wA:p2").unwrap(), "wA:p2");
        assert_eq!(h.ops, ["agents wA"]);
    }

    #[test]
    fn non_agent_focus_falls_back_to_first_agent() {
        let mut h = MockHerdr {
            agents_results: VecDeque::from([Ok(vec![
                agent("wA:p1", false),
                agent("wA:p2", false),
            ])]),
            ..Default::default()
        };
        // Focused pane wA:p9 is not among the agents, so the first agent wins.
        assert_eq!(target_agent_pane(&mut h, "wA", "wA:p9").unwrap(), "wA:p1");
    }

    #[test]
    fn no_agents_is_an_error() {
        let mut h = MockHerdr {
            agents_results: VecDeque::from([Ok(vec![])]),
            ..Default::default()
        };
        assert!(target_agent_pane(&mut h, "wA", "wA:p9").is_err());
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
