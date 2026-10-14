use std::{
    env, fs,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::{
    config::SidebarPosition,
    daemon,
    herdr::{CliHerdr, Dir, Herdr, PaneRect},
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

pub fn toggle(h: &mut dyn Herdr, ctx: &Ctx) -> Result<()> {
    // Opportunistic, best-effort gc: reap stale per-tab daemons left behind by
    // closed tabs. Non-fatal -- a gc failure must never block a toggle.
    let config = crate::config::load();
    let _ = daemon::registry::gc(h, &config.sidebar);

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
    open(
        h,
        ctx,
        config.sidebar.position,
        &SidebarSize::from_config(&config.sidebar),
        &config.sidebar.grow_window,
    )
}

/// The three sizing knobs of `[sidebar]`, resolved against the tab's actual
/// size at open time by [`SidebarSize::share`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SidebarSize {
    pub width: f64,
    pub min_cols: u32,
    pub anchor_min_cols: u32,
    /// Cells the sidebar shows beside its editor (a file tree); used by
    /// `open` to decide what `fit --extra` will later report. `0` = none.
    pub tree_cols: u32,
    /// `Some` in clamp mode (see `FitMode`): the split is the user's, `fit`
    /// only keeps it inside these minima.
    pub clamp: Option<ClampRules>,
}

/// Clamp-mode minima (cells along the split axis). `tree_std` is the normal
/// tree size, `tree_collapsed` what it shrinks to below `collapse_below`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClampRules {
    pub anchor_min: u32,
    pub editor_min: u32,
    pub tree_std: u32,
    pub tree_collapsed: u32,
    pub tab_min: u32,
    pub collapse_below: u32,
    pub floor: u32,
}

/// What clamp-mode `fit` wants: the tree size the sidebar's nvim should
/// show, and the sidebar size to resize to when the current one breaks a
/// minimum (`None` = leave the user's split alone).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ClampPlan {
    pub tree: u32,
    pub sidebar: Option<u32>,
}

impl ClampRules {
    fn collapsed(&self, total: u32) -> bool {
        self.collapse_below > 0 && total < self.collapse_below
    }

    /// Tree cells to open with at `total`.
    pub(crate) fn open_tree(&self, total: u32) -> u32 {
        if self.collapsed(total) {
            self.tree_collapsed
        } else {
            self.tree_std
        }
    }

    /// The default split at open time: anchor and editor equal, the tree on
    /// top of the editor's side; kept inside the minima.
    pub(crate) fn open_cells(&self, total: u32) -> u32 {
        let tree = self.open_tree(total);
        let even = tree + total.saturating_sub(tree) / 2;
        match self.plan(total, even, tree, 0).sidebar {
            Some(cells) => cells,
            None => even,
        }
    }

    /// Pure: `total` tab cells, `current` sidebar cells, `extra` tree cells
    /// the sidebar reports (`0` = the user closed the tree: left closed),
    /// `want` the tree size the user asked for (`0` = standard): respected
    /// even when narrower than standard, only capped by the minima.
    pub(crate) fn plan(&self, total: u32, current: u32, extra: u32, want: u32) -> ClampPlan {
        let collapsed = self.collapsed(total);
        let (anchor_min, editor_min) = if collapsed {
            (self.floor, self.floor)
        } else {
            (self.anchor_min, self.editor_min)
        };
        let tree = if extra == 0 {
            0
        } else if collapsed {
            self.tree_collapsed
        } else {
            // The user's size (narrow or wide), capped so both panes keep
            // their minima.
            let room = total.saturating_sub(anchor_min + editor_min);
            let want = if want == 0 { self.tree_std } else { want };
            want.min(room.max(1)).max(1)
        };
        let lo = editor_min + tree;
        let hi = total.saturating_sub(anchor_min);
        let target = if lo > hi {
            // Not even the minima fit: anchor and editor share what is left.
            tree + total.saturating_sub(tree) / 2
        } else if current < lo {
            lo
        } else if current > hi {
            hi
        } else {
            current
        };
        ClampPlan { tree, sidebar: (target != current).then_some(target) }
    }
}

impl SidebarSize {
    pub(crate) fn from_config(sidebar: &crate::config::Sidebar) -> Self {
        Self {
            width: sidebar.width(),
            min_cols: sidebar.min_cols,
            anchor_min_cols: sidebar.anchor_min_cols,
            tree_cols: sidebar.tree_cols,
            clamp: (sidebar.fit == crate::config::FitMode::Clamp).then_some(ClampRules {
                anchor_min: sidebar.anchor_min_cols,
                editor_min: sidebar.editor_min_cols,
                tree_std: sidebar.tree_cols,
                tree_collapsed: sidebar.tree_collapsed_cols,
                tab_min: sidebar.tab_min_cols,
                collapse_below: sidebar.collapse_below_cols,
                floor: sidebar.floor_cols,
            }),
        }
    }

    /// Cells needed along the split axis for both minima to fit; `0` when no
    /// minimum is configured. Clamp mode: the tab size `reclaim_window` and
    /// `grow_window` are asked for (`tab_min_cols`).
    pub(crate) fn required(&self) -> u32 {
        if let Some(rules) = self.clamp {
            return rules.tab_min;
        }
        if self.min_cols == 0 || self.anchor_min_cols == 0 {
            0
        } else {
            self.min_cols + self.anchor_min_cols
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) const fn plain(width: f64) -> Self {
        Self {
            width,
            min_cols: 0,
            anchor_min_cols: 0,
            tree_cols: 0,
            clamp: None,
        }
    }

    /// Share of `total` cells the sidebar should take: `width` of the tab,
    /// raised to `min_cols` when that is larger, but never leaving the anchor
    /// fewer than `anchor_min_cols` cells; when both minima cannot fit, the
    /// anchor keeps its minimum and the sidebar gets what remains (at least
    /// 20 cells, or half the tab if even that is impossible). Pure.
    pub(crate) fn share(&self, total: u32, extra: u32) -> f64 {
        if total == 0 {
            return self.width;
        }
        let total_f = f64::from(total);
        if let Some(rules) = self.clamp {
            let _ = extra; // the tree the open uses is `rules.open_tree(total)`
            return (f64::from(rules.open_cells(total)) / total_f).clamp(0.05, 0.95);
        }
        let required = self.required();
        let mut cells = if required > 0 && total < required {
            // Below the minima the anchor and the sidebar's *editor* shrink
            // together, 50/50: the sidebar keeps `extra` cells (what it shows
            // beside the editor, e.g. a file tree) on top of the anchor's size.
            ((total_f + f64::from(extra)) / 2.0).round()
        } else {
            let mut cells = (total_f * self.width).round();
            if self.min_cols > 0 {
                cells = cells.max(f64::from(self.min_cols));
            }
            if self.anchor_min_cols > 0 {
                cells = cells.min(total_f - f64::from(self.anchor_min_cols));
            }
            cells
        };
        if cells < 20.0 {
            cells = if total_f >= 40.0 { 20.0 } else { (total_f / 2.0).floor() };
        }
        (cells / total_f).clamp(0.05, 0.95)
    }

    /// Editor cells the sidebar would keep beside `extra` at `total`, under
    /// the same rule as [`SidebarSize::share`].
    pub(crate) fn editor_cells(&self, total: u32, extra: u32) -> u32 {
        let cells = (self.share(total, extra) * f64::from(total)).round() as u32;
        cells.saturating_sub(extra)
    }
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

/// Tab extent along the split axis of `position`, from its pane rects.
fn tab_extent(rects: &[PaneRect], position: SidebarPosition) -> u32 {
    rects
        .iter()
        .map(|r| match position {
            SidebarPosition::Left | SidebarPosition::Right => r.x + r.w,
            SidebarPosition::Top | SidebarPosition::Bottom => r.y + r.h,
        })
        .max()
        .unwrap_or(0)
}

/// Run the configured `grow_window` command, then poll the tab size (10 ×
/// 200 ms) until it reaches `required` or stops changing. Returns the latest
/// rects. Never fails the maneuver: a command that cannot be spawned, or a
/// window that does not grow, just leaves the tab as it is (reported on
/// stderr).
fn grow_window(
    h: &mut dyn Herdr,
    tab: &str,
    position: SidebarPosition,
    command: &[String],
    required: u32,
    mut rects: Vec<PaneRect>,
) -> Vec<PaneRect> {
    let Some((program, args)) = command.split_first() else {
        return rects;
    };
    let before = tab_extent(&rects, position);
    match std::process::Command::new(program)
        .args(args)
        .arg(required.to_string())
        .arg(before.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(_) => {}
        Err(error) => {
            eprintln!("herdr-nvim: grow_window {program:?} could not run: {error}");
            return rects;
        }
    }
    let mut last = before;
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let Ok(fresh) = h.pane_rects(tab) else { break };
        let extent = tab_extent(&fresh, position);
        rects = fresh;
        if extent >= required || extent == last {
            if extent >= required {
                break;
            }
        }
        last = extent;
    }
    let after = tab_extent(&rects, position);
    eprintln!("herdr-nvim: tab was {before} cells, needed {required}: grow_window → {after}");
    rects
}

fn open(
    h: &mut dyn Herdr,
    ctx: &Ctx,
    position: SidebarPosition,
    size: &SidebarSize,
    grow: &[String],
) -> Result<()> {
    // Best-effort: clear out ready markers abandoned by earlier opens (see
    // `sweep_stale_markers`) before this open adds its own.
    state::sweep_stale_markers();

    let mut rects = h.pane_rects(&ctx.tab)?;
    // Tab extent along the split axis, for the min_cols/anchor_min_cols rule.
    // Too small for both minima: let the configured command enlarge the
    // terminal window first, then lay out against the fresh size.
    let required = size.required();
    let mut total = tab_extent(&rects, position);
    if required > 0 && total < required && !grow.is_empty() {
        rects = grow_window(h, &ctx.tab, position, grow, required, rects);
        total = tab_extent(&rects, position);
    }
    // What the sidebar will show beside its editor at open time: its tree
    // (`tree_cols`) if the editor keeps at least twice that, else nothing.
    let extra = if let Some(rules) = size.clamp {
        rules.open_tree(total)
    } else if size.tree_cols > 0 && size.editor_cells(total, size.tree_cols) >= 2 * size.tree_cols {
        size.tree_cols
    } else {
        0
    };
    let width = size.share(total, extra);
    eprintln!(
        "herdr-nvim: sidebar share {:.3} of {total} cells (width {}, min {}, anchor min {}, extra {extra})",
        width, size.width, size.min_cols, size.anchor_min_cols
    );
    state::write_status(
        &format!("open · {total} col · nvim {:.0} · extra {extra}", width * f64::from(total)),
        None,
    );
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
        fitted_total: None,
        fitted_extra: None,
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
    // Note: unlike the old `split_pane(.., 0.5, ..)`, `plugin pane open` takes
    // no ratio, so the sidebar opens at herdr's default split (~50%). If an
    // exact width is ever required, follow this with a `pane resize`.
    //
    // `sidebar_cmd` execs `nvim --remote-ui` as soon as its pane opens and
    // reads its UI size from the pty at attach time, but the layout moves
    // below still have to run after the pane exists. `ready_marker` is a
    // nonced marker file `sidebar_cmd` polls for (with a timeout fallback),
    // touched below once every move is done.
    // The pid is folded into the low bits so a clock error (`unwrap_or_default`
    // collapsing the nanos to 0) still can't collide between two opens
    // racing on the same tab.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let nonce = (nanos << 32) | u128::from(std::process::id());
    let ready_marker = state::ready_marker_path(&ctx.tab, nonce);
    let sidebar = h.open_sidebar_pane(&plan.anchor, split_dir, &ctx.cwd, true, &ready_marker)?;
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
    // `[sidebar] width`: the split opens at 50 %, so move the sidebar's inner
    // edge by the difference, toward the anchor to grow and away from it to
    // shrink. Best-effort like the resync below: a failed resize leaves a
    // working 50 % sidebar, which must still be recorded as Open.
    let delta = width - 0.5;
    if delta.abs() > 1e-6 {
        let grow = delta > 0.0;
        let direction = match (position, grow) {
            (SidebarPosition::Right, true) | (SidebarPosition::Left, false) => "left",
            (SidebarPosition::Right, false) | (SidebarPosition::Left, true) => "right",
            (SidebarPosition::Bottom, true) | (SidebarPosition::Top, false) => "up",
            (SidebarPosition::Bottom, false) | (SidebarPosition::Top, true) => "down",
        };
        if let Err(error) = h.resize_pane(&sidebar, direction, delta.abs()) {
            eprintln!("herdr-nvim: could not resize the sidebar to {width}: {error}");
        }
    }
    // Every branch must end with an explicit winsize resync (see
    // `Herdr::sync_pane_sizes`): on a single-pane tab with a Right/Bottom
    // sidebar, `open_sidebar_pane` is the only layout-touching call, so
    // nothing else would push a winsize. Best-effort: panes are already in
    // their final position, so failing here must not abort before
    // `Phase::Open` is recorded and strand a good sidebar in `recover()`.
    let _ = h.sync_pane_sizes(&sidebar);
    // Signal `sidebar_cmd`'s poll. Must come after every geometry-affecting
    // step, including the placeholder close above, or the sidebar could
    // attach before the layout is final. Best-effort: this is an advisory
    // signal with a timeout fallback on the consumer side, so it must not
    // abort the maneuver with the panes already moved and the placeholder
    // never closed.
    let _ = fs::write(&ready_marker, b"");

    state_file.phase = Phase::Open;
    state_file.parking_tab = None;
    state_file.parked.clear();
    state_file.sidebar_pane = Some(sidebar);
    state_file.fitted_total = Some(total);
    state_file.fitted_extra = Some(extra);
    state::save(&state_file)
}

/// `herdr-nvim fit`: re-apply the configured share to an open sidebar after
/// the tab changed size (the sidebar's nvim calls it on `VimResized`). Reads
/// the tab from `HERDR_TAB_ID`. Does nothing when there is no open sidebar,
/// when the tab extent is the one the share was last computed against (so a
/// split the user moved by hand is left alone), or when the difference is
/// under one cell. Never grows the window: that is `open`'s job.
pub fn fit_cmd() -> Result<()> {
    let tab = env::var("HERDR_TAB_ID").context("HERDR_TAB_ID is not set")?;
    let extra = extra_arg(env::args());
    let mut herdr = CliHerdr;
    let config = crate::config::load();
    fit(
        &mut herdr,
        &tab,
        config.sidebar.position,
        &SidebarSize::from_config(&config.sidebar),
        &FitPolicy {
            reclaim: config.sidebar.reclaim_window.clone(),
            close_below: config.sidebar.close_below_cols,
            extra,
            want: cells_arg(env::args(), "--want"),
        },
    )
}

/// `fit --extra <cells>`: what the sidebar currently shows beside its editor
/// (its file tree, typically), reported by the sidebar's nvim. Default 0.
fn extra_arg(args: impl Iterator<Item = String>) -> u32 {
    cells_arg(args, "--extra")
}

/// `fit --want <cells>` (clamp mode): the tree size the user asked for.
fn cells_arg(mut args: impl Iterator<Item = String>, flag: &str) -> u32 {
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        }
    }
    0
}

/// What `fit` may do beyond resizing when the tab is too small.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FitPolicy {
    pub reclaim: Vec<String>,
    pub close_below: u32,
    /// Cells the sidebar currently shows beside its editor (see `fit --extra`).
    pub extra: u32,
    /// Clamp mode: the tree size the user asked for (`fit --want`), which may
    /// be narrower than `tree_cols`; `0` = the standard size.
    pub want: u32,
}

/// Run `reclaim_window`; `Some(true)` when it reports having changed
/// something (exit 0), `Some(false)` when nothing was left to reclaim,
/// `None` when it could not run (reported on stderr).
fn run_reclaim(command: &[String], required: u32, total: u32) -> Option<bool> {
    let (program, args) = command.split_first()?;
    match std::process::Command::new(program)
        .args(args)
        .arg(required.to_string())
        .arg(total.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) => Some(status.success()),
        Err(error) => {
            eprintln!("herdr-nvim: reclaim_window {program:?} could not run: {error}");
            None
        }
    }
}

pub(crate) fn fit(
    h: &mut dyn Herdr,
    tab: &str,
    position: SidebarPosition,
    size: &SidebarSize,
    policy: &FitPolicy,
) -> Result<()> {
    let started = std::time::Instant::now();
    let outcome = fit_inner(h, tab, position, size, policy, started);
    if let Err(error) = &outcome {
        state::write_status(&format!("fit ✗ {error} ({} ms)", started.elapsed().as_millis()), Some(started.elapsed()));
    }
    outcome
}

fn fit_inner(
    h: &mut dyn Herdr,
    tab: &str,
    position: SidebarPosition,
    size: &SidebarSize,
    policy: &FitPolicy,
    started: std::time::Instant,
) -> Result<()> {
    let Some(mut state_file) = state::load(tab)? else {
        return Ok(());
    };
    if !matches!(state_file.phase, Phase::Open) {
        return Ok(());
    }
    let Some(sidebar) = state_file.sidebar_pane.clone() else {
        return Ok(());
    };
    let rects = h.pane_rects(tab)?;
    let total = tab_extent(&rects, position);
    if state::measure_only() {
        // fit.off: report, never act (the user is sizing panes by hand).
        let current = rects
            .iter()
            .find(|r| r.pane_id == sidebar)
            .map(|r| match position {
                SidebarPosition::Left | SidebarPosition::Right => r.w,
                SidebarPosition::Top | SidebarPosition::Bottom => r.h,
            })
            .unwrap_or(0);
        let line = format!(
            "📏 Claude {} · éditeur {} · explo {} · onglet {total}",
            total.saturating_sub(current),
            current.saturating_sub(policy.extra),
            policy.extra
        );
        eprintln!("herdr-nvim: fit: measure only (fit.off): {line}");
        state::write_status(&format!("mesure (fit.off) · {line} · aucun recalage"), None);
        state::write_ms(&line);
        return Ok(());
    }
    if let Some(rules) = size.clamp {
        return fit_clamp(h, tab, position, &rules, policy, started, state_file, &sidebar, &rects, total);
    }
    if total == 0 || (state_file.fitted_total == Some(total) && state_file.fitted_extra == Some(policy.extra)) {
        state::write_status(
            &format!("fit · {total} col · extra {} · inchangé · {} ms", policy.extra, started.elapsed().as_millis()),
            Some(started.elapsed()),
        );
        return Ok(());
    }
    let required = size.required();
    // Let reclaim_window see every size change (cells needed, cells now): it
    // reclaims space inside the window when the tab is too small, and may give
    // it back when there is plenty. If it changed something, the next resize
    // event brings us back here with the new tab size; nothing more to do now.
    if run_reclaim(&policy.reclaim, required, total) == Some(true) {
        eprintln!("herdr-nvim: fit: tab {total} cells (need {required}): reclaim_window changed something; waiting for the next resize");
        state::write_status(
            &format!("fit · {total} col (besoin {required}) · barre repliée/dépliée · {} ms", started.elapsed().as_millis()),
            Some(started.elapsed()),
        );
        state_file.fitted_total = Some(total);
        state_file.fitted_extra = Some(policy.extra);
        return state::save(&state_file);
    }
    if required > 0 && total < required {
        if policy.close_below > 0 && total < policy.close_below {
            eprintln!(
                "herdr-nvim: fit: tab {total} cells < close_below {}: closing the sidebar",
                policy.close_below
            );
            h.close_pane(&sidebar)?;
            return state::remove(tab);
        }
    }
    let Some(current) = rects.iter().find(|r| r.pane_id == sidebar).map(|r| match position {
        SidebarPosition::Left | SidebarPosition::Right => r.w,
        SidebarPosition::Top | SidebarPosition::Bottom => r.h,
    }) else {
        return Ok(());
    };
    let target = (size.share(total, policy.extra) * f64::from(total)).round() as u32;
    let mut what = "déjà en place".to_string();
    if let Some((direction, amount)) = fit_step(total, current, position, size, policy.extra) {
        eprintln!(
            "herdr-nvim: fit: tab now {total} cells (extra {}), sidebar {current} → {target}: resize {direction} {amount:.3}",
            policy.extra
        );
        h.resize_pane(&sidebar, direction, amount)?;
        what = format!("resize {direction} {current}→{target}");
    }
    state::write_status(
        &format!(
            "fit · {total} col · nvim {target} (éditeur {}) / autre {} · extra {} · {what} · {} ms",
            target.saturating_sub(policy.extra),
            total.saturating_sub(target),
            policy.extra,
            started.elapsed().as_millis()
        ),
        Some(started.elapsed()),
    );
    state_file.fitted_total = Some(total);
    state_file.fitted_extra = Some(policy.extra);
    state::save(&state_file)
}

/// Pure: the `pane resize` (direction, amount as a fraction of the tab) that
/// brings a sidebar of `current` cells to the configured share of `total`,
/// or `None` when the difference is under one cell.
/// Clamp mode: the split is the user's. Reclaim the window space first when
/// the tab is below `tab_min`, then only move the split when a minimum is
/// broken, and tell the sidebar's nvim (stderr `tree <cells>`) what tree size
/// the tab allows. Runs on every call: cheap, and idempotent once satisfied.
#[allow(clippy::too_many_arguments)]
fn fit_clamp(
    h: &mut dyn Herdr,
    tab: &str,
    position: SidebarPosition,
    rules: &ClampRules,
    policy: &FitPolicy,
    started: std::time::Instant,
    mut state_file: crate::state::StateFile,
    sidebar: &str,
    rects: &[crate::herdr::PaneRect],
    total: u32,
) -> Result<()> {
    if total == 0 {
        return Ok(());
    }
    let required = rules.tab_min;
    // reclaim_window costs a process: only when the tab extent changed (a window
    // resize or the multiplexer's own sidebar toggling), not on every split drag.
    if state_file.fitted_total != Some(total) && run_reclaim(&policy.reclaim, required, total) == Some(true) {
        eprintln!("herdr-nvim: fit: tab {total} cells (need {required}): reclaim_window changed something; waiting for the next resize");
        state::write_status(
            &format!("fit · {total} col (besoin {required}) · barre repliée/dépliée · {} ms", started.elapsed().as_millis()),
            Some(started.elapsed()),
        );
        state_file.fitted_total = Some(total);
        state_file.fitted_extra = Some(policy.extra);
        return state::save(&state_file);
    }
    if policy.close_below > 0 && total < policy.close_below {
        eprintln!("herdr-nvim: fit: tab {total} cells < close_below {}: closing the sidebar", policy.close_below);
        h.close_pane(sidebar)?;
        return state::remove(tab);
    }
    let Some(current) = rects.iter().find(|r| r.pane_id == sidebar).map(|r| match position {
        SidebarPosition::Left | SidebarPosition::Right => r.w,
        SidebarPosition::Top | SidebarPosition::Bottom => r.h,
    }) else {
        return Ok(());
    };
    let plan = rules.plan(total, current, policy.extra, policy.want);
    let mut what = String::from("libre");
    if let Some(target) = plan.sidebar {
        let grow = target > current;
        let direction = match (position, grow) {
            (SidebarPosition::Right, true) | (SidebarPosition::Left, false) => "left",
            (SidebarPosition::Right, false) | (SidebarPosition::Left, true) => "right",
            (SidebarPosition::Bottom, true) | (SidebarPosition::Top, false) => "up",
            (SidebarPosition::Bottom, false) | (SidebarPosition::Top, true) => "down",
        };
        let amount = f64::from(target.abs_diff(current)) / f64::from(total);
        eprintln!("herdr-nvim: fit: clamp: tab {total} cells, sidebar {current} → {target}: resize {direction} {amount:.3}");
        h.resize_pane(sidebar, direction, amount)?;
        what = format!("resize {current}→{target}");
    }
    if plan.tree != policy.extra {
        eprintln!("herdr-nvim: fit: tree {}", plan.tree);
        what = format!("{what} · explo→{}", plan.tree);
    }
    let shown = plan.sidebar.unwrap_or(current);
    state::write_status(
        &format!(
            "fit · {total} col · Claude {} · éditeur {} · explo {} · {what} · {} ms",
            total.saturating_sub(shown),
            shown.saturating_sub(plan.tree),
            plan.tree,
            started.elapsed().as_millis()
        ),
        Some(started.elapsed()),
    );
    state_file.fitted_total = Some(total);
    state_file.fitted_extra = Some(policy.extra);
    state::save(&state_file)
}

pub(crate) fn fit_step(
    total: u32,
    current: u32,
    position: SidebarPosition,
    size: &SidebarSize,
    extra: u32,
) -> Option<(&'static str, f64)> {
    if total == 0 {
        return None;
    }
    let target = (size.share(total, extra) * f64::from(total)).round();
    let delta = target - f64::from(current);
    if delta.abs() < 1.0 {
        return None;
    }
    let grow = delta > 0.0;
    let direction = match (position, grow) {
        (SidebarPosition::Right, true) | (SidebarPosition::Left, false) => "left",
        (SidebarPosition::Right, false) | (SidebarPosition::Left, true) => "right",
        (SidebarPosition::Bottom, true) | (SidebarPosition::Top, false) => "up",
        (SidebarPosition::Bottom, false) | (SidebarPosition::Top, true) => "down",
    };
    Some((direction, delta.abs() / f64::from(total)))
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
    toggle(&mut herdr, &ctx)
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, env, path::PathBuf, sync::MutexGuard};

    use super::*;
    use crate::{
        herdr::{MockHerdr, PaneRect},
        layout::MoveStep,
        state::{self, Phase, StateFile},
        test_support::TestEnv,
    };

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

    // The opportunistic gc(h) at the start of toggle() calls h.tab_infos() only
    // if the (isolated, per-test) runtime dir has registered daemons; TestEnv
    // points HERDR_NVIM_RUNTIME_DIR at a directory that is never created, so gc
    // is normally a no-op. Scripting a response here anyway is defensive: it
    // keeps these tests correct even if that gc short-circuit ever changes.
    fn mock_3pane() -> MockHerdr {
        MockHerdr {
            pane_rects_results: VecDeque::from([Ok(three_pane_rects())]),
            create_tab_results: VecDeque::from([Ok(("wT:t9".into(), "wT:p90".into()))]),
            split_pane_results: VecDeque::from([Ok("wT:p99".into())]),
            tab_infos_results: VecDeque::from([Ok(vec![])]),
            ..Default::default()
        }
    }

    fn mock_1pane() -> MockHerdr {
        MockHerdr {
            pane_rects_results: VecDeque::from([Ok(vec![rect("wT:p1", 0, 0, 100, 100)])]),
            split_pane_results: VecDeque::from([Ok("wT:p99".into())]),
            tab_infos_results: VecDeque::from([Ok(vec![])]),
            ..Default::default()
        }
    }

    fn mock_with_alive_sidebar() -> MockHerdr {
        MockHerdr {
            pane_alive_results: VecDeque::from([Ok(true)]),
            tab_infos_results: VecDeque::from([Ok(vec![])]),
            ..Default::default()
        }
    }

    /// Replaces the marker path segment in mocked `open_sidebar` ops with a
    /// fixed placeholder, since the nonce is time-seeded and unpredictable.
    fn normalize_marker(ops: &[String]) -> Vec<String> {
        ops.iter()
            .map(|op| match (op.find("marker:"), op.find(" focus:")) {
                (Some(start), Some(end)) if start < end => {
                    format!("{}marker:<M>{}", &op[..start], &op[end..])
                }
                _ => op.clone(),
            })
            .collect()
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
            fitted_total: None,
            fitted_extra: None,
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
            fitted_total: None,
            fitted_extra: None,
        }
    }

    fn with_state_dir(test: impl FnOnce()) {
        let _env = TestEnv::new();
        test();
    }

    #[test]
    fn open_on_multi_pane_tab_emits_validated_sequence() {
        with_state_dir(|| {
            let mut h = mock_3pane();
            open(&mut h, &ctx(), SidebarPosition::Right, &SidebarSize::plain(0.5), &[]).unwrap();
            assert_eq!(
                normalize_marker(&h.ops),
                vec![
                    "rects wT:t1",
                    "create_tab wT",
                    "move wT:p2 -> tab:wT:t9 dir:Right target:- ratio:- focus:false",
                    "move wT:p3 -> tab:wT:t9 dir:Right target:- ratio:- focus:false",
                    "open_sidebar wT:p1 dir:Right cwd:/repo marker:<M> focus:true",
                    "move wT:p2 -> tab:wT:t1 dir:Right target:wT:p1 ratio:0.4 focus:false",
                    "move wT:p3 -> tab:wT:t1 dir:Down target:wT:p2 ratio:0.3 focus:false",
                    "close wT:p90",
                    "marker_exists_at_close:false",
                    "sync_sizes wT:p99",
                    "marker_exists_at_sync:false",
                ]
            );
            let state = state::load("wT:t1").unwrap().unwrap();
            assert!(matches!(state.phase, Phase::Open));
            assert!(state.parked.is_empty());
            assert_eq!(state.sidebar_pane.as_deref(), Some("wT:p99"));
        });
    }

    #[test]
    fn open_writes_ready_marker_only_after_placeholder_close() {
        with_state_dir(|| {
            let mut h = mock_3pane();
            open(&mut h, &ctx(), SidebarPosition::Right, &SidebarSize::plain(0.5), &[]).unwrap();

            // `close_pane` records this synthetic op itself (see MockHerdr) by
            // checking on-disk marker existence at the moment it's called.
            let close = h
                .ops
                .iter()
                .position(|op| op == "close wT:p90")
                .expect("placeholder must be closed");
            assert_eq!(
                h.ops.get(close + 1).map(String::as_str),
                Some("marker_exists_at_close:false"),
                "marker must not exist yet when the placeholder closes: {:?}",
                h.ops
            );

            // The marker must exist by the time open() returns (nothing in
            // this test consumes it).
            let marker = h
                .ops
                .iter()
                .find_map(|op| op.strip_prefix("open_sidebar wT:p1 dir:Right cwd:/repo marker:"))
                .and_then(|rest| rest.split(' ').next())
                .expect("open_sidebar op must record the marker path");
            assert!(
                PathBuf::from(marker).exists(),
                "ready marker must exist once open() has settled the layout"
            );
        });
    }

    #[test]
    fn open_on_single_pane_tab_skips_evacuation() {
        with_state_dir(|| {
            let mut h = mock_1pane();
            open(&mut h, &ctx(), SidebarPosition::Right, &SidebarSize::plain(0.5), &[]).unwrap();
            assert!(h.ops.iter().all(|op| !op.starts_with("create_tab")));
            assert!(h
                .ops
                .iter()
                .any(|op| op.starts_with("open_sidebar wT:p1") && op.ends_with("focus:true")));
        });
    }

    /// Every branch of `open` must end with an explicit winsize resync before
    /// the ready marker, since a single-pane tab with a Right/Bottom sidebar
    /// has no other layout-touching call to push one.
    #[test]
    fn open_syncs_pane_sizes_on_every_branch_before_the_ready_marker() {
        for position in [
            SidebarPosition::Left,
            SidebarPosition::Right,
            SidebarPosition::Top,
            SidebarPosition::Bottom,
        ] {
            for panes in ["1pane", "3pane"] {
                with_state_dir(|| {
                    let mut h = if panes == "1pane" {
                        mock_1pane()
                    } else {
                        mock_3pane()
                    };
                    // Left/Top round-trip the anchor through a parking tab, so
                    // the single-pane mock needs one scripted `create_tab`.
                    if panes == "1pane"
                        && matches!(position, SidebarPosition::Left | SidebarPosition::Top)
                    {
                        h.create_tab_results
                            .push_back(Ok(("wT:t9".into(), "wT:p90".into())));
                    }
                    open(&mut h, &ctx(), position, &SidebarSize::plain(0.5), &[]).unwrap();

                    // The resync happens, on the sidebar pane, exactly once.
                    let sync = h
                        .ops
                        .iter()
                        .position(|op| op == "sync_sizes wT:p99")
                        .unwrap_or_else(|| {
                            panic!("{position:?}/{panes}: no winsize resync: {:?}", h.ops)
                        });
                    assert_eq!(
                        h.ops
                            .iter()
                            .filter(|op| op.starts_with("sync_sizes"))
                            .count(),
                        1,
                        "{position:?}/{panes}: {:?}",
                        h.ops
                    );

                    // It is the LAST layout-affecting op...
                    assert_eq!(
                        sync + 2,
                        h.ops.len(),
                        "{position:?}/{panes}: resync must be the final op: {:?}",
                        h.ops
                    );
                    // ...and the marker is not on disk yet when it runs, so
                    // the sidebar cannot attach to the un-resized pty.
                    assert_eq!(
                        h.ops.get(sync + 1).map(String::as_str),
                        Some("marker_exists_at_sync:false"),
                        "{position:?}/{panes}: {:?}",
                        h.ops
                    );
                });
            }
        }
    }

    #[test]
    fn sidebar_position_controls_split_direction() {
        with_state_dir(|| {
            let cases = [
                (
                    SidebarPosition::Left,
                    "open_sidebar wT:p1 dir:Right cwd:/repo",
                    Some("move wT:p1 -> tab:wT:t1 dir:Right target:wT:p99 ratio:0.5 focus:false"),
                ),
                (
                    SidebarPosition::Right,
                    "open_sidebar wT:p1 dir:Right cwd:/repo",
                    None,
                ),
                (
                    SidebarPosition::Top,
                    "open_sidebar wT:p1 dir:Down cwd:/repo",
                    Some("move wT:p1 -> tab:wT:t1 dir:Down target:wT:p99 ratio:0.5 focus:false"),
                ),
                (
                    SidebarPosition::Bottom,
                    "open_sidebar wT:p1 dir:Down cwd:/repo",
                    None,
                ),
            ];

            for (position, split, reposition) in cases {
                let mut h = mock_1pane();
                if matches!(position, SidebarPosition::Left | SidebarPosition::Top) {
                    h.create_tab_results
                        .push_back(Ok(("wT:t9".into(), "wT:p90".into())));
                }
                open(&mut h, &ctx(), position, &SidebarSize::plain(0.5), &[]).unwrap();
                assert!(
                    h.ops
                        .iter()
                        .any(|op| op.starts_with(split) && op.ends_with("focus:true")),
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
            toggle(&mut h, &ctx()).unwrap();
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

            toggle(&mut h, &ctx()).unwrap();

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
            toggle(&mut h, &ctx()).unwrap();
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

    #[test]
    fn sidebar_width_resizes_once_after_layout() {
        let _env = TestEnv::new();
        for (position, width, expected) in [
            (SidebarPosition::Right, 0.4, Some(("right", 0.1))),
            (SidebarPosition::Right, 0.7, Some(("left", 0.2))),
            (SidebarPosition::Left, 0.4, Some(("left", 0.1))),
            (SidebarPosition::Bottom, 0.3, Some(("down", 0.2))),
            (SidebarPosition::Right, 0.5, None),
        ] {
            let mut h = if matches!(position, SidebarPosition::Left | SidebarPosition::Top) {
                let mut h = mock_1pane();
                h.create_tab_results = VecDeque::from([Ok(("wT:t9".into(), "wT:p90".into()))]);
                h
            } else {
                mock_1pane()
            };
            let _ = state::remove("wT:t1");
            open(&mut h, &ctx(), position, &SidebarSize::plain(width), &[]).unwrap();
            let resize = h.ops.iter().find(|op| op.starts_with("resize wT:p99 "));
            match expected {
                Some((want_dir, want_amount)) => {
                    let op = resize.unwrap_or_else(|| panic!("{position:?} {width}: no resize op in {:?}", h.ops));
                    let rest = op.trim_start_matches("resize wT:p99 ");
                    let (dir, amount) = rest.split_once(' ').unwrap();
                    assert_eq!(dir, want_dir, "{position:?} {width}");
                    let got: f64 = amount.parse().unwrap();
                    assert!((got - want_amount).abs() < 1e-9, "{position:?} {width}: {op}");
                }
                None => assert!(resize.is_none(), "0.5 must not resize: {:?}", h.ops),
            }
        }
    }

    #[test]
    fn sidebar_share_applies_minima_against_the_tab_size() {
        // 45 % of 165 cells = 74, raised to min_cols 95, anchor keeps 70.
        let size = SidebarSize { width: 0.45, min_cols: 95, anchor_min_cols: 70, tree_cols: 0, clamp: None };
        assert!((size.share(165, 0) - 95.0 / 165.0).abs() < 1e-9);
        // Narrow tab: both minima cannot fit, the anchor and the editor split 50/50
        // around the extra cells (none here): 60 each.
        assert!((size.share(120, 0) - 60.0 / 120.0).abs() < 1e-9);
        // With a 34-cell tree beside the editor: sidebar 77, anchor 43 = editor 43.
        assert!((size.share(120, 34) - 77.0 / 120.0).abs() < 1e-9);
        // Very narrow: still 50/50 (40 each).
        assert!((size.share(80, 0) - 40.0 / 80.0).abs() < 1e-9);
        // Wide tab: plain width applies (45 % of 400 = 180 > 95).
        assert!((size.share(400, 0) - 180.0 / 400.0).abs() < 1e-9);
        // No minima: plain width, whatever the size.
        assert_eq!(SidebarSize::plain(0.4).share(165, 0), (66.0_f64 / 165.0).clamp(0.05, 0.95));
        assert_eq!(SidebarSize::plain(0.4).share(0, 0), 0.4);
    }

    #[test]
    fn required_cells_only_when_both_minima_are_set() {
        assert_eq!(SidebarSize { width: 0.45, min_cols: 95, anchor_min_cols: 70, tree_cols: 0, clamp: None }.required(), 165);
        assert_eq!(SidebarSize { width: 0.45, min_cols: 95, anchor_min_cols: 0, tree_cols: 0, clamp: None }.required(), 0);
        assert_eq!(SidebarSize::plain(0.5).required(), 0);
    }

    #[test]
    fn tab_extent_follows_the_split_axis() {
        let rects = vec![rect("wT:p1", 0, 0, 40, 100), rect("wT:p2", 40, 0, 60, 30)];
        assert_eq!(tab_extent(&rects, SidebarPosition::Right), 100);
        assert_eq!(tab_extent(&rects, SidebarPosition::Bottom), 100);
        assert_eq!(tab_extent(&[], SidebarPosition::Right), 0);
    }

    #[test]
    fn grow_window_polls_rects_after_the_command() {
        // A command that exists everywhere and exits at once; the mock then
        // serves one fresh rects result (the "grown" tab) and the function
        // returns it once the required size is reached.
        let cmd: Vec<String> = if cfg!(windows) {
            vec!["cmd.exe".into(), "/c".into(), "rem".into()]
        } else {
            vec!["true".into()]
        };
        let mut h = MockHerdr {
            pane_rects_results: VecDeque::from([Ok(vec![rect("wT:p1", 0, 0, 200, 50)])]),
            ..Default::default()
        };
        let small = vec![rect("wT:p1", 0, 0, 49, 50)];
        let grown = grow_window(&mut h, "wT:t1", SidebarPosition::Right, &cmd, 165, small);
        assert_eq!(tab_extent(&grown, SidebarPosition::Right), 200);
        // Unknown command: the original rects come back untouched.
        let cmd = vec!["herdr-nvim-no-such-command-xyz".to_string()];
        let same = grow_window(&mut h, "wT:t1", SidebarPosition::Right, &cmd, 165, vec![rect("wT:p1", 0, 0, 49, 50)]);
        assert_eq!(tab_extent(&same, SidebarPosition::Right), 49);
    }

    #[test]
    fn fit_step_moves_the_sidebar_to_the_share_of_the_new_tab_size() {
        let size = SidebarSize { width: 0.45, min_cols: 95, anchor_min_cols: 70, tree_cols: 0, clamp: None };
        // Tab shrank to 157 (below the 165 the minima need) with the sidebar still at
        // 161 cells: 50/50 below the minima, target 79, shrink by 82.
        let (dir, amount) = fit_step(157, 161, SidebarPosition::Right, &size, 0).unwrap();
        assert_eq!(dir, "right");
        assert!((amount - 82.0 / 157.0).abs() < 1e-9);
        // Tab grew to 357 with the sidebar at 87: target 161, grow by 74.
        let (dir, amount) = fit_step(357, 87, SidebarPosition::Right, &size, 0).unwrap();
        assert_eq!(dir, "left");
        assert!((amount - 74.0 / 357.0).abs() < 1e-9);
        // Already there: nothing.
        assert!(fit_step(357, 161, SidebarPosition::Right, &size, 0).is_none());
        assert!(fit_step(0, 10, SidebarPosition::Right, &size, 0).is_none());
    }

    #[test]
    fn fit_skips_when_the_tab_extent_did_not_change() {
        let _env = TestEnv::new();
        let size = SidebarSize { width: 0.45, min_cols: 95, anchor_min_cols: 70, tree_cols: 0, clamp: None };
        state::save(&StateFile {
            phase: Phase::Open,
            workspace: "wT".into(),
            tab: "wT:t1".into(),
            anchor: "wT:p1".into(),
            parking_tab: None,
            parked: vec![],
            plan_steps: vec![],
            sidebar_pane: Some("wT:p99".into()),
            fitted_total: Some(157),
            fitted_extra: Some(0),
        })
        .unwrap();
        // Same extent (157), sidebar moved by hand to 120: left alone.
        let mut h = MockHerdr {
            pane_rects_results: VecDeque::from([Ok(vec![rect("wT:p1", 0, 0, 37, 43), rect("wT:p99", 37, 0, 120, 43)])]),
            ..Default::default()
        };
        fit(&mut h, "wT:t1", SidebarPosition::Right, &size, &FitPolicy::default()).unwrap();
        assert!(h.ops.iter().all(|op| !op.starts_with("resize")), "{:?}", h.ops);
        // New extent (357): re-applied, and the new extent is remembered.
        let mut h = MockHerdr {
            pane_rects_results: VecDeque::from([Ok(vec![rect("wT:p1", 0, 0, 237, 43), rect("wT:p99", 237, 0, 120, 43)])]),
            ..Default::default()
        };
        fit(&mut h, "wT:t1", SidebarPosition::Right, &size, &FitPolicy::default()).unwrap();
        assert!(h.ops.iter().any(|op| op.starts_with("resize wT:p99 left")), "{:?}", h.ops);
        assert_eq!(state::load("wT:t1").unwrap().unwrap().fitted_total, Some(357));
    }

    #[test]
    fn fit_closes_the_sidebar_below_close_below_when_nothing_can_be_reclaimed() {
        let _env = TestEnv::new();
        let size = SidebarSize { width: 0.45, min_cols: 107, anchor_min_cols: 72, tree_cols: 0, clamp: None };
        let save = || {
            state::save(&StateFile {
                phase: Phase::Open,
                workspace: "wT".into(),
                tab: "wT:t1".into(),
                anchor: "wT:p1".into(),
                parking_tab: None,
                parked: vec![],
                plan_steps: vec![],
                sidebar_pane: Some("wT:p99".into()),
                fitted_total: Some(200),
                fitted_extra: None,
            })
            .unwrap()
        };
        // 100 cells, no reclaim command, close_below 112: the sidebar is closed and state removed.
        save();
        let mut h = MockHerdr {
            pane_rects_results: VecDeque::from([Ok(vec![rect("wT:p1", 0, 0, 72, 43), rect("wT:p99", 72, 0, 28, 43)])]),
            ..Default::default()
        };
        fit(&mut h, "wT:t1", SidebarPosition::Right, &size, &FitPolicy { reclaim: vec![], close_below: 112, extra: 0, want: 0 }).unwrap();
        assert!(h.ops.iter().any(|op| op == "close wT:p99"), "{:?}", h.ops);
        assert!(state::load("wT:t1").unwrap().is_none());
        // 130 cells (above close_below): degraded rule, no close.
        save();
        let mut h = MockHerdr {
            pane_rects_results: VecDeque::from([Ok(vec![rect("wT:p1", 0, 0, 72, 43), rect("wT:p99", 72, 0, 58, 43)])]),
            ..Default::default()
        };
        fit(&mut h, "wT:t1", SidebarPosition::Right, &size, &FitPolicy { reclaim: vec![], close_below: 112, extra: 0, want: 0 }).unwrap();
        assert!(h.ops.iter().all(|op| op != "close wT:p99"), "{:?}", h.ops);
        // A reclaim command that reports success defers everything to the next resize.
        save();
        let ok_cmd: Vec<String> = if cfg!(windows) { vec!["cmd.exe".into(), "/c".into(), "rem".into()] } else { vec!["true".into()] };
        let mut h = MockHerdr {
            pane_rects_results: VecDeque::from([Ok(vec![rect("wT:p1", 0, 0, 72, 43), rect("wT:p99", 72, 0, 28, 43)])]),
            ..Default::default()
        };
        fit(&mut h, "wT:t1", SidebarPosition::Right, &size, &FitPolicy { reclaim: ok_cmd, close_below: 112, extra: 0, want: 0 }).unwrap();
        assert!(h.ops.iter().all(|op| !op.starts_with("close") && !op.starts_with("resize")), "{:?}", h.ops);
        assert_eq!(state::load("wT:t1").unwrap().unwrap().fitted_total, Some(100));
    }

    #[test]
    fn extra_arg_is_parsed_from_fit_args() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>().into_iter();
        assert_eq!(extra_arg(args(&["herdr-nvim", "fit", "--extra", "34"])), 34);
        assert_eq!(extra_arg(args(&["herdr-nvim", "fit"])), 0);
        assert_eq!(extra_arg(args(&["herdr-nvim", "fit", "--extra", "x"])), 0);
    }

    #[test]
    fn editor_cells_follow_the_share() {
        let size = SidebarSize { width: 0.45, min_cols: 107, anchor_min_cols: 72, tree_cols: 34, clamp: None };
        assert_eq!(size.editor_cells(208, 34), 107 - 34);
        assert_eq!(size.editor_cells(120, 34), 77 - 34);
    }

    #[test]
    fn fit_reapplies_when_only_extra_changed() {
        let _env = TestEnv::new();
        let size = SidebarSize { width: 0.45, min_cols: 107, anchor_min_cols: 72, tree_cols: 34, clamp: None };
        state::save(&StateFile {
            phase: Phase::Open,
            workspace: "wT".into(),
            tab: "wT:t1".into(),
            anchor: "wT:p1".into(),
            parking_tab: None,
            parked: vec![],
            plan_steps: vec![],
            sidebar_pane: Some("wT:p99".into()),
            fitted_total: Some(163),
            fitted_extra: Some(34),
        })
        .unwrap();
        // Same extent, the tree just got hidden (extra 0): 50/50 re-applied (99 -> 82).
        let mut h = MockHerdr {
            pane_rects_results: VecDeque::from([Ok(vec![rect("wT:p1", 0, 0, 64, 43), rect("wT:p99", 64, 0, 99, 43)])]),
            ..Default::default()
        };
        fit(&mut h, "wT:t1", SidebarPosition::Right, &size, &FitPolicy { reclaim: vec![], close_below: 0, extra: 0, want: 0 }).unwrap();
        assert!(h.ops.iter().any(|op| op.starts_with("resize wT:p99 right")), "{:?}", h.ops);
        assert_eq!(state::load("wT:t1").unwrap().unwrap().fitted_extra, Some(0));
    }

    fn atelier_rules() -> ClampRules {
        // PC gabarit, 2026-10-14 evening (user's own numbers).
        ClampRules { anchor_min: 55, editor_min: 55, tree_std: 30, tree_collapsed: 1, tab_min: 160, collapse_below: 140, floor: 40 }
    }

    #[test]
    fn clamp_leaves_a_split_inside_the_minima_alone() {
        let r = atelier_rules();
        // 160, sidebar 80 = editor 50 + tree 30: the editor minimum (55) pushes it to 85.
        assert_eq!(r.plan(160, 80, 30, 30), ClampPlan { tree: 30, sidebar: Some(85) });
        // Claude 65 / editor 65 / tree 30 (the standard) and Claude 55 / editor 71 / tree 34: free.
        assert_eq!(r.plan(160, 95, 30, 0), ClampPlan { tree: 30, sidebar: None });
        assert_eq!(r.plan(160, 105, 34, 34), ClampPlan { tree: 34, sidebar: None });
        // Claude squeezed under 55: the sidebar gives back.
        assert_eq!(r.plan(160, 110, 30, 30), ClampPlan { tree: 30, sidebar: Some(105) });
        // Tree widened too far for the minima at 160: narrowed to what fits (50), split follows.
        assert_eq!(r.plan(160, 110, 60, 60), ClampPlan { tree: 50, sidebar: Some(105) });
        // Tree closed by the user: stays closed, editor min still enforced.
        assert_eq!(r.plan(160, 40, 0, 30), ClampPlan { tree: 0, sidebar: Some(55) });
        // The user collapsed the tree to 1 by hand on a wide tab: respected.
        assert_eq!(r.plan(160, 95, 1, 1), ClampPlan { tree: 1, sidebar: None });
        // ...and a tree at 1 with a standard wish (a rule collapse that no longer applies): back to 30.
        assert_eq!(r.plan(160, 95, 1, 30), ClampPlan { tree: 30, sidebar: None });
    }

    #[test]
    fn clamp_collapses_the_tree_below_140_and_keeps_the_floor() {
        let r = atelier_rules();
        // 140: the minimal balance 55/55/30 is free.
        assert_eq!(r.plan(140, 85, 30, 30), ClampPlan { tree: 30, sidebar: None });
        // 139: the tree collapses to 1, floor 40 for both — a 70/69 split is free.
        assert_eq!(r.plan(139, 69, 30, 30), ClampPlan { tree: 1, sidebar: None });
        // 100: Claude 40 / editor 59 / tree 1 is the user's example, free.
        assert_eq!(r.plan(100, 60, 1, 30), ClampPlan { tree: 1, sidebar: None });
        // 100 with Claude at 30: pushed back to 40.
        assert_eq!(r.plan(100, 70, 1, 30), ClampPlan { tree: 1, sidebar: Some(60) });
        // 70: nothing fits, half/half around the collapsed tree.
        assert_eq!(r.plan(70, 50, 1, 30), ClampPlan { tree: 1, sidebar: Some(35) });
        // Back above 140 with a collapsed tree: it reopens to 30 and the editor keeps 55.
        assert_eq!(r.plan(160, 60, 1, 30), ClampPlan { tree: 30, sidebar: Some(85) });
    }

    #[test]
    fn clamp_open_splits_anchor_and_editor_evenly() {
        let r = atelier_rules();
        assert_eq!(r.open_cells(160), 95); // Claude 65 / editor 65 / tree 30
        assert_eq!(r.open_cells(238), 134); // Claude 104 / editor 104 / tree 30
        assert_eq!(r.open_cells(100), 50); // collapsed: Claude 50 / editor 49 / tree 1
        let size = SidebarSize { width: 0.45, min_cols: 0, anchor_min_cols: 55, tree_cols: 30, clamp: Some(r) };
        assert_eq!(size.required(), 160);
        assert!((size.share(160, 30) - 95.0 / 160.0).abs() < 1e-9);
    }
}
