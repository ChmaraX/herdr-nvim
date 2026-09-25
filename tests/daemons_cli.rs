//! `herdr-nvim daemons` as a user runs it, against an isolated, empty runtime
//! dir so no real daemon is ever listed or stopped.

use std::{env, path::PathBuf, process::Command};

fn run_daemons(args: &[&str]) -> std::process::Output {
    let sandbox: PathBuf = env::temp_dir().join(format!("hn-daemons-cli-{}", std::process::id()));
    Command::new(env!("CARGO_BIN_EXE_herdr-nvim"))
        .arg("daemons")
        .args(args)
        .env("HERDR_NVIM_RUNTIME_DIR", sandbox.join("runtime"))
        .env("HERDR_NVIM_STATE_DIR", sandbox.join("state"))
        .output()
        .expect("run herdr-nvim daemons")
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn empty_listing_in_text_and_json() {
    let text = run_daemons(&[]);
    assert!(text.status.success());
    assert_eq!(stdout(&text), "no nvim daemons running\n");

    let json = run_daemons(&["--json"]);
    assert!(json.status.success());
    let value: serde_json::Value = serde_json::from_str(&stdout(&json)).expect("valid JSON");
    assert_eq!(value, serde_json::json!([]));
}

#[test]
fn stopping_an_unknown_tab_or_bad_arguments_fail() {
    let unknown = run_daemons(&["stop", "wNone:t1"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("no nvim daemon for tab wNone:t1"));

    let bad = run_daemons(&["stop"]);
    assert_eq!(bad.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("usage: herdr-nvim daemons"));

    let nothing = run_daemons(&["stop", "--all"]);
    assert!(nothing.status.success());
    assert_eq!(stdout(&nothing), "no nvim daemons running\n");
}
