//! `herdr-nvim on-event` as herdr invokes it: the real binary, with the event
//! payload in `HERDR_PLUGIN_EVENT_JSON`, against real nvim daemons in a
//! sandboxed runtime dir (fake tab ids; no real daemon is ever touched).
//! Whatever herdr hands the hook, it must exit 0 quietly unless it genuinely
//! failed to stop a daemon -- a noisy or failing hook for an irrelevant event
//! would spam herdr's plugin log.
//!
//! Failure modes, and where each is covered:
//! - Prefix trap: closing workspace `wHnA` must spare `wHnAB`'s daemons.
//!   Covered: `close_hooks_stop_exactly_the_closed_tabs_daemons`.
//! - Unresponsive daemon / stale registry entry (a daemon that died without
//!   cleanup; on Windows a `.pipe` marker with no pipe): the close removes
//!   the entry. Covered: same test (`wHnA:t9`).
//! - Quit timeout (a daemon that ignores `qa!`): the hook fails, and the
//!   daemon keeps its registry entry and state so it stays listable and
//!   reachable. Covered: same test (`wHnA:t3`, swallows every key).
//! - Partial failure on workspace close: the other daemons of the workspace
//!   are still stopped. Covered: same test.
//! - Unsaved buffers in a closing tab: discarded, reported on stderr.
//!   Covered: same test (`wHnA:t1`).
//! - Missing, malformed or irrelevant events, closes of tabs/workspaces
//!   without a daemon: silent no-op. Covered:
//!   `on_event_exits_cleanly_for_missing_malformed_and_irrelevant_events`.
//! - herdr unreachable: not a failure mode here -- the hook never asks
//!   herdr (see tests/daemons_cli.rs for `daemon-gc` and `daemons`).
//!
//! The E2E test writes its steps (events, exit codes, output, before/after
//! `daemons --json`) to `target/test-artifacts/<test>/log.md`.

mod support;

use support::{nvim_available, Artifact, Daemon, Sandbox};

fn tab_closed(tab: &str) -> String {
    let workspace = tab.split(':').next().unwrap();
    format!(
        r#"{{"event":"tab_closed","data":{{"type":"tab_closed","tab_id":"{tab}","workspace_id":"{workspace}"}}}}"#
    )
}

fn workspace_closed(workspace: &str) -> String {
    format!(
        r#"{{"event":"workspace_closed","data":{{"type":"workspace_closed","workspace_id":"{workspace}"}}}}"#
    )
}

#[test]
fn close_hooks_stop_exactly_the_closed_tabs_daemons() {
    if !nvim_available() {
        eprintln!("skipping: nvim not found on PATH");
        return;
    }
    let sandbox = Sandbox::new("on-event-e2e");
    let mut log = Artifact::new("close_hooks_stop_exactly_the_closed_tabs_daemons");
    let mut a1 = Daemon::spawn(&sandbox, "wHnA:t1");
    let mut a2 = Daemon::spawn(&sandbox, "wHnA:t2");
    let mut stubborn = Daemon::spawn_unquittable(&sandbox, "wHnA:t3");
    let stale = sandbox.stale_entry("wHnA:t9");
    let mut ab1 = Daemon::spawn(&sandbox, "wHnAB:t1");
    let mut b1 = Daemon::spawn(&sandbox, "wHnB:t1");
    a1.eval(r#"execute('setlocal noswapfile | call setline(1, "unsaved")')"#)
        .expect("dirty a buffer");
    sandbox.herdr_tabs(&[
        ("wHnA:t1", "alpha", "one"),
        ("wHnA:t2", "alpha", "two"),
        ("wHnA:t3", "alpha", "stubborn"),
        ("wHnAB:t1", "alphabet", "one"),
        ("wHnB:t1", "beta", "one"),
    ]);
    log.step(
        "before: daemons --json",
        "",
        &sandbox.run(&["daemons", "--json"], None),
    );

    let event = tab_closed("wHnA:t1");
    let output = sandbox.run(&["on-event"], Some(&event));
    log.step("tab_closed wHnA:t1", &event, &output);
    assert!(output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("discarding 1 unsaved buffer"),
        "unsaved work is reported"
    );
    a1.assert_gone(&sandbox);
    for daemon in [&mut a2, &mut stubborn, &mut ab1, &mut b1] {
        daemon.assert_alive(&sandbox);
    }

    let event = workspace_closed("wHnA");
    let output = sandbox.run(&["on-event"], Some(&event));
    log.step("workspace_closed wHnA", &event, &output);
    assert!(
        !output.status.success(),
        "a daemon that won't quit fails the hook"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("wHnA_t3"), "{stderr}");
    a2.assert_gone(&sandbox);
    assert!(!stale.exists(), "stale entry cleaned up");
    stubborn.assert_alive(&sandbox);
    ab1.assert_alive(&sandbox);
    b1.assert_alive(&sandbox);

    sandbox.herdr_tabs(&[("wHnAB:t1", "alphabet", "one"), ("wHnB:t1", "beta", "one")]);
    let after = sandbox.run(&["daemons", "--json"], None);
    log.step("after: daemons --json", "", &after);
    let listed: serde_json::Value = serde_json::from_slice(&after.stdout).expect("json");
    let keys: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["wHnAB_t1", "wHnA_t3", "wHnB_t1"]);
    log.note("result: pass");
}

#[test]
fn on_event_exits_cleanly_for_missing_malformed_and_irrelevant_events() {
    let sandbox = Sandbox::new("on-event-noop");
    for event_json in [
        None,
        Some(""),
        Some("{not json"),
        Some(r#"{"event":"tab_closed","data":{}}"#),
        Some(r#"{"event":"pane_focused","data":{"type":"pane_focused","pane_id":"w1:p1"}}"#),
        // Well-formed closes for tabs/workspaces that never had a daemon.
        Some(&tab_closed("wNone:t1")),
        Some(&workspace_closed("wNone")),
    ] {
        let output = sandbox.run(&["on-event"], event_json);
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
