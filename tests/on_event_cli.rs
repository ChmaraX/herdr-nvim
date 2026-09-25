//! `herdr-nvim on-event` as herdr invokes it: the real binary, with the event
//! payload in `HERDR_PLUGIN_EVENT_JSON`. Whatever herdr hands the hook, it
//! must exit 0 quietly unless it genuinely failed to stop a daemon -- a noisy
//! or failing hook for an irrelevant event would spam herdr's plugin log.

use std::{env, path::PathBuf, process::Command};

fn run_on_event(event_json: Option<&str>) -> std::process::Output {
    // Point every path the hook could touch at a directory that does not
    // exist, so no real daemon or state file is ever at risk.
    let sandbox: PathBuf = env::temp_dir().join(format!("hn-on-event-cli-{}", std::process::id()));
    let mut command = Command::new(env!("CARGO_BIN_EXE_herdr-nvim"));
    command
        .arg("on-event")
        .env("HERDR_NVIM_RUNTIME_DIR", sandbox.join("runtime"))
        .env("HERDR_NVIM_STATE_DIR", sandbox.join("state"))
        .env_remove("HERDR_PLUGIN_EVENT_JSON");
    if let Some(json) = event_json {
        command.env("HERDR_PLUGIN_EVENT_JSON", json);
    }
    command.output().expect("run herdr-nvim on-event")
}

#[test]
fn on_event_exits_cleanly_for_missing_malformed_and_irrelevant_events() {
    for event_json in [
        None,
        Some(""),
        Some("{not json"),
        Some(r#"{"event":"tab_closed","data":{}}"#),
        Some(r#"{"event":"pane_focused","data":{"type":"pane_focused","pane_id":"w1:p1"}}"#),
        // Well-formed closes for tabs/workspaces that never had a daemon.
        Some(
            r#"{"event":"tab_closed","data":{"type":"tab_closed","tab_id":"wNone:t1","workspace_id":"wNone"}}"#,
        ),
        Some(
            r#"{"event":"workspace_closed","data":{"type":"workspace_closed","workspace_id":"wNone"}}"#,
        ),
    ] {
        let output = run_on_event(event_json);
        assert!(
            output.status.success(),
            "{event_json:?}: exit {:?}",
            output.status
        );
        assert!(
            output.stderr.is_empty(),
            "{event_json:?}: unexpected stderr {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
