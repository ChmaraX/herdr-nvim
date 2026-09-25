use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::TabId;

/// The plugin id, used for `herdr plugin pane open --entrypoint sidebar`.
const PLUGIN_ID: &str = "chmarax.herdr-nvim";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRect {
    pub pane_id: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// An agent pane discovered via `herdr agent list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentInfo {
    pub pane_id: String,
    /// The tab this agent lives in, so the picker can prefer the agent in the
    /// *same tab* as the focused pane (e.g. the sidebar's own tab) over an
    /// unrelated first-in-workspace agent that may sit in a different repo.
    pub tab_id: String,
    pub focused: bool,
}

/// A live tab with the human-facing names herdr shows for it (from
/// `herdr api snapshot`), used by `herdr-nvim daemons` to say which tab a
/// hidden nvim daemon belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabInfo {
    pub tab_id: TabId,
    pub workspace_label: Option<String>,
    pub tab_label: Option<String>,
    pub tab_number: Option<u64>,
}

/// Pure: every tab in a `herdr api snapshot` response, joined with its
/// workspace's label. Missing labels/numbers are `None`, not errors.
fn parse_tab_infos(value: &Value) -> Result<Vec<TabInfo>> {
    let snapshot = value
        .pointer("/result/snapshot")
        .context("herdr api snapshot response missing result.snapshot")?;
    let tabs = snapshot
        .pointer("/tabs")
        .and_then(Value::as_array)
        .context("herdr api snapshot response missing result.snapshot.tabs array")?;
    let label = |node: &Value| {
        node.get("label")
            .and_then(Value::as_str)
            .filter(|label| !label.is_empty())
            .map(str::to_owned)
    };
    let workspaces = snapshot
        .pointer("/workspaces")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    tabs.iter()
        .map(|tab| {
            let tab_id = TabId::new(string_at(tab, "/tab_id")?);
            let workspace_id = tab
                .get("workspace_id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| tab_id.workspace());
            let workspace_label = workspaces
                .iter()
                .find(|ws| ws.get("workspace_id").and_then(Value::as_str) == Some(workspace_id))
                .and_then(label);
            Ok(TabInfo {
                workspace_label,
                tab_label: label(tab),
                tab_number: tab.get("number").and_then(Value::as_u64),
                tab_id,
            })
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSession {
    pub agent: String,
    pub kind: String,
    pub value: String,
}

/// Scroll geometry from `pane get` (`result.pane.scroll`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaneScroll {
    pub viewport_rows: u32,
    pub max_offset_from_bottom: u32,
}

impl PaneScroll {
    /// Lines herdr can serve *cheaply*: the live screen plus host scrollback.
    ///
    /// Asking `pane read` for more makes herdr drive the *application's own
    /// scroll* to recover history -- visibly scrolling an alt-screen TUI
    /// (pi's chat, which keeps `max_offset_from_bottom` at 0) at ~40ms per
    /// extra line. Callers use this to clamp read sizes.
    pub fn cheap_read_limit(self) -> u32 {
        self.viewport_rows
            .saturating_add(self.max_offset_from_bottom)
    }
}

/// The cwd, agent session, and scroll geometry of a pane, from a single
/// `pane get` response (see `Herdr::pane_snapshot`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneSnapshot {
    pub cwd: PathBuf,
    pub agent_session: Option<AgentSession>,
    pub scroll: Option<PaneScroll>,
}

/// Pure: extract `result.pane.agent_session` from a `pane get` response, if
/// present. Absence (older herdr, or a pane herdr doesn't track a session
/// for) is `None`, not an error -- the caller degrades to git/scrape layers.
fn parse_agent_session(value: &Value) -> Option<AgentSession> {
    let node = value.pointer("/result/pane/agent_session")?;
    Some(AgentSession {
        agent: node.get("agent")?.as_str()?.to_owned(),
        kind: node.get("kind")?.as_str()?.to_owned(),
        value: node.get("value")?.as_str()?.to_owned(),
    })
}

/// Pure: extract `result.pane.scroll` from a `pane get` response, if present.
/// Absence (older herdr without the scroll block) is `None`, not an error --
/// the caller then skips the read clamp and behaves as before.
fn parse_pane_scroll(value: &Value) -> Option<PaneScroll> {
    let node = value.pointer("/result/pane/scroll")?;
    Some(PaneScroll {
        viewport_rows: node.get("viewport_rows")?.as_u64()?.try_into().ok()?,
        max_offset_from_bottom: node
            .get("max_offset_from_bottom")?
            .as_u64()?
            .try_into()
            .ok()?,
    })
}

/// Pure: extract `result.pane.foreground_cwd` (required) plus the optional
/// `agent_session` and `scroll` blocks from a single `pane get` response.
/// Backs `pane_snapshot`, which folds what used to be two separate `pane get`
/// subprocess spawns (`pane_cwd` + `agent_session`) -- profiled as a
/// meaningful chunk of the pick-file action phase's latency -- into one.
fn parse_pane_snapshot(value: &Value) -> Result<PaneSnapshot> {
    Ok(PaneSnapshot {
        cwd: PathBuf::from(string_at(value, "/result/pane/foreground_cwd")?),
        agent_session: parse_agent_session(value),
        scroll: parse_pane_scroll(value),
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Dir {
    Right,
    Down,
}

impl Dir {
    fn as_cli_arg(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

pub trait Herdr {
    fn pane_rects(&mut self, tab: &str) -> Result<Vec<PaneRect>>;
    fn tab_of_pane(&mut self, pane: &str) -> Result<String>;
    fn create_tab(&mut self, workspace: &str) -> Result<(String, String)>;
    fn move_pane(
        &mut self,
        pane: &str,
        tab: &str,
        dir: Dir,
        target: Option<&str>,
        ratio: Option<f64>,
        focus: bool,
    ) -> Result<()>;
    fn split_pane(&mut self, pane: &str, dir: Dir, ratio: f64, focus: bool) -> Result<String>;
    /// Open the sidebar as a non-interactive plugin pane split off `pane`
    /// (`herdr plugin pane open --entrypoint sidebar --placement split`).
    /// Returns the new pane id. Non-interactive spawn means no shell echo of
    /// the command line in the pane (the whole reason this exists). `focus`
    /// requests the new pane be focused. `ready_marker` is passed to the pane
    /// via `--env HERDR_NVIM_READY_MARKER=...` for `daemon::sidebar_cmd`'s
    /// settle poll.
    fn open_sidebar_pane(
        &mut self,
        anchor: &str,
        dir: Dir,
        cwd: &Path,
        focus: bool,
        ready_marker: &Path,
    ) -> Result<String>;
    /// Re-pushes every pane's rect into its pty as a `TIOCSWINSZ` (and the
    /// SIGWINCH that follows) via a no-op `pane resize --amount 0`, without
    /// moving the layout: `herdr plugin pane open --placement split` inserts
    /// the new pane but does not resync pty winsizes for it or its sibling.
    fn sync_pane_sizes(&mut self, pane: &str) -> Result<()>;
    fn run_in_pane(&mut self, pane: &str, cmd: &str) -> Result<()>;
    fn close_pane(&mut self, pane: &str) -> Result<()>;
    fn pane_alive(&mut self, pane: &str) -> Result<bool>;
    /// Every live tab across all workspaces, with its workspace/tab names
    /// (`herdr api snapshot`). Used by `daemon::registry::gc` to tell which
    /// per-tab daemons are still live, and to name them.
    fn tab_infos(&mut self) -> Result<Vec<TabInfo>>;
    /// Read the pane's recent (unwrapped) output as plain text, newest lines
    /// last. `lines` bounds how many trailing lines are returned.
    fn read_pane(&mut self, pane: &str, lines: u32) -> Result<String>;
    /// The pane's foreground working directory (used to resolve relative paths).
    fn pane_cwd(&mut self, pane: &str) -> Result<PathBuf>;
    /// Agent panes in `workspace` (entries from `herdr agent list` that carry an
    /// `agent` label and belong to `workspace`).
    fn agents(&mut self, workspace: &str) -> Result<Vec<AgentInfo>>;
    /// The pane's cwd, agent session, and scroll geometry together, from a
    /// single `pane get` call. Prefer this over calling `pane_cwd` and
    /// `agent_session` separately when both are needed (as the pick-file
    /// action phase does) -- each of those is otherwise its own `pane get`
    /// subprocess spawn against the exact same underlying data. `open-link`
    /// only ever needs the cwd, so it keeps using `pane_cwd` alone.
    fn pane_snapshot(&mut self, pane: &str) -> Result<PaneSnapshot>;
}

pub struct CliHerdr;

impl CliHerdr {
    /// Runs a `herdr` subcommand and returns its raw `Output`, having already
    /// checked the exit status. Shared core behind `run`/`run_raw`/`run_text`,
    /// which differ only in how they interpret stdout on success.
    fn output(args: &[String]) -> Result<std::process::Output> {
        let command = format!("herdr {}", args.join(" "));
        let output = Command::new("herdr")
            .args(args)
            .output()
            .with_context(|| format!("failed to run {command}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("{command} failed: {}", stderr.trim());
        }
        Ok(output)
    }

    fn run(args: &[String]) -> Result<Value> {
        let output = Self::output(args)?;
        serde_json::from_slice(&output.stdout)
            .with_context(|| format!("failed to parse JSON from herdr {}", args.join(" ")))
    }

    /// Runs a `herdr` subcommand that is not expected to emit JSON (e.g.
    /// `pane run`, which prints nothing on success). Only the exit status is
    /// inspected; stdout is ignored entirely so an empty response is not
    /// mistaken for a parse error.
    fn run_raw(args: &[String]) -> Result<()> {
        Self::output(args)?;
        Ok(())
    }

    /// Runs a `herdr` subcommand whose stdout is plain text rather than JSON
    /// (e.g. `pane read --format text`) and returns that stdout verbatim.
    fn run_text(args: &[String]) -> Result<String> {
        let output = Self::output(args)?;
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn panes() -> Result<Vec<(String, String)>> {
        let value = Self::run(&args(&["pane", "list"]))?;
        let panes = value
            .pointer("/result/panes")
            .and_then(Value::as_array)
            .context("herdr pane list response missing result.panes array")?;

        panes
            .iter()
            .map(|pane| {
                Ok((
                    string_at(pane, "/pane_id")?.to_owned(),
                    string_at(pane, "/tab_id")?.to_owned(),
                ))
            })
            .collect()
    }
}

impl Herdr for CliHerdr {
    fn pane_rects(&mut self, tab: &str) -> Result<Vec<PaneRect>> {
        let pane = Self::panes()?
            .into_iter()
            .find_map(|(pane, pane_tab)| (pane_tab == tab).then_some(pane))
            .with_context(|| format!("tab {tab} has no live panes"))?;
        let value = Self::run(&args(&["pane", "layout", "--pane", &pane]))?;
        parse_pane_rects_value(&value)
    }

    fn tab_of_pane(&mut self, pane: &str) -> Result<String> {
        let value = Self::run(&args(&["pane", "get", pane]))?;
        Ok(string_at(&value, "/result/pane/tab_id")?.to_owned())
    }

    fn create_tab(&mut self, workspace: &str) -> Result<(String, String)> {
        let value = Self::run(&args(&[
            "tab",
            "create",
            "--workspace",
            workspace,
            "--no-focus",
        ]))?;
        Ok((
            string_at(&value, "/result/tab/tab_id")?.to_owned(),
            string_at(&value, "/result/root_pane/pane_id")?.to_owned(),
        ))
    }

    fn move_pane(
        &mut self,
        pane: &str,
        tab: &str,
        dir: Dir,
        target: Option<&str>,
        ratio: Option<f64>,
        focus: bool,
    ) -> Result<()> {
        let mut command = args(&[
            "pane",
            "move",
            pane,
            "--tab",
            tab,
            "--split",
            dir.as_cli_arg(),
        ]);
        if let Some(target) = target {
            command.extend(args(&["--target-pane", target]));
        }
        if let Some(ratio) = ratio {
            command.extend(args(&["--ratio", &ratio.to_string()]));
        }
        command.push(if focus { "--focus" } else { "--no-focus" }.to_owned());
        Self::run(&command)?;
        Ok(())
    }

    fn split_pane(&mut self, pane: &str, dir: Dir, ratio: f64, focus: bool) -> Result<String> {
        let value = Self::run(&args(&[
            "pane",
            "split",
            pane,
            "--direction",
            dir.as_cli_arg(),
            "--ratio",
            &ratio.to_string(),
            if focus { "--focus" } else { "--no-focus" },
        ]))?;
        Ok(string_at(&value, "/result/pane/pane_id")?.to_owned())
    }

    fn open_sidebar_pane(
        &mut self,
        anchor: &str,
        dir: Dir,
        cwd: &Path,
        focus: bool,
        ready_marker: &Path,
    ) -> Result<String> {
        // `herdr plugin pane open --entrypoint sidebar --placement split` runs
        // the manifest pane's command via `sh -c` (non-interactive) — no shell
        // prompt, no echoed command line. The sidebar reads its tab id from
        // HERDR_TAB_ID (set by herdr for plugin panes) and its cwd from this
        // --cwd, so no positional argv is needed.
        let value = Self::run(&args(&[
            "plugin",
            "pane",
            "open",
            "--plugin",
            PLUGIN_ID,
            "--entrypoint",
            "sidebar",
            "--placement",
            "split",
            "--target-pane",
            anchor,
            "--direction",
            dir.as_cli_arg(),
            "--cwd",
            &cwd.display().to_string(),
            "--env",
            &format!("HERDR_NVIM_READY_MARKER={}", ready_marker.display()),
            if focus { "--focus" } else { "--no-focus" },
        ]))?;
        Ok(string_at(&value, "/result/plugin_pane/pane/pane_id")?.to_owned())
    }

    fn sync_pane_sizes(&mut self, pane: &str) -> Result<()> {
        // The load-bearing effect here is the `TIOCSWINSZ` herdr re-pushes for
        // the resized pane and its sibling, not the SIGWINCH that follows it:
        // with the ready-marker handshake, nvim attaches only after this call,
        // so the size is already correct at attach time and the signal is a
        // happy-path no-op. Do not "simplify" by dropping the resize.
        //
        // `--direction`/`--amount` are arbitrary under `--amount 0`: the resize
        // moves no border (herdr reports `changed:false`), so `left` is just a
        // required-arg placeholder and the pane argument is what matters.
        Self::run(&args(&[
            "pane",
            "resize",
            "--pane",
            pane,
            "--direction",
            "left",
            "--amount",
            "0",
        ]))?;
        Ok(())
    }

    fn run_in_pane(&mut self, pane: &str, cmd: &str) -> Result<()> {
        // `herdr pane run` returns empty stdout on success, so it must not go
        // through the JSON-parsing `run` helper.
        Self::run_raw(&args(&["pane", "run", pane, cmd]))
    }

    fn close_pane(&mut self, pane: &str) -> Result<()> {
        Self::run(&args(&["pane", "close", pane]))?;
        Ok(())
    }

    fn pane_alive(&mut self, pane: &str) -> Result<bool> {
        Ok(Self::panes()?
            .iter()
            .any(|(pane_id, _tab_id)| pane_id == pane))
    }

    fn tab_infos(&mut self) -> Result<Vec<TabInfo>> {
        parse_tab_infos(&Self::run(&args(&["api", "snapshot"]))?)
    }

    fn read_pane(&mut self, pane: &str, lines: u32) -> Result<String> {
        Self::run_text(&args(&[
            "pane",
            "read",
            pane,
            "--source",
            "recent-unwrapped",
            "--lines",
            &lines.to_string(),
            "--format",
            "text",
        ]))
    }

    fn pane_cwd(&mut self, pane: &str) -> Result<PathBuf> {
        let value = Self::run(&args(&["pane", "get", pane]))?;
        Ok(PathBuf::from(string_at(
            &value,
            "/result/pane/foreground_cwd",
        )?))
    }

    fn pane_snapshot(&mut self, pane: &str) -> Result<PaneSnapshot> {
        let value = Self::run(&args(&["pane", "get", pane]))?;
        parse_pane_snapshot(&value)
    }

    fn agents(&mut self, workspace: &str) -> Result<Vec<AgentInfo>> {
        let value = Self::run(&args(&["agent", "list"]))?;
        let agents = value
            .pointer("/result/agents")
            .and_then(Value::as_array)
            .context("herdr agent list response missing result.agents array")?;
        agents
            .iter()
            // Only entries that actually carry an `agent` label are agents;
            // herdr also reports bare/undetected panes here (no `agent` field).
            .filter(|agent| agent.get("agent").and_then(Value::as_str).is_some())
            .filter(|agent| {
                agent.pointer("/workspace_id").and_then(Value::as_str) == Some(workspace)
            })
            .map(|agent| {
                Ok(AgentInfo {
                    pane_id: string_at(agent, "/pane_id")?.to_owned(),
                    tab_id: agent
                        .pointer("/tab_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    focused: agent
                        .get("focused")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                })
            })
            .collect()
    }
}

pub fn parse_pane_rects(json: &str) -> Result<Vec<PaneRect>> {
    let value: Value = serde_json::from_str(json).context("invalid pane layout JSON")?;
    parse_pane_rects_value(&value)
}

fn parse_pane_rects_value(value: &Value) -> Result<Vec<PaneRect>> {
    let layout = value
        .pointer("/result/layout")
        .context("pane layout response missing result.layout")?;
    let origin_x = u32_at(layout, "/area/x")?;
    let origin_y = u32_at(layout, "/area/y")?;
    let panes = layout
        .pointer("/panes")
        .and_then(Value::as_array)
        .context("pane layout response missing result.layout.panes array")?;

    panes
        .iter()
        .map(|pane| {
            let x = u32_at(pane, "/rect/x")?;
            let y = u32_at(pane, "/rect/y")?;
            Ok(PaneRect {
                pane_id: string_at(pane, "/pane_id")?.to_owned(),
                x: x.checked_sub(origin_x)
                    .context("pane rect x is outside the layout area")?,
                y: y.checked_sub(origin_y)
                    .context("pane rect y is outside the layout area")?,
                w: u32_at(pane, "/rect/width")?,
                h: u32_at(pane, "/rect/height")?,
            })
        })
        .collect()
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn string_at<'a>(value: &'a Value, pointer: &str) -> Result<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("JSON response missing string at {pointer}"))
}

fn u32_at(value: &Value, pointer: &str) -> Result<u32> {
    let number = value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("JSON response missing unsigned integer at {pointer}"))?;
    number
        .try_into()
        .with_context(|| format!("integer at {pointer} does not fit in u32"))
}

#[cfg(test)]
use std::collections::VecDeque;

#[cfg(test)]
#[derive(Default)]
pub struct MockHerdr {
    pub ops: Vec<String>,
    pub pane_rects_results: VecDeque<Result<Vec<PaneRect>>>,
    pub tab_of_pane_results: VecDeque<Result<String>>,
    pub create_tab_results: VecDeque<Result<(String, String)>>,
    pub split_pane_results: VecDeque<Result<String>>,
    pub pane_alive_results: VecDeque<Result<bool>>,
    pub tab_infos_results: VecDeque<Result<Vec<TabInfo>>>,
    pub read_pane_results: VecDeque<Result<String>>,
    pub pane_cwd_results: VecDeque<Result<PathBuf>>,
    pub agents_results: VecDeque<Result<Vec<AgentInfo>>>,
    pub pane_snapshot_results: VecDeque<Result<PaneSnapshot>>,
    /// The `ready_marker` path most recently passed to `open_sidebar_pane`,
    /// if any; used to probe on-disk marker existence from `close_pane` and
    /// `sync_pane_sizes`.
    pub(crate) ready_marker_seen: Option<PathBuf>,
}

#[cfg(test)]
impl MockHerdr {
    fn next<T>(queue: &mut VecDeque<Result<T>>, operation: &str) -> Result<T> {
        queue
            .pop_front()
            .unwrap_or_else(|| Err(anyhow!("no scripted response for {operation}")))
    }

    /// Records `{label}:{exists}` for the most recent ready marker, so tests
    /// can assert `maneuver::open` writes it only after every geometry op.
    /// No-op until `open_sidebar_pane` has recorded a marker path.
    fn probe_marker(&mut self, label: &str) {
        if let Some(marker) = &self.ready_marker_seen {
            self.ops.push(format!("{label}:{}", marker.exists()));
        }
    }
}

#[cfg(test)]
impl Herdr for MockHerdr {
    fn pane_rects(&mut self, tab: &str) -> Result<Vec<PaneRect>> {
        self.ops.push(format!("rects {tab}"));
        Self::next(&mut self.pane_rects_results, "pane_rects")
    }

    fn tab_of_pane(&mut self, pane: &str) -> Result<String> {
        self.ops.push(format!("tab_of {pane}"));
        Self::next(&mut self.tab_of_pane_results, "tab_of_pane")
    }

    fn create_tab(&mut self, workspace: &str) -> Result<(String, String)> {
        self.ops.push(format!("create_tab {workspace}"));
        Self::next(&mut self.create_tab_results, "create_tab")
    }

    fn move_pane(
        &mut self,
        pane: &str,
        tab: &str,
        dir: Dir,
        target: Option<&str>,
        ratio: Option<f64>,
        focus: bool,
    ) -> Result<()> {
        self.ops.push(format!(
            "move {pane} -> tab:{tab} dir:{dir:?} target:{} ratio:{} focus:{focus}",
            target.unwrap_or("-"),
            ratio.map_or_else(|| "-".to_owned(), |ratio| ratio.to_string())
        ));
        Ok(())
    }

    fn split_pane(&mut self, pane: &str, dir: Dir, ratio: f64, focus: bool) -> Result<String> {
        self.ops.push(format!(
            "split {pane} dir:{dir:?} ratio:{ratio} focus:{focus}"
        ));
        Self::next(&mut self.split_pane_results, "split_pane")
    }

    fn open_sidebar_pane(
        &mut self,
        anchor: &str,
        dir: Dir,
        cwd: &Path,
        focus: bool,
        ready_marker: &Path,
    ) -> Result<String> {
        self.ops.push(format!(
            "open_sidebar {anchor} dir:{dir:?} cwd:{} marker:{} focus:{focus}",
            cwd.display(),
            ready_marker.display()
        ));
        self.ready_marker_seen = Some(ready_marker.to_path_buf());
        Self::next(&mut self.split_pane_results, "open_sidebar_pane")
    }

    fn sync_pane_sizes(&mut self, pane: &str) -> Result<()> {
        self.ops.push(format!("sync_sizes {pane}"));
        // The winsize resync is a geometry-affecting op, so it too must land
        // before the ready marker (see `close_pane`).
        self.probe_marker("marker_exists_at_sync");
        Ok(())
    }

    fn run_in_pane(&mut self, pane: &str, cmd: &str) -> Result<()> {
        self.ops.push(format!("run {pane} {cmd}"));
        Ok(())
    }

    fn close_pane(&mut self, pane: &str) -> Result<()> {
        self.ops.push(format!("close {pane}"));
        // `maneuver::open` must write the ready marker only after every
        // geometry-affecting op, including this close.
        self.probe_marker("marker_exists_at_close");
        Ok(())
    }

    fn pane_alive(&mut self, pane: &str) -> Result<bool> {
        self.ops.push(format!("alive {pane}"));
        Self::next(&mut self.pane_alive_results, "pane_alive")
    }

    fn tab_infos(&mut self) -> Result<Vec<TabInfo>> {
        self.ops.push("tab_infos".to_owned());
        Self::next(&mut self.tab_infos_results, "tab_infos")
    }

    fn read_pane(&mut self, pane: &str, lines: u32) -> Result<String> {
        self.ops.push(format!("read_pane {pane} {lines}"));
        Self::next(&mut self.read_pane_results, "read_pane")
    }

    fn pane_cwd(&mut self, pane: &str) -> Result<PathBuf> {
        self.ops.push(format!("pane_cwd {pane}"));
        Self::next(&mut self.pane_cwd_results, "pane_cwd")
    }

    fn agents(&mut self, workspace: &str) -> Result<Vec<AgentInfo>> {
        self.ops.push(format!("agents {workspace}"));
        Self::next(&mut self.agents_results, "agents")
    }

    fn pane_snapshot(&mut self, pane: &str) -> Result<PaneSnapshot> {
        self.ops.push(format!("pane_snapshot {pane}"));
        Self::next(&mut self.pane_snapshot_results, "pane_snapshot")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pane_rects_from_layout_fixture() {
        let json = include_str!("../tests/fixtures/layout_3pane.json");
        let rects = parse_pane_rects(json).unwrap();

        assert_eq!(rects.len(), 3);
        assert_eq!(
            rects
                .iter()
                .filter(|rect| rect.x == 0 && rect.y == 0)
                .count(),
            1
        );
        assert_eq!(
            rects[0],
            PaneRect {
                pane_id: "w0:p1".to_owned(),
                x: 0,
                y: 0,
                w: 72,
                h: 52,
            }
        );
    }

    #[test]
    fn single_pane_layout_parses() {
        let json = include_str!("../tests/fixtures/layout_1pane.json");
        assert_eq!(parse_pane_rects(json).unwrap().len(), 1);
    }

    #[test]
    fn parses_pane_get_agent_session_when_present() {
        let json = include_str!("../tests/fixtures/pane_get_with_session.json");
        let value: Value = serde_json::from_str(json).unwrap();
        let session = parse_agent_session(&value).unwrap();
        assert_eq!(session.agent, "pi");
        assert_eq!(session.kind, "path");
        assert!(session.value.ends_with(".jsonl"));
    }

    #[test]
    fn returns_none_when_agent_session_field_absent() {
        let json = include_str!("../tests/fixtures/pane_get_without_session.json");
        let value: Value = serde_json::from_str(json).unwrap();
        assert!(parse_agent_session(&value).is_none());
    }

    #[test]
    fn parses_pane_snapshot_cwd_session_and_scroll_from_one_response() {
        let json = include_str!("../tests/fixtures/pane_get_with_session.json");
        let value: Value = serde_json::from_str(json).unwrap();
        let snapshot = parse_pane_snapshot(&value).unwrap();
        assert_eq!(snapshot.cwd, PathBuf::from("/repo"));
        assert_eq!(snapshot.agent_session.unwrap().agent, "pi");
        assert_eq!(
            snapshot.scroll,
            Some(PaneScroll {
                viewport_rows: 73,
                max_offset_from_bottom: 0,
            })
        );
    }

    #[test]
    fn parses_pane_snapshot_cwd_with_no_session_or_scroll() {
        let json = include_str!("../tests/fixtures/pane_get_without_session.json");
        let value: Value = serde_json::from_str(json).unwrap();
        let snapshot = parse_pane_snapshot(&value).unwrap();
        assert_eq!(snapshot.cwd, PathBuf::from("/repo"));
        assert!(snapshot.agent_session.is_none());
        assert!(snapshot.scroll.is_none());
    }

    #[test]
    fn tab_infos_join_workspace_labels_and_tolerate_missing_names() {
        let value = serde_json::json!({"result": {"snapshot": {
            "workspaces": [{"workspace_id": "w26", "label": "novu"}],
            "tabs": [
                {"tab_id": "w26:t2", "workspace_id": "w26", "label": "api", "number": 2},
                {"tab_id": "w9:t1", "label": ""},
            ],
        }}});
        let tabs = parse_tab_infos(&value).unwrap();
        assert_eq!(
            tabs,
            [
                TabInfo {
                    tab_id: TabId::new("w26:t2"),
                    workspace_label: Some("novu".to_owned()),
                    tab_label: Some("api".to_owned()),
                    tab_number: Some(2),
                },
                TabInfo {
                    tab_id: TabId::new("w9:t1"),
                    workspace_label: None,
                    tab_label: None,
                    tab_number: None,
                },
            ]
        );
    }

    #[test]
    fn mock_records_canonical_move_operation() {
        let mut herdr = MockHerdr::default();
        herdr
            .move_pane("p2", "t9", Dir::Right, Some("p1"), Some(0.4), false)
            .unwrap();

        assert_eq!(
            herdr.ops,
            ["move p2 -> tab:t9 dir:Right target:p1 ratio:0.4 focus:false"]
        );
    }
}
