//! `Herdr` for the desktop app: panes are herdr-core terminal addresses
//! (`t7`), tabs are `<space>:<tab>` (see `app::tab_id`), and everything is
//! answered by herdr-core (`hc`), so it works whether or not the window is
//! open. The TUI's layout surgery (moving and parking panes) has no
//! counterpart: the app splits a whole tab in one command (`app::sidebar`).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::herdr::{AgentInfo, Dir, Herdr, PaneRect, PaneSnapshot, TabInfo};

use super::{doc, rpc, tab_id};

#[derive(Default)]
pub struct AppHost;

fn unsupported<T>(what: &str) -> Result<T> {
    bail!("{what} is a Herdr TUI operation; the Herdr app host does not use it")
}

/// A live terminal's facts (`terminal.get`).
pub(crate) fn terminal(address: &str) -> Result<Value> {
    rpc::call("terminal.get", json!({"terminal": address}))
}

/// Its tab as `<space>:<tab>`, from the labels the app puts on its terminals.
pub(crate) fn tab_of(facts: &Value) -> Option<String> {
    let space = facts.pointer("/labels/herdr.space")?.as_str()?;
    let tab = facts.pointer("/labels/herdr.tab")?.as_str()?;
    Some(tab_id(space, tab))
}

/// Where the program runs now, else where the terminal started.
pub(crate) fn cwd_of(facts: &Value) -> Option<PathBuf> {
    facts
        .pointer("/foreground/cwd")
        .or_else(|| facts.get("cwd"))
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
}

/// Pure: the agents among `terminal.list`'s terminals. An agent is a terminal
/// whose program reports a status (OSC 7501), as in the app's agents list;
/// only terminals the app shows (labelled with a space and tab) count.
pub(crate) fn agents_in(list: &Value) -> Vec<(AgentInfo, Value)> {
    list.get("terminals")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter(|t| t.get("state").and_then(Value::as_str) == Some("live"))
        .filter(|t| t.get("status").is_some_and(Value::is_object))
        .filter_map(|t| {
            Some((
                AgentInfo {
                    pane_id: t.get("address")?.as_str()?.to_owned(),
                    tab_id: tab_of(t)?,
                    focused: false,
                },
                t.clone(),
            ))
        })
        .collect()
}

pub(crate) fn list_terminals() -> Result<Value> {
    rpc::call("terminal.list", json!({}))
}

impl Herdr for AppHost {
    fn pane_rects(&mut self, _tab: &str) -> Result<Vec<PaneRect>> {
        unsupported("pane layout")
    }

    fn tab_of_pane(&mut self, pane: &str) -> Result<String> {
        tab_of(&terminal(pane)?).with_context(|| format!("terminal {pane} is in no tab"))
    }

    fn create_tab(&mut self, _workspace: &str) -> Result<(String, String)> {
        unsupported("parking tab")
    }

    fn move_pane(
        &mut self,
        _pane: &str,
        _tab: &str,
        _dir: Dir,
        _target: Option<&str>,
        _ratio: Option<f64>,
        _focus: bool,
    ) -> Result<()> {
        unsupported("pane move")
    }

    fn split_pane(&mut self, _pane: &str, _dir: Dir, _ratio: f64, _focus: bool) -> Result<String> {
        unsupported("pane split")
    }

    fn open_sidebar_pane(
        &mut self,
        _anchor: &str,
        _dir: Dir,
        _cwd: &Path,
        _focus: bool,
        _ready_marker: &Path,
    ) -> Result<String> {
        unsupported("plugin pane")
    }

    fn sync_pane_sizes(&mut self, _pane: &str) -> Result<()> {
        unsupported("pane resize")
    }

    fn run_in_pane(&mut self, _pane: &str, _cmd: &str) -> Result<()> {
        unsupported("pane run")
    }

    fn close_pane(&mut self, pane: &str) -> Result<()> {
        super::sidebar::close(pane)
    }

    fn pane_alive(&mut self, pane: &str) -> Result<bool> {
        Ok(terminal(pane)
            .map(|facts| facts.get("state").and_then(Value::as_str) == Some("live"))
            .unwrap_or(false))
    }

    fn tab_infos(&mut self) -> Result<Vec<TabInfo>> {
        let doc = doc::read()?.context("herdr-core has no Herdr app layout document")?;
        Ok(doc::tab_infos(&doc))
    }

    fn read_pane(&mut self, pane: &str, lines: u32) -> Result<String> {
        let output = rpc::hc()
            .args(["read", pane, "--lines", &lines.to_string()])
            .stdin(Stdio::null())
            .output()
            .context("failed to run hc read")?;
        if !output.status.success() {
            bail!(
                "hc read {pane} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn pane_cwd(&mut self, pane: &str) -> Result<PathBuf> {
        cwd_of(&terminal(pane)?).with_context(|| format!("terminal {pane} has no folder"))
    }

    fn agents(&mut self, workspace: &str) -> Result<Vec<AgentInfo>> {
        let prefix = format!("{workspace}:");
        Ok(agents_in(&list_terminals()?)
            .into_iter()
            .map(|(agent, _)| agent)
            .filter(|agent| agent.tab_id.starts_with(&prefix))
            .collect())
    }

    fn pane_snapshot(&mut self, pane: &str) -> Result<PaneSnapshot> {
        // herdr-core knows no agent session file and serves `hc read` from its
        // own scrollback, so there is nothing to clamp: the picker uses its
        // git and scraped-output tiers.
        Ok(PaneSnapshot {
            cwd: self.pane_cwd(pane)?,
            agent_session: None,
            scroll: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agents_are_live_reporting_terminals_the_app_shows() {
        // Shapes from herdr-core 0.1.16's terminal.list.
        let list = json!({"terminals": [
            {"address": "t1", "state": "live", "status": null,
             "labels": {"herdr.space": "spA", "herdr.tab": "tbA"}},
            {"address": "t5", "state": "live", "status": {"app": "pi", "state": "idle"},
             "labels": {"herdr.space": "spA", "herdr.tab": "tbB"}},
            {"address": "t6", "state": "live", "status": {"app": "pi", "state": "working"},
             "labels": {}},
            {"address": "t7", "state": "finished", "status": {"app": "pi", "state": "idle"},
             "labels": {"herdr.space": "spA", "herdr.tab": "tbB"}}
        ]});
        let agents: Vec<AgentInfo> = agents_in(&list).into_iter().map(|(a, _)| a).collect();
        assert_eq!(
            agents,
            [AgentInfo {
                pane_id: "t5".to_owned(),
                tab_id: "spA:tbB".to_owned(),
                focused: false,
            }]
        );
    }

    #[test]
    fn cwd_prefers_the_foreground_program() {
        let facts = json!({"cwd": "/tmp/x", "foreground": {"cwd": "/private/tmp/x/sub"}});
        assert_eq!(cwd_of(&facts), Some(PathBuf::from("/private/tmp/x/sub")));
        assert_eq!(
            cwd_of(&json!({"cwd": "/tmp/x"})),
            Some(PathBuf::from("/tmp/x"))
        );
    }
}
