//! `pick-file` orchestration — the file bridge tying M1/M2/M3 together.
//!
//! Two phases share one entry point ([`pick_file_cmd`]), selected by argv:
//!
//! * **Phase 1 (action)** — `herdr-nvim pick-file`. Resolves the target agent
//!   pane, reads its recent output, extracts file-path [`Candidate`]s, and (if
//!   any) writes a [`Handoff`] to a temp file and opens the picker popup.
//! * **Phase 2 (finisher)** — `herdr-nvim pick-file --finish <handoff>`. Spawned
//!   detached by the picker once the user picks. Ensures the sidebar is open in
//!   the invoking tab, opens the first chosen file in that tab's nvim daemon
//!   (adding any others to its buffer list), focuses the sidebar, and deletes
//!   the handoff file.
//!
//! Only the pure selection rule [`target_agent_pane`] and the [`badd_expr`]
//! builder are unit tested; the two phases are exercised live in Task 4's
//! end-to-end run.

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
    pathkey,
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
        chosen: Vec::new(),
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
    // No session text mines nothing, like an agent without a parser.
    let mined =
        sessions::mine_session(agent_name, session_text.as_deref().unwrap_or(""), &|path| {
            pathkey::resolve(path, cwd)
        });

    // git paths are `<toplevel>/<path>`; canonical so they match the mined
    // and scraped paths below.
    let toplevel = gitscan::toplevel(cwd).map(|top| pathkey::canonical(&top));
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

    let mut scraped = extract::extract(scrape_text, cwd, exists);
    for mention in &mut scraped {
        // Already absolute; canonical so it matches the git and mined paths.
        mention.path = pathkey::canonical(Path::new(&mention.path))
            .to_string_lossy()
            .into_owned();
    }

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
    let Some((&chosen, rest)) = handoff.chosen.split_first() else {
        cleanup_handoff(handoff_path);
        return Ok(());
    };
    let candidate_at = |index: usize| {
        handoff
            .candidates
            .get(index)
            .with_context(|| format!("handoff chosen index {index} out of range"))
    };
    let candidate = candidate_at(chosen)?;
    let background: Vec<_> = rest
        .iter()
        .filter_map(|&index| match candidate_at(index) {
            Ok(c) => Some((c.path.clone(), c.line)),
            Err(error) => {
                eprintln!("herdr-nvim: {error:#}");
                None
            }
        })
        .collect();

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

    open_in_sidebar(
        &mut herdr,
        &ctx,
        &candidate.path,
        candidate.line,
        &background,
    )?;

    cleanup_handoff(handoff_path);
    Ok(())
}

/// Ensure the sidebar is open in `ctx.tab`, open `path` (optionally at
/// `line`) in that tab's nvim daemon, and focus the sidebar. `background`
/// paths (each with an optional line) are then added to the buffer list
/// without being shown; that step is best-effort, like the focus. Shared by
/// the pick-file finisher above and (from a later task) `open-link`'s
/// Ctrl+click handler — one behavior, two entry points.
pub(crate) fn open_in_sidebar(
    h: &mut dyn Herdr,
    ctx: &Ctx,
    path: &str,
    line: Option<u32>,
    background: &[(String, Option<u32>)],
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
    if let Err(error) = add_buffers(&socket, background, &config.sidebar) {
        eprintln!("herdr-nvim: {error:#}");
    }
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
        .arg("--headless")
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
            .arg("--headless")
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

/// Runs each command in its own `pcall`, so one failure (say, an exception
/// from a user's `BufAdd` autocmd) never stops the rest, and returns the
/// failures' messages joined by newlines, or "" when every command ran.
const PCALL_EACH_LUA: &str = "(function(cmds) local failed = {} for _, c in ipairs(cmds) do local ok, err = pcall(vim.cmd, c) if not ok then table.insert(failed, err) end end return table.concat(failed, \"\\n\") end)(_A)";

/// Expression that runs `:badd [+line] <path>` for every path through
/// [`PCALL_EACH_LUA`]. `:badd` lists a buffer without reading the file (nvim
/// reads it on first visit, landing on `line`) and skips paths already
/// listed, so files the daemon already has open are left alone. Paths go in
/// as Vimscript string literals and through `fnameescape()`, so spaces and
/// specials need no escaping here. Pure, unit tested.
fn badd_expr(paths: &[(String, Option<u32>)]) -> String {
    let commands = paths
        .iter()
        .map(|(path, line)| {
            let line = line.map(|line| format!("+{line} ")).unwrap_or_default();
            format!(
                "'badd {line}' .. fnameescape({})",
                daemon::vim_string_literal(path)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "luaeval({}, [{commands}])",
        daemon::vim_string_literal(PCALL_EACH_LUA)
    )
}

/// Add `paths` to the daemon's buffer list in one round trip, without changing
/// the current window. A no-op for an empty list. Errors name every path nvim
/// rejected; the others are still added.
fn add_buffers(
    socket: &Path,
    paths: &[(String, Option<u32>)],
    sidebar: &crate::config::Sidebar,
) -> Result<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let failed = daemon::remote_expr(socket, &badd_expr(paths), sidebar)
        .with_context(|| format!("failed to add {} buffers to the sidebar", paths.len()))?;
    if !failed.is_empty() {
        bail!("some buffers were not added to the sidebar:\n{failed}");
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
    use crate::config::Config;
    use crate::herdr::{AgentInfo, MockHerdr};
    use crate::test_support::{nvim_available, TestDaemon, TestEnv};

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
    fn badd_expr_quotes_every_path_and_keeps_lines() {
        assert_eq!(
            badd_expr(&[
                ("/r/a.rs".into(), None),
                ("/r/it's here.md".into(), Some(12))
            ]),
            format!(
                "luaeval({}, ['badd ' .. fnameescape('/r/a.rs'), 'badd +12 ' .. fnameescape('/r/it''s here.md')])",
                daemon::vim_string_literal(PCALL_EACH_LUA)
            )
        );
    }

    #[test]
    fn add_buffers_keeps_going_past_a_failing_path_and_reports_it() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let env = TestEnv::new();
        let config = Config::default();
        let daemon = TestDaemon::spawn("w41:t1", &config);
        let paths: Vec<(String, Option<u32>)> = ["broken.rs", "fine.rs"]
            .iter()
            .map(|name| {
                let path = env.dir.join(name);
                std::fs::write(&path, "").unwrap();
                (path.to_string_lossy().into_owned(), None)
            })
            .collect();
        daemon.eval(r#"execute('autocmd BufAdd */broken.rs throw "boom"')"#);

        let error = add_buffers(&daemon.socket, &paths, &config.sidebar).unwrap_err();

        assert!(format!("{error:#}").contains("boom"), "{error:#}");
        assert_eq!(
            daemon.eval(&format!(
                "buflisted(bufnr({}))",
                daemon::vim_string_literal(&paths[1].0)
            )),
            "1",
            "fine.rs must still be added"
        );
    }

    #[test]
    fn add_buffers_lists_missing_files_and_keeps_the_shown_one() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let env = TestEnv::new();
        let config = Config::default();
        let daemon = TestDaemon::spawn("w40:t1", &config);
        let paths: Vec<(String, Option<u32>)> = [
            ("shown.rs", None),
            ("with space's.rs", None),
            ("fresh.rs", Some(2)),
        ]
        .iter()
        .map(|&(name, line)| {
            let path = env.dir.join(name);
            std::fs::write(&path, "one\ntwo\nthree\n").unwrap();
            (path.to_string_lossy().into_owned(), line)
        })
        .collect();
        let shown = daemon::vim_string_literal(&paths[0].0);
        let fresh = daemon::vim_string_literal(&paths[2].0);

        daemon.eval(&format!(
            "execute('noswapfile edit ' .. fnameescape({shown}))"
        ));
        add_buffers(&daemon.socket, &paths, &config.sidebar).unwrap();
        add_buffers(&daemon.socket, &paths, &config.sidebar).unwrap();

        // By tail: buffer names may resolve the temp dir's symlinks (macOS).
        for name in ["shown.rs", "with space''s.rs", "fresh.rs"] {
            assert_eq!(
                daemon.eval(&format!(
                    "len(filter(getbufinfo({{'buflisted': 1}}), {{_, b -> fnamemodify(b.name, ':t') ==# '{name}'}}))"
                )),
                "1",
                "{name} must be listed exactly once"
            );
        }
        assert_eq!(daemon.eval("expand('%:t')"), "shown.rs");
        assert_eq!(daemon.eval(&format!("bufloaded({fresh})")), "0");

        daemon.eval(&format!("execute('noswapfile buffer ' .. bufnr({fresh}))"));
        assert_eq!(daemon.eval("line('.')"), "2");
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

    /// A throwaway git repo with `src/greet.js` (edited, uncommitted) and
    /// `src/notes.md` (clean). Deleted on drop.
    struct Repo {
        root: PathBuf,
        path: PathBuf,
    }

    impl Repo {
        fn new(name: &str) -> Self {
            let root = env::temp_dir().join(format!("hn-gather-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let path = root.join("repo");
            std::fs::create_dir_all(path.join("src")).unwrap();
            let git = |args: &[&str]| {
                // Ignore the developer's git config (commit signing, hooks).
                let out = std::process::Command::new("git")
                    // Git for Windows maps `/dev/null` to `NUL` itself.
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .arg("-C")
                    .arg(&path)
                    .args(args)
                    .output()
                    .unwrap();
                assert!(out.status.success(), "git {args:?}: {out:?}");
            };
            git(&["init", "-q"]);
            std::fs::write(path.join("src/greet.js"), "hello\n").unwrap();
            std::fs::write(path.join("src/notes.md"), "notes\n").unwrap();
            git(&["add", "."]);
            git(&[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "init",
            ]);
            std::fs::write(path.join("src/greet.js"), "hello\nbye\n").unwrap();
            // git reports the toplevel's real path, so expectations use it too
            // (macOS temp dirs sit behind the /var -> /private/var symlink).
            let path = pathkey::canonical(&path);
            Self { root, path }
        }

        /// `rel` (written with `/`) under the repo, spelled the way the
        /// picker spells it: native separators, like `gitscan` produces.
        fn file(&self, rel: &str) -> String {
            let native = rel.replace('/', std::path::MAIN_SEPARATOR_STR);
            self.path.join(native).to_string_lossy().into_owned()
        }

        /// A pi session file whose assistant turns call `(tool, path)` in order.
        fn pi_session(&self, cwd: &Path, calls: &[(&str, &str)]) -> crate::herdr::AgentSession {
            let header = serde_json::json!({
                "type": "session", "version": 3, "id": "s",
                "timestamp": "2026-07-22T11:07:49.944Z", "cwd": cwd,
            });
            let mut text = format!("{header}\n");
            for (i, (tool, path)) in calls.iter().enumerate() {
                let record = serde_json::json!({
                    "type": "message", "id": format!("m{i}"), "parentId": null,
                    "timestamp": format!("2026-07-22T11:08:0{i}.000Z"),
                    "message": {"role": "assistant", "content": [{
                        "type": "toolCall", "id": format!("t{i}"), "name": tool,
                        "arguments": {"path": path},
                    }]},
                });
                text.push_str(&format!("{record}\n"));
            }
            let file = self.root.join("session.jsonl");
            std::fs::write(&file, text).unwrap();
            crate::herdr::AgentSession {
                agent: "pi".into(),
                kind: "path".into(),
                value: file.to_string_lossy().into_owned(),
            }
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// The picker's default view: (path, is_edit, diff_stat, line) per row.
    fn session_rows(
        session: Option<&crate::herdr::AgentSession>,
        scrape: &str,
        cwd: &Path,
    ) -> Vec<(String, bool, Option<(u32, u32)>, Option<u32>)> {
        gather_candidates(session, scrape, cwd, &|path| path.exists())
            .into_iter()
            .filter(|c| c.session)
            .map(|c| (c.path, c.is_edit, c.diff_stat, c.line))
            .collect()
    }

    /// One file on disk is one picker row, whichever way the agent spelled
    /// its path. git always reports `<toplevel>/<path>`; pi logs the path the
    /// model wrote, which pi itself resolves against its cwd after dropping a
    /// leading `@`.
    #[test]
    fn gather_candidates_lists_an_agent_edited_file_once() {
        let repo = Repo::new("spellings");
        let greet = repo.file("src/greet.js");
        let src = repo.path.join("src");
        let cases: [(&str, &Path, Vec<(&str, &str)>); 5] = [
            ("relative", &repo.path, vec![("edit", "src/greet.js")]),
            ("dot-relative", &repo.path, vec![("edit", "./src/greet.js")]),
            ("dot-dot", &src, vec![("edit", "../src/greet.js")]),
            ("at-prefixed", &repo.path, vec![("edit", "@src/greet.js")]),
            (
                "read absolute, edit relative",
                &repo.path,
                vec![("read", greet.as_str()), ("edit", "src/greet.js")],
            ),
        ];
        for (name, cwd, calls) in cases {
            let session = repo.pi_session(cwd, &calls);
            assert_eq!(
                session_rows(Some(&session), "", cwd),
                vec![(greet.clone(), true, Some((1, 0)), None)],
                "{name}"
            );
        }
    }

    /// A file the agent only read (clean in git) is listed under its real path.
    #[test]
    fn gather_candidates_keeps_a_read_only_touch_whatever_its_spelling() {
        let repo = Repo::new("read-only");
        let session = repo.pi_session(&repo.path, &[("read", "@src/notes.md")]);
        let rows = session_rows(Some(&session), "", &repo.path);
        assert!(
            rows.iter()
                .any(|(path, ..)| *path == repo.file("src/notes.md")),
            "{rows:?}"
        );
    }

    /// The pane's cwd reached through a symlinked directory still names the
    /// same files git reports under the real toplevel.
    #[cfg(unix)]
    #[test]
    fn gather_candidates_dedupes_paths_under_a_symlinked_cwd() {
        let repo = Repo::new("symlink");
        let link = repo.root.join("link");
        std::os::unix::fs::symlink(&repo.path, &link).unwrap();
        let session = repo.pi_session(&link, &[("edit", "src/greet.js")]);
        assert_eq!(
            session_rows(Some(&session), "", &link),
            vec![(repo.file("src/greet.js"), true, Some((1, 0)), None)]
        );
    }

    /// Without an agent session (a plain shell pane) the scraped mention and
    /// git's dirty entry are the same file: one row, keeping the line number.
    #[test]
    fn gather_candidates_merges_a_scraped_mention_into_the_git_row() {
        let repo = Repo::new("scrape");
        assert_eq!(
            session_rows(None, "changed src/greet.js:2 just now", &repo.path),
            vec![(repo.file("src/greet.js"), true, Some((1, 0)), Some(2))]
        );
    }
}
