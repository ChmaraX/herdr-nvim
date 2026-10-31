//! `herdr-nvim daemons` and `herdr-nvim daemon-gc` as a user (or herdr's
//! `[[startup]]` hook) runs them: the real binary against a sandboxed runtime
//! dir and a fake `herdr` on PATH, so no real daemon or tab is ever touched.
//!
//! Failure modes, and where each is covered:
//! - herdr unreachable: `daemons` still lists (state `unknown`, no names);
//!   `daemons stop --orphans` and `daemon-gc` fail and stop nothing, since
//!   every daemon would look orphaned. Covered:
//!   `lists_reaps_and_refuses_against_real_daemons` (Unix only).
//! - Stale registry entry (a daemon that died without cleanup): listed as
//!   `stale`, reaped as an orphan once herdr says its tab is gone. Covered:
//!   same test.
//! - Hung daemon (its socket takes connections, nothing answers; issue
//!   #61): `daemons` lists it as `unresponsive` with its pid within the
//!   request timeout, a plain `daemons stop` refuses it (exit 1) and keeps
//!   it, `--force` kills it by its pid. Covered:
//!   `a_hung_daemon_is_listed_refused_then_killed_with_force` (Unix only).
//! - Process table unreadable (`ps` fails): a refused socket cannot be
//!   told from a hung daemon whose listen queue is full, so nothing is
//!   taken for gone: the stale entry stays, a stop of it fails, and so does
//!   the close hook. Covered:
//!   `nothing_is_forgotten_while_the_process_table_cannot_be_read` (Unix).
//! - One daemon's probe fails (it answers with an error): that row says
//!   `error` with the reason, the others are listed and stopped as usual,
//!   and nothing, not even `--force`, stops the failing one. Covered:
//!   `a_daemon_whose_probe_fails_spoils_only_its_own_row`.
//! - Prefix trap on `stop <tab>`: only that key, never a longer id sharing
//!   its prefix. Covered: same test (`wHnDa:t1` vs `wHnDa:t10`).
//! - Quit timeout and partial failure: covered through the shared stop path
//!   in tests/on_event_cli.rs and `registry::tests`.
//! - Unknown tab, bad arguments, empty registry. Covered:
//!   `empty_listing_in_text_and_json`,
//!   `stopping_an_unknown_tab_or_bad_arguments_fail`.
//! - Not covered on Windows: the fake herdr is a shell script, and
//!   `Command::new("herdr")` only resolves `.exe` there.
//!
//! The E2E tests write their steps to `target/test-artifacts/<test>/log.md`.

mod support;

use support::Sandbox;

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(unix)]
fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn empty_listing_in_text_and_json() {
    let sandbox = Sandbox::new("daemons-empty");
    let text = sandbox.run(&["daemons"], None);
    assert!(text.status.success());
    assert_eq!(stdout(&text), "no nvim daemons running\n");

    let json = sandbox.run(&["daemons", "--json"], None);
    assert!(json.status.success());
    let value: serde_json::Value = serde_json::from_str(&stdout(&json)).expect("valid JSON");
    assert_eq!(value, serde_json::json!([]));
}

#[test]
fn stopping_an_unknown_tab_or_bad_arguments_fail() {
    let sandbox = Sandbox::new("daemons-bad");
    let unknown = sandbox.run(&["daemons", "stop", "wNone:t1"], None);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("wNone:t1"));

    let bad = sandbox.run(&["daemons", "stop"], None);
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("usage: herdr-nvim daemons"));

    let nothing = sandbox.run(&["daemons", "stop", "--all"], None);
    assert!(nothing.status.success());
    assert_eq!(stdout(&nothing), "no nvim daemons running\n");
}

#[cfg(unix)]
#[test]
fn lists_reaps_and_refuses_against_real_daemons() {
    use support::{nvim_available, Artifact, Daemon};

    if !nvim_available() {
        eprintln!("skipping: nvim not found on PATH");
        return;
    }
    let states = |output: &std::process::Output| -> Vec<(String, String)> {
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|d| {
                (
                    d["key"].as_str().unwrap().to_owned(),
                    d["state"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    };
    let pairs = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, s)| ((*k).to_owned(), (*s).to_owned()))
            .collect()
    };

    let sandbox = Sandbox::new("daemons-e2e");
    let mut log = Artifact::new("lists_reaps_and_refuses_against_real_daemons");
    let mut open = Daemon::spawn(&sandbox, "wHnDa:t1");
    let mut sibling = Daemon::spawn(&sandbox, "wHnDa:t10");
    let mut orphan = Daemon::spawn(&sandbox, "wHnDc:t1");
    let stale = sandbox.stale_entry("wHnDs:t1");

    // herdr unreachable: listed, but nothing is judged an orphan.
    sandbox.herdr_unreachable();
    let blind = sandbox.run(&["daemons", "--json"], None);
    log.step("herdr unreachable: daemons --json", "", &blind);
    assert!(blind.status.success());
    assert_eq!(
        states(&blind),
        pairs(&[
            ("wHnDa_t1", "unknown"),
            ("wHnDa_t10", "unknown"),
            ("wHnDc_t1", "unknown"),
            ("wHnDs_t1", "stale"),
        ])
    );
    for args in [&["daemons", "stop", "--orphans"][..], &["daemon-gc"]] {
        let output = sandbox.run(args, None);
        log.step(
            &format!("herdr unreachable: {}", args.join(" ")),
            "",
            &output,
        );
        assert_eq!(output.status.code(), Some(1), "{args:?}");
    }
    orphan.assert_alive(&sandbox);
    assert!(stale.exists());

    // herdr reachable: names, orphan detection, and the action hints.
    sandbox.herdr_tabs(&[("wHnDa:t1", "novu", "api"), ("wHnDa:t10", "novu", "web")]);
    let listed = sandbox.run(&["daemons", "--json"], None);
    log.step("before: daemons --json", "", &listed);
    assert_eq!(
        states(&listed),
        pairs(&[
            ("wHnDa_t1", "alive"),
            ("wHnDa_t10", "alive"),
            ("wHnDc_t1", "orphaned"),
            ("wHnDs_t1", "stale"),
        ])
    );
    let table = sandbox.run(&["daemons"], None);
    log.step("daemons", "", &table);
    let table = stdout(&table);
    assert!(table.contains("novu / api"), "{table}");
    assert!(table.contains("daemons stop --all"), "{table}");
    assert!(table.contains("daemons stop --orphans"), "{table}");

    // One tab by id: its sibling with the longer id survives.
    let output = sandbox.run(&["daemons", "stop", "wHnDa:t1"], None);
    log.step("daemons stop wHnDa:t1", "", &output);
    assert!(output.status.success());
    open.assert_gone(&sandbox);
    sibling.assert_alive(&sandbox);

    // The startup hook reaps the orphan and the stale entry, not open tabs.
    let output = sandbox.run(&["daemon-gc"], None);
    log.step("daemon-gc", "", &output);
    assert!(output.status.success());
    orphan.assert_gone(&sandbox);
    assert!(!stale.exists(), "stale entry reaped");
    sibling.assert_alive(&sandbox);

    let after = sandbox.run(&["daemons", "--json"], None);
    log.step("after: daemons --json", "", &after);
    assert_eq!(states(&after), pairs(&[("wHnDa_t10", "alive")]));
    let table = stdout(&sandbox.run(&["daemons"], None));
    assert!(!table.contains("--orphans"), "{table}");
    log.note("result: pass");
}

#[cfg(unix)]
#[test]
fn a_daemon_whose_probe_fails_spoils_only_its_own_row() {
    use support::{nvim_available, Artifact, Daemon};

    if !nvim_available() {
        eprintln!("skipping: nvim not found on PATH");
        return;
    }
    let sandbox = Sandbox::new("daemons-probe-error");
    let mut log = Artifact::new("a_daemon_whose_probe_fails_spoils_only_its_own_row");
    let mut fine = Daemon::spawn(&sandbox, "wHnDe:t1");
    let mut broken = Daemon::spawn(&sandbox, "wHnDe:t2");
    // A comment store whose list is not a list: the probe's comment count
    // then raises a Lua error, so the daemon answers the probe with one.
    broken
        .eval(r#"luaeval("(function() package.loaded['herdr-nvim.comments'] = { list = function() return 1 end } return 1 end)()")"#)
        .expect("break the comment store");
    sandbox.herdr_tabs(&[("wHnDe:t1", "novu", "api"), ("wHnDe:t2", "novu", "web")]);

    let listed = sandbox.run(&["daemons", "--json"], None);
    log.step("daemons --json", "", &listed);
    assert!(listed.status.success(), "{}", stderr(&listed));
    let value: serde_json::Value = serde_json::from_slice(&listed.stdout).expect("json");
    assert_eq!(value[0]["state"], "alive", "{value}");
    assert_eq!(value[1]["state"], "error", "{value}");
    assert!(
        value[1]["error"]
            .as_str()
            .is_some_and(|e| e.contains("wHnDe_t2")),
        "{value}"
    );

    let table = sandbox.run(&["daemons"], None);
    log.step("daemons", "", &table);
    assert!(table.status.success(), "{}", stderr(&table));
    assert!(stdout(&table).contains("error"), "{}", stdout(&table));

    for args in [
        &["daemons", "stop", "--all", "--force"][..],
        &["daemons", "stop", "--all"],
    ] {
        let stopped = sandbox.run(args, None);
        log.step(&args.join(" "), "", &stopped);
        assert_eq!(stopped.status.code(), Some(1));
        assert!(
            stderr(&stopped).contains("could not stop novu / web"),
            "{}",
            stderr(&stopped)
        );
        assert!(
            broken.running(),
            "a daemon whose probe failed is never stopped"
        );
        assert!(broken.entry.exists(), "nor forgotten");
    }
    fine.assert_gone(&sandbox);
    log.note("result: pass");
}

#[cfg(unix)]
#[test]
fn nothing_is_forgotten_while_the_process_table_cannot_be_read() {
    use support::Artifact;

    let sandbox = Sandbox::new("daemons-no-ps");
    let mut log = Artifact::new("nothing_is_forgotten_while_the_process_table_cannot_be_read");
    let stale = sandbox.stale_entry("wHnDp:t1");
    sandbox.herdr_tabs(&[("wHnDp:t1", "novu", "api")]);
    sandbox.break_ps();

    let stopped = sandbox.run(&["daemons", "stop", "wHnDp:t1"], None);
    log.step("daemons stop wHnDp:t1 (ps broken)", "", &stopped);
    assert_eq!(stopped.status.code(), Some(1));
    assert!(stale.exists(), "a stop must not forget it");

    let event = r#"{"event":"tab_closed","data":{"type":"tab_closed","tab_id":"wHnDp:t1","workspace_id":"wHnDp"}}"#;
    let hook = sandbox.run(&["on-event"], Some(event));
    log.step("on-event tab_closed wHnDp:t1 (ps broken)", event, &hook);
    assert!(!hook.status.success());
    assert!(stale.exists(), "the close hook must not forget it");

    sandbox.fix_ps();
    let stopped = sandbox.run(&["daemons", "stop", "wHnDp:t1"], None);
    log.step("daemons stop wHnDp:t1 (ps fixed)", "", &stopped);
    assert!(stopped.status.success(), "{}", stdout(&stopped));
    assert!(!stale.exists(), "once ps answers, the stale entry goes");
    log.note("result: pass");
}

#[cfg(unix)]
#[test]
fn a_hung_daemon_is_listed_refused_then_killed_with_force() {
    use std::time::{Duration, Instant};
    use support::{nvim_available, Artifact, Daemon};

    if !nvim_available() {
        eprintln!("skipping: nvim not found on PATH");
        return;
    }
    let sandbox = Sandbox::new("daemons-hung");
    let mut log = Artifact::new("a_hung_daemon_is_listed_refused_then_killed_with_force");
    let mut hung = Daemon::spawn(&sandbox, "wHnDh:t1");
    hung.hang();
    sandbox.herdr_tabs(&[("wHnDh:t1", "novu", "api")]);

    // Listed within the request timeout (5s), as unresponsive, by its pid.
    let started = Instant::now();
    let listed = sandbox.run(&["daemons", "--json"], None);
    log.step("daemons --json", "", &listed);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert!(listed.status.success());
    let value: serde_json::Value = serde_json::from_slice(&listed.stdout).expect("json");
    assert_eq!(value[0]["state"], "unresponsive", "{value}");
    assert_eq!(value[0]["pid"], hung.pid(), "{value}");

    let plain = sandbox.run(&["daemons", "stop", "wHnDh:t1"], None);
    log.step("daemons stop wHnDh:t1", "", &plain);
    assert_eq!(plain.status.code(), Some(1));
    assert!(
        stderr(&plain).contains("busy or hung"),
        "{}",
        stderr(&plain)
    );
    assert!(stderr(&plain).contains("--force"), "{}", stderr(&plain));
    assert!(hung.running(), "a plain stop must not kill it");
    assert!(hung.entry.exists(), "nor forget it");

    let forced = sandbox.run(&["daemons", "stop", "wHnDh:t1", "--force"], None);
    log.step("daemons stop wHnDh:t1 --force", "", &forced);
    assert!(forced.status.success(), "{}", stderr(&forced));
    assert!(
        stdout(&forced).contains("stopped novu / api"),
        "{}",
        stdout(&forced)
    );
    let killed = format!("killed it (pid {})", hung.pid());
    assert!(stderr(&forced).contains(&killed), "{}", stderr(&forced));
    hung.assert_gone(&sandbox);
    log.note("result: pass");
}
