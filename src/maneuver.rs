use std::{env, path::PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::{
    config::SidebarPosition,
    daemon,
    herdr::{CliHerdr, Dir, Herdr},
    layout::plan_rebuild,
    state::{self, Phase, StateFile},
};

pub struct Ctx {
    pub workspace: String,
    pub tab: String,
    pub focused_pane: String,
    /// The workspace's actual working directory, used to spawn the nvim
    /// daemon in the right place (see `daemon::spawn_daemon`) instead of
    /// wherever the herdr-nvim binary process happened to inherit as its own
    /// cwd.
    pub cwd: PathBuf,
}

/// Read the invoking plugin context, resolving `cwd` in priority order:
/// `workspace_cwd` from the context JSON, then `focused_pane_cwd` from the
/// same JSON, then (only if a very old herdr populated neither field) a
/// live `pane get` call via `h` as a last resort.
pub fn read_ctx(h: &mut dyn Herdr) -> Result<Ctx> {
    let workspace = env::var("HERDR_WORKSPACE_ID").context("HERDR_WORKSPACE_ID is not set")?;
    let focused_pane = env::var("HERDR_PANE_ID").context("HERDR_PANE_ID is not set")?;
    let plugin_context =
        env::var("HERDR_PLUGIN_CONTEXT_JSON").context("HERDR_PLUGIN_CONTEXT_JSON is not set")?;
    let plugin_context: Value =
        serde_json::from_str(&plugin_context).context("invalid HERDR_PLUGIN_CONTEXT_JSON")?;
    let tab = tab_id(&plugin_context).context("plugin context is missing pane tab_id")?;

    let cwd = match cwd_from_context(&plugin_context) {
        Some(cwd) => cwd,
        None => h.pane_cwd(&focused_pane)?,
    };

    Ok(Ctx {
        workspace,
        tab: tab.to_owned(),
        focused_pane,
        cwd,
    })
}

fn tab_id(context: &Value) -> Option<&str> {
    context
        .get("tab_id")
        .and_then(Value::as_str)
        .or_else(|| context.pointer("/pane/tab_id").and_then(Value::as_str))
        .or_else(|| {
            context
                .pointer("/focused_pane/tab_id")
                .and_then(Value::as_str)
        })
}

/// Pure: extract the workspace's cwd from the plugin invocation context
/// JSON -- prefer the top-level `workspace_cwd` string field, fall back to
/// `focused_pane_cwd` (both per herdr's PluginInvocationContext schema).
/// `None` if neither is present (a very old herdr) -- `read_ctx` falls back
/// to a live `pane get` call in that case.
fn cwd_from_context(context: &Value) -> Option<PathBuf> {
    context
        .get("workspace_cwd")
        .and_then(Value::as_str)
        .or_else(|| context.get("focused_pane_cwd").and_then(Value::as_str))
        .map(PathBuf::from)
}

pub fn toggle(h: &mut dyn Herdr, ctx: &Ctx, sidebar_cmd: &str) -> Result<()> {
    // Opportunistic, best-effort gc: reap stale per-tab daemons left behind by
    // closed tabs. Non-fatal -- a gc failure must never block a toggle.
    let config = crate::config::load();
    let _ = daemon::gc(h, &config.sidebar.nvim_bin);

    // Only a fully-Open sidebar is eligible for the fast close-and-return
    // path. A sidebar_pane recorded while phase is still Evacuating is a
    // mid-open checkpoint (see `open()`): closing it here and removing state
    // directly would strand any still-parked panes and destroy the recovery
    // info recover() needs to put them back. Evacuating state always falls
    // through to recover() below instead, which already knows how to close
    // an orphaned sidebar AND replay parked moves.
    if let Some(sidebar) = live_open_sidebar(h, &ctx.tab)? {
        h.close_pane(&sidebar)?;
        state::remove(&ctx.tab)?;
        return Ok(());
    }

    recover(h, &ctx.tab)?;
    open(h, ctx, sidebar_cmd, config.sidebar.position)
}

/// The pane id of `tab`'s sidebar if state records it as fully Open and its
/// pane is still alive; `None` otherwise (no state, a dead sidebar, or a
/// mid-open Evacuating checkpoint). Shared by `toggle`'s fast close-and-return
/// path and `bridge::ensure_sidebar_open`'s idempotent open check.
pub(crate) fn live_open_sidebar(h: &mut dyn Herdr, tab: &str) -> Result<Option<String>> {
    let Some(existing) = state::load(tab)? else {
        return Ok(None);
    };
    if !matches!(existing.phase, Phase::Open) {
        return Ok(None);
    }
    let Some(sidebar) = existing.sidebar_pane else {
        return Ok(None);
    };
    if h.pane_alive(&sidebar)? {
        Ok(Some(sidebar))
    } else {
        Ok(None)
    }
}

fn open(h: &mut dyn Herdr, ctx: &Ctx, sidebar_cmd: &str, position: SidebarPosition) -> Result<()> {
    let rects = h.pane_rects(&ctx.tab)?;
    let plan = plan_rebuild(&rects)?;
    let mut state_file = StateFile {
        phase: Phase::Open,
        workspace: ctx.workspace.clone(),
        tab: ctx.tab.clone(),
        anchor: plan.anchor.clone(),
        parking_tab: None,
        parked: vec![],
        plan_steps: plan.steps.clone(),
        sidebar_pane: None,
    };

    let mut parking = if rects.len() > 1 {
        let (parking_tab, placeholder) = h.create_tab(&ctx.workspace)?;
        state_file.phase = Phase::Evacuating;
        state_file.parking_tab = Some(parking_tab.clone());
        state_file.parked = rects
            .iter()
            .filter(|rect| rect.pane_id != plan.anchor)
            .map(|rect| rect.pane_id.clone())
            .collect();
        state::save(&state_file)?;

        for pane in &state_file.parked {
            h.move_pane(pane, &parking_tab, Dir::Right, None, None, false)?;
        }
        Some((parking_tab, placeholder))
    } else {
        None
    };

    // herdr only supports creating splits to the right or down. For a left
    // or top sidebar, create the inverse split and then move the original
    // anchor to the other side of the new sidebar.
    let split_dir = match position {
        SidebarPosition::Left | SidebarPosition::Right => Dir::Right,
        SidebarPosition::Top | SidebarPosition::Bottom => Dir::Down,
    };
    let sidebar = h.split_pane(&plan.anchor, split_dir, 0.5, true)?;
    state_file.sidebar_pane = Some(sidebar.clone());
    state::save(&state_file)?;

    if matches!(position, SidebarPosition::Left | SidebarPosition::Top) {
        // Moving a pane relative to a sibling in the same tab does not reorder
        // them. Round-trip the original anchor through a temporary tab, then
        // place it right/below the sidebar so the sidebar becomes left/top.
        if parking.is_none() {
            let (parking_tab, placeholder) = h.create_tab(&ctx.workspace)?;
            parking = Some((parking_tab, placeholder));
        }
        let Some((parking_tab, _)) = parking.as_ref() else {
            unreachable!("inverse positions always create a parking tab")
        };
        h.move_pane(&plan.anchor, parking_tab, Dir::Right, None, None, false)?;
        let dir = match position {
            SidebarPosition::Left => Dir::Right,
            SidebarPosition::Top => Dir::Down,
            SidebarPosition::Right | SidebarPosition::Bottom => unreachable!(),
        };
        h.move_pane(
            &plan.anchor,
            &ctx.tab,
            dir,
            Some(&sidebar),
            Some(0.5),
            false,
        )?;
    }

    h.run_in_pane(&sidebar, sidebar_cmd)?;
    for step in &plan.steps {
        h.move_pane(
            &step.pane,
            &ctx.tab,
            step.dir,
            Some(&step.target),
            Some(step.ratio),
            false,
        )?;
    }
    if let Some((_, placeholder)) = parking {
        h.close_pane(&placeholder)?;
    }

    state_file.phase = Phase::Open;
    state_file.parking_tab = None;
    state_file.parked.clear();
    state_file.sidebar_pane = Some(sidebar);
    state::save(&state_file)
}

pub fn recover(h: &mut dyn Herdr, tab: &str) -> Result<()> {
    let Some(mut state_file) = state::load(tab)? else {
        return Ok(());
    };
    if !matches!(state_file.phase, Phase::Evacuating) {
        return Ok(());
    }

    if let Some(sidebar) = state_file.sidebar_pane.as_deref() {
        if h.pane_alive(sidebar)? {
            h.close_pane(sidebar)?;
        }
    }

    for step in &state_file.plan_steps {
        if !state_file.parked.iter().any(|pane| pane == &step.pane) {
            continue;
        }
        h.move_pane(
            &step.pane,
            &state_file.tab,
            step.dir,
            Some(&step.target),
            Some(step.ratio),
            false,
        )?;
        state_file.parked.retain(|pane| pane != &step.pane);
        state::save(&state_file)?;
    }

    if let Some(pane) = state_file.parked.first() {
        bail!("recovery state contains parked pane {pane} without a rebuild step");
    }
    state::remove(tab)
}

pub fn toggle_cmd() -> Result<()> {
    let mut herdr = CliHerdr;
    let ctx = read_ctx(&mut herdr)?;
    let sidebar_cmd = daemon::sidebar_shell_cmd(&ctx.tab, &ctx.cwd);
    toggle(&mut herdr, &ctx, &sidebar_cmd)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        env, fs,
        path::PathBuf,
        sync::{
            atomic::{AtomicUsize, Ordering},
            MutexGuard,
        },
    };

    use super::*;
    use crate::{
        herdr::{MockHerdr, PaneRect},
        layout::MoveStep,
        state::{self, Phase, StateFile},
    };

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn ctx() -> Ctx {
        Ctx {
            workspace: "wT".into(),
            tab: "wT:t1".into(),
            focused_pane: "wT:p1".into(),
            cwd: PathBuf::from("/repo"),
        }
    }

    fn rect(pane_id: &str, x: u32, y: u32, w: u32, h: u32) -> PaneRect {
        PaneRect {
            pane_id: pane_id.into(),
            x,
            y,
            w,
            h,
        }
    }

    fn three_pane_rects() -> Vec<PaneRect> {
        vec![
            rect("wT:p1", 0, 0, 40, 100),
            rect("wT:p2", 40, 0, 60, 30),
            rect("wT:p3", 40, 30, 60, 70),
        ]
    }

    // The opportunistic gc(h) at the start of toggle() calls h.list_tabs() only
    // if the (isolated, per-test) runtime dir happens to exist; StateDirGuard
    // points HERDR_NVIM_RUNTIME_DIR at a directory that is never created, so gc
    // is normally a no-op. Scripting a response here anyway is defensive: it
    // keeps these tests correct even if that gc short-circuit ever changes.
    fn mock_3pane() -> MockHerdr {
        MockHerdr {
            pane_rects_results: VecDeque::from([Ok(three_pane_rects())]),
            create_tab_results: VecDeque::from([Ok(("wT:t9".into(), "wT:p90".into()))]),
            split_pane_results: VecDeque::from([Ok("wT:p99".into())]),
            list_tabs_results: VecDeque::from([Ok(vec![])]),
            ..Default::default()
        }
    }

    fn mock_1pane() -> MockHerdr {
        MockHerdr {
            pane_rects_results: VecDeque::from([Ok(vec![rect("wT:p1", 0, 0, 100, 100)])]),
            split_pane_results: VecDeque::from([Ok("wT:p99".into())]),
            list_tabs_results: VecDeque::from([Ok(vec![])]),
            ..Default::default()
        }
    }

    fn mock_with_alive_sidebar() -> MockHerdr {
        MockHerdr {
            pane_alive_results: VecDeque::from([Ok(true)]),
            list_tabs_results: VecDeque::from([Ok(vec![])]),
            ..Default::default()
        }
    }

    fn open_state() -> StateFile {
        StateFile {
            phase: Phase::Open,
            workspace: "wT".into(),
            tab: "wT:t1".into(),
            anchor: "wT:p1".into(),
            parking_tab: None,
            parked: vec![],
            plan_steps: vec![],
            sidebar_pane: Some("wT:p99".into()),
        }
    }

    fn evacuating_state() -> StateFile {
        StateFile {
            phase: Phase::Evacuating,
            workspace: "wT".into(),
            tab: "wT:t1".into(),
            anchor: "wT:p1".into(),
            parking_tab: Some("wT:t9".into()),
            parked: vec!["wT:p2".into(), "wT:p3".into()],
            plan_steps: vec![
                MoveStep {
                    pane: "wT:p2".into(),
                    dir: Dir::Right,
                    target: "wT:p1".into(),
                    ratio: 0.4,
                },
                MoveStep {
                    pane: "wT:p3".into(),
                    dir: Dir::Down,
                    target: "wT:p2".into(),
                    ratio: 0.3,
                },
            ],
            sidebar_pane: None,
        }
    }

    struct StateDirGuard {
        _state_lock: MutexGuard<'static, ()>,
        _runtime_lock: MutexGuard<'static, ()>,
        old_state: Option<std::ffi::OsString>,
        old_runtime: Option<std::ffi::OsString>,
        dir: PathBuf,
    }

    impl StateDirGuard {
        fn new(dir: PathBuf) -> Self {
            let state_lock = state::STATE_DIR_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            // Also isolate the daemon socket dir: toggle()'s opportunistic gc
            // scans it, and without this override it would fall back to the
            // real XDG runtime dir / system temp dir and could reap live
            // daemons unrelated to this test.
            let runtime_lock = daemon::RUNTIME_DIR_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let old_state = env::var_os("HERDR_NVIM_STATE_DIR");
            let old_runtime = env::var_os("HERDR_NVIM_RUNTIME_DIR");
            let _ = fs::remove_dir_all(&dir);
            env::set_var("HERDR_NVIM_STATE_DIR", &dir);
            env::set_var("HERDR_NVIM_RUNTIME_DIR", dir.join("runtime"));
            Self {
                _state_lock: state_lock,
                _runtime_lock: runtime_lock,
                old_state,
                old_runtime,
                dir,
            }
        }
    }

    impl Drop for StateDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
            match &self.old_state {
                Some(value) => env::set_var("HERDR_NVIM_STATE_DIR", value),
                None => env::remove_var("HERDR_NVIM_STATE_DIR"),
            }
            match &self.old_runtime {
                Some(value) => env::set_var("HERDR_NVIM_RUNTIME_DIR", value),
                None => env::remove_var("HERDR_NVIM_RUNTIME_DIR"),
            }
        }
    }

    fn with_state_dir(test: impl FnOnce()) {
        let dir = env::temp_dir().join(format!(
            "herdr-nvim-maneuver-{}-{}",
            std::process::id(),
            TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _guard = StateDirGuard::new(dir);
        test();
    }

    #[test]
    fn open_on_multi_pane_tab_emits_validated_sequence() {
        with_state_dir(|| {
            let mut h = mock_3pane();
            open(&mut h, &ctx(), "exec sidebar", SidebarPosition::Right).unwrap();
            assert_eq!(
                h.ops,
                vec![
                    "rects wT:t1",
                    "create_tab wT",
                    "move wT:p2 -> tab:wT:t9 dir:Right target:- ratio:- focus:false",
                    "move wT:p3 -> tab:wT:t9 dir:Right target:- ratio:- focus:false",
                    "split wT:p1 dir:Right ratio:0.5 focus:true",
                    "run wT:p99 exec sidebar",
                    "move wT:p2 -> tab:wT:t1 dir:Right target:wT:p1 ratio:0.4 focus:false",
                    "move wT:p3 -> tab:wT:t1 dir:Down target:wT:p2 ratio:0.3 focus:false",
                    "close wT:p90",
                ]
            );
            let state = state::load("wT:t1").unwrap().unwrap();
            assert!(matches!(state.phase, Phase::Open));
            assert!(state.parked.is_empty());
            assert_eq!(state.sidebar_pane.as_deref(), Some("wT:p99"));
        });
    }

    #[test]
    fn open_on_single_pane_tab_skips_evacuation() {
        with_state_dir(|| {
            let mut h = mock_1pane();
            open(&mut h, &ctx(), "exec sidebar", SidebarPosition::Right).unwrap();
            assert!(h.ops.iter().all(|op| !op.starts_with("create_tab")));
            assert!(h
                .ops
                .iter()
                .any(|op| op.starts_with("split wT:p1") && op.ends_with("focus:true")));
        });
    }

    #[test]
    fn sidebar_position_controls_split_direction() {
        with_state_dir(|| {
            let cases = [
                (
                    SidebarPosition::Left,
                    "split wT:p1 dir:Right ratio:0.5 focus:true",
                    Some("move wT:p1 -> tab:wT:t1 dir:Right target:wT:p99 ratio:0.5 focus:false"),
                ),
                (
                    SidebarPosition::Right,
                    "split wT:p1 dir:Right ratio:0.5 focus:true",
                    None,
                ),
                (
                    SidebarPosition::Top,
                    "split wT:p1 dir:Down ratio:0.5 focus:true",
                    Some("move wT:p1 -> tab:wT:t1 dir:Down target:wT:p99 ratio:0.5 focus:false"),
                ),
                (
                    SidebarPosition::Bottom,
                    "split wT:p1 dir:Down ratio:0.5 focus:true",
                    None,
                ),
            ];

            for (position, split, reposition) in cases {
                let mut h = mock_1pane();
                if matches!(position, SidebarPosition::Left | SidebarPosition::Top) {
                    h.create_tab_results
                        .push_back(Ok(("wT:t9".into(), "wT:p90".into())));
                }
                open(&mut h, &ctx(), "exec sidebar", position).unwrap();
                assert!(
                    h.ops.iter().any(|op| op == split),
                    "{position:?}: {:?}",
                    h.ops
                );
                if let Some(reposition) = reposition {
                    let parked = h
                        .ops
                        .iter()
                        .position(|op| {
                            op == "move wT:p1 -> tab:wT:t9 dir:Right target:- ratio:- focus:false"
                        })
                        .expect("inverse position must park the anchor first");
                    let restored = h
                        .ops
                        .iter()
                        .position(|op| op == reposition)
                        .expect("inverse position must restore the anchor beside the sidebar");
                    assert!(parked < restored, "{position:?}: {:?}", h.ops);
                } else {
                    assert!(
                        h.ops.iter().all(|op| !op.starts_with("create_tab")),
                        "{position:?}: {:?}",
                        h.ops
                    );
                }
            }
        });
    }

    #[test]
    fn toggle_with_live_sidebar_closes_it() {
        with_state_dir(|| {
            state::save(&open_state()).unwrap();
            let mut h = mock_with_alive_sidebar();
            toggle(&mut h, &ctx(), "exec sidebar").unwrap();
            assert_eq!(h.ops, vec!["alive wT:p99", "close wT:p99"]);
            assert!(state::load("wT:t1").unwrap().is_none());
        });
    }

    #[test]
    fn toggle_falls_through_to_recover_for_evacuating_checkpoint_with_alive_sidebar() {
        // Regression test: a mid-open crash can leave phase=Evacuating with
        // sidebar_pane already recorded (checkpointed right after split_pane).
        // toggle() must NOT take the fast Open-only close-and-return path here
        // -- it must fall through to recover(), which closes the orphaned
        // sidebar, replays the still-parked moves, and only then lets the
        // normal open() path create one fresh sidebar. Losing this gate would
        // strand p2/p3 on the parking tab and destroy the recovery state.
        with_state_dir(|| {
            let mut checkpoint = evacuating_state();
            checkpoint.sidebar_pane = Some("wT:p99".into());
            state::save(&checkpoint).unwrap();

            let mut h = mock_3pane();
            h.pane_alive_results.push_back(Ok(true));

            toggle(&mut h, &ctx(), "exec sidebar").unwrap();

            assert_eq!(h.ops[0], "alive wT:p99");
            assert_eq!(h.ops[1], "close wT:p99");
            assert!(
                h.ops[2..].iter().any(|op| op
                    == "move wT:p2 -> tab:wT:t1 dir:Right target:wT:p1 ratio:0.4 focus:false"),
                "parked pane p2 must be replayed back by recover(), not stranded: {:?}",
                h.ops
            );
            assert!(
                h.ops[2..].iter().any(|op| op
                    == "move wT:p3 -> tab:wT:t1 dir:Down target:wT:p2 ratio:0.3 focus:false"),
                "parked pane p3 must be replayed back by recover(), not stranded: {:?}",
                h.ops
            );
            assert!(
                h.ops.iter().any(|op| op == "create_tab wT"),
                "toggle must fall through to a fresh open() after recovery: {:?}",
                h.ops
            );

            let state = state::load("wT:t1").unwrap().unwrap();
            assert!(matches!(state.phase, Phase::Open));
            assert!(state.parked.is_empty());
            assert_eq!(state.sidebar_pane.as_deref(), Some("wT:p99"));
        });
    }

    #[test]
    fn stale_state_dead_sidebar_reopens() {
        with_state_dir(|| {
            state::save(&open_state()).unwrap();
            let mut h = mock_3pane();
            h.pane_alive_results.push_front(Ok(false));
            toggle(&mut h, &ctx(), "exec sidebar").unwrap();
            assert_eq!(h.ops[0], "alive wT:p99");
            assert!(h.ops.iter().any(|op| op == "create_tab wT"));
            let state = state::load("wT:t1").unwrap().unwrap();
            assert!(matches!(state.phase, Phase::Open));
            assert_eq!(state.sidebar_pane.as_deref(), Some("wT:p99"));
        });
    }

    #[test]
    fn recover_moves_parked_panes_back() {
        with_state_dir(|| {
            state::save(&evacuating_state()).unwrap();
            let mut h = MockHerdr::default();
            recover(&mut h, "wT:t1").unwrap();
            assert_eq!(
                h.ops,
                vec![
                    "move wT:p2 -> tab:wT:t1 dir:Right target:wT:p1 ratio:0.4 focus:false",
                    "move wT:p3 -> tab:wT:t1 dir:Down target:wT:p2 ratio:0.3 focus:false",
                ]
            );
            assert!(state::load("wT:t1").unwrap().is_none());
        });
    }

    #[test]
    fn recover_closes_orphaned_sidebar_before_replaying_moves() {
        with_state_dir(|| {
            let mut saved_state = evacuating_state();
            saved_state.sidebar_pane = Some("wT:p99".into());
            state::save(&saved_state).unwrap();
            let mut h = MockHerdr {
                pane_alive_results: VecDeque::from([Ok(true)]),
                ..Default::default()
            };

            recover(&mut h, "wT:t1").unwrap();

            assert_eq!(
                h.ops,
                vec![
                    "alive wT:p99",
                    "close wT:p99",
                    "move wT:p2 -> tab:wT:t1 dir:Right target:wT:p1 ratio:0.4 focus:false",
                    "move wT:p3 -> tab:wT:t1 dir:Down target:wT:p2 ratio:0.3 focus:false",
                ]
            );
            assert!(state::load("wT:t1").unwrap().is_none());
        });
    }

    #[test]
    fn recover_skips_closing_dead_orphaned_sidebar() {
        with_state_dir(|| {
            let mut saved_state = evacuating_state();
            saved_state.sidebar_pane = Some("wT:p99".into());
            state::save(&saved_state).unwrap();
            let mut h = MockHerdr {
                pane_alive_results: VecDeque::from([Ok(false)]),
                ..Default::default()
            };

            recover(&mut h, "wT:t1").unwrap();

            assert_eq!(
                h.ops,
                vec![
                    "alive wT:p99",
                    "move wT:p2 -> tab:wT:t1 dir:Right target:wT:p1 ratio:0.4 focus:false",
                    "move wT:p3 -> tab:wT:t1 dir:Down target:wT:p2 ratio:0.3 focus:false",
                ]
            );
            assert!(state::load("wT:t1").unwrap().is_none());
        });
    }

    struct CtxEnvGuard {
        _lock: MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    static CTX_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    const CTX_ENV_VARS: [&str; 3] = [
        "HERDR_WORKSPACE_ID",
        "HERDR_PANE_ID",
        "HERDR_PLUGIN_CONTEXT_JSON",
    ];

    impl CtxEnvGuard {
        fn new() -> Self {
            let lock = CTX_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let saved = CTX_ENV_VARS
                .iter()
                .map(|&key| (key, env::var_os(key)))
                .collect();
            for key in CTX_ENV_VARS {
                env::remove_var(key);
            }
            Self { _lock: lock, saved }
        }
    }

    impl Drop for CtxEnvGuard {
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
    fn read_ctx_prefers_workspace_cwd_from_context_json() {
        let _guard = CtxEnvGuard::new();
        env::set_var("HERDR_WORKSPACE_ID", "wA");
        env::set_var("HERDR_PANE_ID", "wA:p1");
        env::set_var(
            "HERDR_PLUGIN_CONTEXT_JSON",
            r#"{"tab_id":"wA:t1","workspace_cwd":"/repo/ws","focused_pane_cwd":"/repo/pane"}"#,
        );

        let mut h = MockHerdr::default();
        let ctx = read_ctx(&mut h).unwrap();
        assert_eq!(ctx.cwd, PathBuf::from("/repo/ws"));
        assert!(h.ops.is_empty(), "no pane_cwd fallback call expected");
    }

    #[test]
    fn read_ctx_falls_back_to_focused_pane_cwd_when_workspace_cwd_absent() {
        let _guard = CtxEnvGuard::new();
        env::set_var("HERDR_WORKSPACE_ID", "wA");
        env::set_var("HERDR_PANE_ID", "wA:p1");
        env::set_var(
            "HERDR_PLUGIN_CONTEXT_JSON",
            r#"{"tab_id":"wA:t1","focused_pane_cwd":"/repo/pane"}"#,
        );

        let mut h = MockHerdr::default();
        let ctx = read_ctx(&mut h).unwrap();
        assert_eq!(ctx.cwd, PathBuf::from("/repo/pane"));
        assert!(h.ops.is_empty(), "no pane_cwd fallback call expected");
    }

    #[test]
    fn read_ctx_falls_back_to_pane_cwd_call_when_neither_json_field_present() {
        let _guard = CtxEnvGuard::new();
        env::set_var("HERDR_WORKSPACE_ID", "wA");
        env::set_var("HERDR_PANE_ID", "wA:p1");
        env::set_var("HERDR_PLUGIN_CONTEXT_JSON", r#"{"tab_id":"wA:t1"}"#);

        let mut h = MockHerdr {
            pane_cwd_results: VecDeque::from([Ok(PathBuf::from("/repo/from-cli"))]),
            ..Default::default()
        };
        let ctx = read_ctx(&mut h).unwrap();
        assert_eq!(ctx.cwd, PathBuf::from("/repo/from-cli"));
        assert_eq!(h.ops, vec!["pane_cwd wA:p1"]);
    }
}
