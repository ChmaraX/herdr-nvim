use std::env;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::{
    daemon,
    herdr::{CliHerdr, Dir, Herdr},
    layout::{plan_rebuild, RebuildPlan},
    state::{self, Phase, PlanStep, StateFile},
};

pub struct Ctx {
    pub workspace: String,
    pub tab: String,
    pub focused_pane: String,
}

pub fn read_ctx() -> Result<Ctx> {
    let workspace = env::var("HERDR_WORKSPACE_ID").context("HERDR_WORKSPACE_ID is not set")?;
    let focused_pane = env::var("HERDR_PANE_ID").context("HERDR_PANE_ID is not set")?;
    let plugin_context =
        env::var("HERDR_PLUGIN_CONTEXT_JSON").context("HERDR_PLUGIN_CONTEXT_JSON is not set")?;
    let plugin_context: Value =
        serde_json::from_str(&plugin_context).context("invalid HERDR_PLUGIN_CONTEXT_JSON")?;
    let tab = tab_id(&plugin_context).context("plugin context is missing pane tab_id")?;

    Ok(Ctx {
        workspace,
        tab: tab.to_owned(),
        focused_pane,
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

pub fn toggle(h: &mut dyn Herdr, ctx: &Ctx, sidebar_cmd: &str) -> Result<()> {
    if let Some(existing) = state::load(&ctx.workspace)? {
        if let Some(sidebar) = existing.sidebar_pane.as_deref() {
            if h.pane_alive(sidebar)? {
                h.close_pane(sidebar)?;
                state::remove(&ctx.workspace)?;
                if existing.tab == ctx.tab {
                    return Ok(());
                }
            }
        }
    }

    recover(h, &ctx.workspace)?;
    open(h, ctx, sidebar_cmd)
}

fn open(h: &mut dyn Herdr, ctx: &Ctx, sidebar_cmd: &str) -> Result<()> {
    let rects = h.pane_rects(&ctx.tab)?;
    let plan = plan_rebuild(&rects)?;
    let mut state_file = StateFile {
        phase: Phase::Open,
        workspace: ctx.workspace.clone(),
        tab: ctx.tab.clone(),
        anchor: plan.anchor.clone(),
        parking_tab: None,
        parked: vec![],
        plan_steps: plan_steps(&plan),
        sidebar_pane: None,
    };

    let parking_placeholder = if rects.len() > 1 {
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
        Some(placeholder)
    } else {
        None
    };

    let sidebar = h.split_pane(&plan.anchor, Dir::Right, 0.5, true)?;
    state_file.sidebar_pane = Some(sidebar.clone());
    state::save(&state_file)?;

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
    if let Some(placeholder) = parking_placeholder {
        h.close_pane(&placeholder)?;
    }

    state_file.phase = Phase::Open;
    state_file.parking_tab = None;
    state_file.parked.clear();
    state_file.sidebar_pane = Some(sidebar);
    state::save(&state_file)
}

pub fn recover(h: &mut dyn Herdr, workspace: &str) -> Result<()> {
    let Some(mut state_file) = state::load(workspace)? else {
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
            parse_dir(&step.dir)?,
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
    state::remove(workspace)
}

fn plan_steps(plan: &RebuildPlan) -> Vec<PlanStep> {
    plan.steps
        .iter()
        .map(|step| PlanStep {
            pane: step.pane.clone(),
            dir: dir_name(step.dir).to_owned(),
            target: step.target.clone(),
            ratio: step.ratio,
        })
        .collect()
}

fn dir_name(dir: Dir) -> &'static str {
    match dir {
        Dir::Right => "right",
        Dir::Down => "down",
    }
}

fn parse_dir(dir: &str) -> Result<Dir> {
    match dir {
        "right" => Ok(Dir::Right),
        "down" => Ok(Dir::Down),
        _ => bail!("invalid rebuild direction {dir}"),
    }
}

pub fn toggle_cmd() -> Result<()> {
    let ctx = read_ctx()?;
    let sidebar_cmd = daemon::sidebar_shell_cmd(&ctx.workspace);
    let mut herdr = CliHerdr;
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
        state::{self, Phase, PlanStep, StateFile},
    };

    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn ctx() -> Ctx {
        Ctx {
            workspace: "wT".into(),
            tab: "wT:t1".into(),
            focused_pane: "wT:p1".into(),
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

    fn mock_3pane() -> MockHerdr {
        MockHerdr {
            pane_rects_results: VecDeque::from([Ok(three_pane_rects())]),
            create_tab_results: VecDeque::from([Ok(("wT:t9".into(), "wT:p90".into()))]),
            split_pane_results: VecDeque::from([Ok("wT:p99".into())]),
            ..Default::default()
        }
    }

    fn mock_1pane() -> MockHerdr {
        MockHerdr {
            pane_rects_results: VecDeque::from([Ok(vec![rect("wT:p1", 0, 0, 100, 100)])]),
            split_pane_results: VecDeque::from([Ok("wT:p99".into())]),
            ..Default::default()
        }
    }

    fn mock_with_alive_sidebar() -> MockHerdr {
        MockHerdr {
            pane_alive_results: VecDeque::from([Ok(true)]),
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
                PlanStep {
                    pane: "wT:p2".into(),
                    dir: "right".into(),
                    target: "wT:p1".into(),
                    ratio: 0.4,
                },
                PlanStep {
                    pane: "wT:p3".into(),
                    dir: "down".into(),
                    target: "wT:p2".into(),
                    ratio: 0.3,
                },
            ],
            sidebar_pane: None,
        }
    }

    struct StateDirGuard {
        _lock: MutexGuard<'static, ()>,
        old: Option<std::ffi::OsString>,
        dir: PathBuf,
    }

    impl StateDirGuard {
        fn new(dir: PathBuf) -> Self {
            let lock = state::STATE_DIR_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let old = env::var_os("HERDR_NVIM_STATE_DIR");
            let _ = fs::remove_dir_all(&dir);
            env::set_var("HERDR_NVIM_STATE_DIR", &dir);
            Self {
                _lock: lock,
                old,
                dir,
            }
        }
    }

    impl Drop for StateDirGuard {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
            match &self.old {
                Some(value) => env::set_var("HERDR_NVIM_STATE_DIR", value),
                None => env::remove_var("HERDR_NVIM_STATE_DIR"),
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
            toggle(&mut h, &ctx(), "exec sidebar").unwrap();
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
            let state = state::load("wT").unwrap().unwrap();
            assert!(matches!(state.phase, Phase::Open));
            assert!(state.parked.is_empty());
            assert_eq!(state.sidebar_pane.as_deref(), Some("wT:p99"));
        });
    }

    #[test]
    fn open_on_single_pane_tab_skips_evacuation() {
        with_state_dir(|| {
            let mut h = mock_1pane();
            toggle(&mut h, &ctx(), "exec sidebar").unwrap();
            assert!(h.ops.iter().all(|op| !op.starts_with("create_tab")));
            assert!(h
                .ops
                .iter()
                .any(|op| op.starts_with("split wT:p1") && op.ends_with("focus:true")));
        });
    }

    #[test]
    fn toggle_with_live_sidebar_closes_it() {
        with_state_dir(|| {
            state::save(&open_state()).unwrap();
            let mut h = mock_with_alive_sidebar();
            toggle(&mut h, &ctx(), "exec sidebar").unwrap();
            assert_eq!(h.ops, vec!["alive wT:p99", "close wT:p99"]);
            assert!(state::load("wT").unwrap().is_none());
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
            let state = state::load("wT").unwrap().unwrap();
            assert!(matches!(state.phase, Phase::Open));
            assert_eq!(state.sidebar_pane.as_deref(), Some("wT:p99"));
        });
    }

    #[test]
    fn recover_moves_parked_panes_back() {
        with_state_dir(|| {
            state::save(&evacuating_state()).unwrap();
            let mut h = MockHerdr::default();
            recover(&mut h, "wT").unwrap();
            assert_eq!(
                h.ops,
                vec![
                    "move wT:p2 -> tab:wT:t1 dir:Right target:wT:p1 ratio:0.4 focus:false",
                    "move wT:p3 -> tab:wT:t1 dir:Down target:wT:p2 ratio:0.3 focus:false",
                ]
            );
            assert!(state::load("wT").unwrap().is_none());
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

            recover(&mut h, "wT").unwrap();

            assert_eq!(
                h.ops,
                vec![
                    "alive wT:p99",
                    "close wT:p99",
                    "move wT:p2 -> tab:wT:t1 dir:Right target:wT:p1 ratio:0.4 focus:false",
                    "move wT:p3 -> tab:wT:t1 dir:Down target:wT:p2 ratio:0.3 focus:false",
                ]
            );
            assert!(state::load("wT").unwrap().is_none());
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

            recover(&mut h, "wT").unwrap();

            assert_eq!(
                h.ops,
                vec![
                    "alive wT:p99",
                    "move wT:p2 -> tab:wT:t1 dir:Right target:wT:p1 ratio:0.4 focus:false",
                    "move wT:p3 -> tab:wT:t1 dir:Down target:wT:p2 ratio:0.3 focus:false",
                ]
            );
            assert!(state::load("wT").unwrap().is_none());
        });
    }
}
