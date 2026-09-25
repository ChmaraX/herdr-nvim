//! herdr's `tab.closed` / `workspace.closed` hooks (`herdr-nvim on-event`):
//! stop the closed tabs' daemons right away.

use std::env;

use anyhow::Result;
use serde_json::Value;

use crate::{config::Sidebar, state::TabId};

use super::registry::{all_stopped, daemon_keys, stop_keys};

/// A close event herdr delivered to the `on-event` hook.
#[derive(Debug, PartialEq)]
enum CloseEvent {
    Tab(TabId),
    Workspace(String),
}

/// Parse `HERDR_PLUGIN_EVENT_JSON`, which herdr sends as
/// `{"event":"tab_closed","data":{"type":"tab_closed","tab_id":..,"workspace_id":..}}`.
/// Anything that is not a well-formed `tab_closed`/`workspace_closed` event
/// yields `None` (ignored).
fn parse_close_event(raw: &str) -> Option<CloseEvent> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let id = |pointer: &str| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    };
    match id("/data/type")?.as_str() {
        "tab_closed" => id("/data/tab_id").map(|tab| CloseEvent::Tab(TabId::new(tab))),
        "workspace_closed" => id("/data/workspace_id").map(CloseEvent::Workspace),
        _ => None,
    }
}

/// Stop the daemon(s) a close event makes obsolete. A workspace close does
/// not fire `tab_closed` for its tabs, so it stops every daemon of that
/// workspace. Best effort: one daemon failing to stop does not spare the rest.
fn handle_close_event(event: &CloseEvent, sidebar: &Sidebar) -> Result<()> {
    let keys = match event {
        CloseEvent::Tab(tab) => vec![tab.key()],
        CloseEvent::Workspace(workspace) => daemon_keys()?
            .into_iter()
            .filter(|key| TabId::key_in_workspace(key, workspace))
            .collect(),
    };
    all_stopped(stop_keys(&keys, sidebar))
}

/// herdr `tab.closed` / `workspace.closed` event hook: free the closed tab's
/// (or every closed-workspace tab's) daemon right away instead of waiting for
/// the next toggle's gc. Irrelevant or unparseable events are a silent no-op.
pub fn on_event_cmd() -> Result<()> {
    let Some(event) = env::var("HERDR_PLUGIN_EVENT_JSON")
        .ok()
        .as_deref()
        .and_then(parse_close_event)
    else {
        return Ok(());
    };
    let config = crate::config::load();
    handle_close_event(&event, &config.sidebar)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        test_support::{nvim_available, TestDaemon, TestEnv},
    };

    // The event-hook scenario at the library level: real daemons, close
    // events as herdr delivers them, observed through the actual processes
    // and files (tests/on_event_cli.rs drives the same through the binary).
    // `wHnAB` is the prefix trap -- closing workspace `wHnA` must not touch it.
    #[test]
    fn close_events_stop_exactly_the_closed_tabs_daemons() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let _env = TestEnv::new();
        let config = Config::default();
        let sidebar = &config.sidebar;

        let a1 = TestDaemon::spawn("wHnA:t1", &config);
        let a2 = TestDaemon::spawn("wHnA:t2", &config);
        let ab1 = TestDaemon::spawn("wHnAB:t1", &config);
        let b1 = TestDaemon::spawn("wHnB:t1", &config);
        // Unsaved work in the closing tab is discarded, not a blocker.
        a1.dirty_a_buffer();
        for daemon in [&a1, &a2, &ab1, &b1] {
            daemon.assert_alive();
        }

        let deliver = |raw: &str| {
            if let Some(event) = parse_close_event(raw) {
                handle_close_event(&event, sidebar).expect("handle event");
            }
        };

        deliver(
            r#"{"event":"tab_closed","data":{"type":"tab_closed","tab_id":"wHnA:t1","workspace_id":"wHnA"}}"#,
        );
        a1.assert_gone();
        for daemon in [&a2, &ab1, &b1] {
            daemon.assert_alive();
        }

        deliver(
            r#"{"event":"workspace_closed","data":{"type":"workspace_closed","workspace_id":"wHnA","workspace":{"workspace_id":"wHnA","label":"x"}}}"#,
        );
        a2.assert_gone();
        ab1.assert_alive();
        b1.assert_alive();

        // Irrelevant events, and closes of things that have no daemon.
        deliver(
            r#"{"event":"pane_focused","data":{"type":"pane_focused","pane_id":"wHnB:p1","workspace_id":"wHnB"}}"#,
        );
        deliver(
            r#"{"event":"tab_closed","data":{"type":"tab_closed","tab_id":"wHnA:t1","workspace_id":"wHnA"}}"#,
        );
        deliver(
            r#"{"event":"workspace_closed","data":{"type":"workspace_closed","workspace_id":"wHnZ"}}"#,
        );
        ab1.assert_alive();
        b1.assert_alive();
    }

    #[test]
    fn parses_close_events_from_data_type() {
        assert_eq!(
            parse_close_event(
                r#"{"event":"tab_closed","data":{"type":"tab_closed","tab_id":"w1:t1","workspace_id":"w1"}}"#
            ),
            Some(CloseEvent::Tab(TabId::new("w1:t1")))
        );
        assert_eq!(
            parse_close_event(
                r#"{"event":"workspace_closed","data":{"type":"workspace_closed","workspace_id":"w1"}}"#
            ),
            Some(CloseEvent::Workspace("w1".to_owned()))
        );
    }

    #[test]
    fn malformed_or_foreign_event_json_is_ignored() {
        for raw in [
            "",
            "not json",
            "{}",
            "[]",
            r#"{"event":"tab_closed"}"#,
            // Only `data.type` names the event.
            r#"{"event":"tab_closed","data":{"tab_id":"w1:t1"}}"#,
            r#"{"data":{"type":"tab_closed","tab_id":""}}"#,
            r#"{"data":{"type":"tab_closed","tab_id":7}}"#,
            r#"{"data":{"type":"workspace_closed"}}"#,
            r#"{"data":{"type":"tab_created","tab_id":"w1:t1"}}"#,
        ] {
            assert_eq!(parse_close_event(raw), None, "{raw:?}");
        }
    }
}
