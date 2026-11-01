//! Stopping daemons whose tab is gone. The app has no plugin events or startup
//! hooks, so one watcher, `herdr-nvim app-watch`, follows herdr-core's event
//! stream (`hc events`): each change of the app's layout document (a tab or
//! space closed, from this window or another) reconciles the daemons against
//! the tabs in it. It runs as a named core terminal (`herdr-nvim-watch`, no
//! labels, so no window shows it), started by every sidebar, and ends when no
//! daemon is left. Starting it reconciles first: that is the startup GC.

use std::{
    io::{BufRead, BufReader},
    process::Stdio,
};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::{config, daemon::registry, host::Host};

use super::{cli, doc, rpc};

pub(crate) const NAME: &str = "herdr-nvim-watch";

/// Stop every daemon whose tab is not in the app's layout document. An
/// unreadable or missing document stops nothing.
pub(crate) fn reconcile() -> Result<usize> {
    let keys = registry::daemon_keys()?;
    if keys.is_empty() {
        return Ok(0);
    }
    let Some(doc) = doc::read()? else {
        return Ok(keys.len());
    };
    let orphans = registry::orphans(&keys, &doc::tab_infos(&doc));
    let config = config::load();
    registry::all_stopped(registry::stop_keys(&orphans, &config.sidebar))?;
    Ok(keys.len() - orphans.len())
}

/// Pure: whether an `hc events --json` line can mean a tab went away: the
/// app's document changed, or the stream skipped events.
pub(crate) fn wakes(line: &str) -> bool {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    let kind = message.pointer("/params/type").and_then(Value::as_str);
    match kind {
        Some("doc") => {
            message.pointer("/params/data/name").and_then(Value::as_str) == Some("herdr-app")
        }
        Some("missed") => true,
        _ => false,
    }
}

/// `herdr-nvim app-watch`: reconcile now, then on every document change, until
/// no daemon is left or the core goes away.
pub fn app_watch_cmd() -> Result<()> {
    if reconcile()? == 0 {
        return Ok(());
    }
    let mut events = rpc::hc()
        .args(["events", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("cannot run hc events")?;
    let stdout = events.stdout.take().context("hc events: no stdout")?;
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if !wakes(&line) {
            continue;
        }
        match reconcile() {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => eprintln!("herdr-nvim: reconcile failed: {error:#}"),
        }
    }
    let _ = events.kill();
    let _ = events.wait();
    Ok(())
}

/// Start the watcher unless it runs. Its name is unique among live
/// terminals, so a second start is refused by herdr-core: no lock needed.
pub(crate) fn ensure_running() {
    debug_assert_eq!(crate::host::current(), Host::App);
    let Ok(me) = cli::self_command() else { return };
    let _ = rpc::call(
        "terminal.run",
        json!({
            "argv": [me.to_string_lossy(), "app-watch"],
            "name": NAME,
            "no_parent": true,
            "rm": true,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wakes_on_the_apps_document_and_on_missed_events_only() {
        // Lines as herdr-core 0.1.16 prints them.
        assert!(wakes(
            r#"{"jsonrpc":"2.0","method":"event","params":{"data":{"name":"herdr-app","version":5},"seq":32,"type":"doc"}}"#
        ));
        assert!(wakes(
            r#"{"jsonrpc":"2.0","method":"event","params":{"seq":40,"type":"missed"}}"#
        ));
        assert!(!wakes(
            r#"{"jsonrpc":"2.0","method":"event","params":{"data":{"name":"other","version":1},"type":"doc"}}"#
        ));
        assert!(!wakes(
            r#"{"jsonrpc":"2.0","method":"event","params":{"data":{"title":"x"},"terminal":"t4","type":"title"}}"#
        ));
        assert!(!wakes("not json"));
    }
}
