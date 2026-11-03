//! `Herdr` for the desktop app: panes are herdr-core terminal addresses
//! (`t7`), tabs are `<space>:<tab>` (see `app::tab_id`), and everything is
//! answered by herdr-core (`hc`), so it works whether or not the window is
//! open. The TUI's layout surgery (`herdr::Layout`) has no counterpart: the
//! app splits a whole tab in one command (`app::sidebar`).

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::{
    config::Sidebar,
    daemon,
    herdr::{AgentInfo, Herdr, PaneSnapshot, TabInfo},
    host::{HostOps, Opened, Pick},
    maneuver::Ctx,
};

use super::{doc, rpc, tab_id};

#[derive(Default)]
pub struct AppHost;

/// The Herdr app's side of `host::HostOps`.
pub struct App;

impl HostOps for App {
    fn herdr(&self) -> Box<dyn Herdr> {
        Box::new(AppHost)
    }

    /// The mod passes the focused tab as `--ctx JSON`.
    fn ctx(&self) -> Result<Ctx> {
        let args: Vec<String> = std::env::args().skip(2).collect();
        super::sidebar::ctx(&super::cli::read_ctx(&args)?)
    }

    fn show_pick(&self, pick: Pick) -> Result<()> {
        super::commands::show_pick(pick)
    }

    fn ensure_sidebar(&self, ctx: &Ctx) -> Result<Opened> {
        super::sidebar::ensure_open(ctx)
    }

    fn focus(&self, pane: &str) {
        if let Err(error) = super::sidebar::focus(pane) {
            eprintln!("herdr-nvim: could not focus sidebar {pane}: {error:#}");
        }
    }

    fn daemon_env(&self, command: &mut Command, tab: &str, plugin_root: &Path) {
        daemon::app_daemon(command, tab, daemon::runtime_stamp(plugin_root));
    }

    fn socket_subdir(&self) -> Option<&'static str> {
        Some("app")
    }

    fn sidebar_identity(&self) -> Result<(String, String, PathBuf)> {
        let (tab, pane, cwd) = super::sidebar::identity()?;
        // Opened by the mod (the controller's swap), not by `sidebar::open`,
        // which checks before it splits: check here, before the daemon is used.
        if std::env::args().any(|arg| arg == "--refresh") {
            if let Err(error) = super::stale::refresh_on_open(tab.as_str(), &cwd) {
                eprintln!("herdr-nvim: could not check this tab's nvim for updates: {error:#}");
            }
        }
        Ok((tab.as_str().to_owned(), pane, cwd))
    }

    fn sidebar_ready(&self, socket: &Path, sidebar: &Sidebar) {
        super::watch::ensure_running();
        if crate::config::load().app.tree == crate::config::TreeMode::Nvim {
            // nvim's own explorer, once this window is attached (UIEnter).
            let _ = daemon::remote_expr(
                socket,
                "luaeval(\"require('herdr-nvim.app').explore_on_ui()\")",
                sidebar,
            );
        }
    }
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
    fn tab_infos(&mut self) -> Result<Vec<TabInfo>> {
        let doc = doc::read()?.context("herdr-core has no Herdr app layout document")?;
        Ok(doc::tab_infos(&doc))
    }

    /// The screen only, whatever `lines` asks: for history herdr-core scrolls
    /// a full-screen program (pi's chat) back and forth while it is idle,
    /// which took 5-6 s per `cmd-o` and moved the agent's view. The agent's
    /// session log is where the older files come from.
    fn read_pane(&mut self, pane: &str, _lines: u32) -> Result<String> {
        let read = rpc::call("terminal.read", json!({"terminal": pane, "lines": 0}))?;
        Ok(read
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned())
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
        // The agent's session log is found from its program and folder
        // (`session`); herdr-core serves `hc read` from its own scrollback, so
        // there is no scroll depth to clamp.
        let facts = terminal(pane)?;
        Ok(PaneSnapshot {
            cwd: cwd_of(&facts).with_context(|| format!("terminal {pane} has no folder"))?,
            agent_session: super::session::find(&facts),
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
