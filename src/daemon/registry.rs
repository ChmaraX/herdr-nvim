//! Where running daemons are registered, and how they are stopped: the one
//! stop path (`stop_keys`) shared by `daemon-gc`, the herdr close hooks and
//! `herdr-nvim daemons stop`.

use std::{
    env,
    ffi::OsStr,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Stdio,
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};

use crate::{
    config::Sidebar,
    herdr::{CliHerdr, Herdr, TabInfo},
    state::{self, tab_key},
};

use super::{nvim_cmd, remote_expr};

/// How long `send_quit` waits for a daemon to go away after asking it to quit.
const QUIT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const QUIT_POLL_TIMEOUT: Duration = Duration::from_secs(3);

/// Directory that registers every running daemon, one entry per tab.
///
/// On Unix each entry is the daemon's own `<tab>.sock` listen socket. Windows
/// daemons listen on named pipes, which cannot be enumerated reliably, so
/// `ensure_daemon` drops a `<tab>.pipe` marker file here instead -- that is
/// what lets `gc` and `on-event` find a workspace's daemons on Windows.
///
/// `HERDR_NVIM_RUNTIME_DIR` overrides everything (used by tests); otherwise the
/// XDG runtime dir, falling back to the platform temp dir.
fn socket_dir() -> PathBuf {
    env::var_os("HERDR_NVIM_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_RUNTIME_DIR").map(|path| PathBuf::from(path).join("herdr-nvim"))
        })
        .unwrap_or_else(|| env::temp_dir().join("herdr-nvim"))
}

/// Extension of the per-daemon entries in `socket_dir` (see there).
#[cfg(not(windows))]
const REGISTRY_EXT: &str = "sock";
#[cfg(windows)]
const REGISTRY_EXT: &str = "pipe";

pub fn socket_path(tab: &str) -> PathBuf {
    socket_path_for_key(&tab_key(tab))
}

/// Socket (Unix) or named pipe (Windows) for an already-sanitized tab key.
pub(crate) fn socket_path_for_key(key: &str) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(format!(r"\\.\pipe\herdr-nvim-{key}"))
    }
    #[cfg(not(windows))]
    {
        registry_path(key)
    }
}

/// The `socket_dir` entry that registers the daemon for `key`: the socket
/// itself on Unix, a marker file on Windows.
pub(crate) fn registry_path(key: &str) -> PathBuf {
    socket_dir().join(format!("{key}.{REGISTRY_EXT}"))
}

/// Windows only: record the daemon in `socket_dir` (named pipes themselves
/// cannot be listed). The marker holds the raw tab id for diagnostics.
#[cfg(windows)]
pub(super) fn register_daemon(tab: &str) -> Result<()> {
    let marker = registry_path(&tab_key(tab));
    let dir = socket_dir();
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create runtime directory {}", dir.display()))?;
    fs::write(&marker, tab)
        .with_context(|| format!("failed to write daemon marker {}", marker.display()))
}

/// Sanitized tab keys of every registered daemon (see `socket_dir`), sorted.
/// Empty when the runtime directory does not exist yet.
pub(crate) fn daemon_keys() -> Result<Vec<String>> {
    let dir = socket_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read runtime directory {}", dir.display()))
        }
    };
    let mut keys = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("failed to read entry in {}", dir.display()))?
            .path();
        if path.extension().and_then(OsStr::to_str) != Some(REGISTRY_EXT) {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(OsStr::to_str) {
            keys.push(stem.to_owned());
        }
    }
    keys.sort();
    Ok(keys)
}

/// The keys among `keys` whose tab is not among herdr's live `tabs`.
pub(crate) fn orphans(keys: &[String], tabs: &[TabInfo]) -> Vec<String> {
    keys.iter()
        .filter(|key| !tabs.iter().any(|tab| tab.tab_id.key() == **key))
        .cloned()
        .collect()
}

/// Garbage-collect daemons whose tab no longer exists (herdr's `[[startup]]`
/// hook, after a restart that did not restore every tab).
pub fn gc_cmd() -> Result<()> {
    let mut herdr = CliHerdr;
    let config = crate::config::load();
    gc(&mut herdr, &config.sidebar)
}

/// Stop every daemon whose tab herdr no longer has. herdr being unreachable
/// is an error that stops nothing: every daemon would look orphaned.
/// `pub(crate)` so `maneuver::toggle` can run an opportunistic, best-effort gc
/// on every toggle to reap stale per-tab daemons from closed tabs.
pub(crate) fn gc(h: &mut dyn Herdr, sidebar: &Sidebar) -> Result<()> {
    let keys = daemon_keys()?;
    if keys.is_empty() {
        return Ok(());
    }
    let tabs = h.tab_infos()?;
    all_stopped(stop_keys(&orphans(&keys, &tabs), sidebar))
}

/// Vimscript: number of listed buffers with unsaved changes.
pub(crate) const UNSAVED_BUFFERS_EXPR: &str =
    "len(filter(getbufinfo({'bufmodified':1}),'v:val.listed'))";

/// Stop each daemon in `keys`, in order. Best effort: one failing does not
/// spare the rest. Returns every key with its own outcome, in input order.
pub(crate) fn stop_keys(keys: &[String], sidebar: &Sidebar) -> Vec<(String, Result<()>)> {
    keys.iter()
        .map(|key| (key.clone(), stop_tab_key(key, sidebar)))
        .collect()
}

/// `Ok` if every stop in `results` succeeded, else one error naming each
/// daemon that failed and why.
pub(crate) fn all_stopped(results: Vec<(String, Result<()>)>) -> Result<()> {
    let failures: Vec<String> = results
        .into_iter()
        .filter_map(|(key, result)| result.err().map(|err| format!("{key}: {err:#}")))
        .collect();
    if failures.is_empty() {
        return Ok(());
    }
    Err(anyhow!(
        "failed to stop {} nvim daemon(s):\n{}",
        failures.len(),
        failures.join("\n")
    ))
}

/// Stop the daemon for an already-sanitized tab key (a `socket_dir` entry
/// stem, see `state::tab_key`): force-quit it if it is still running, then
/// drop its registry entry and sidebar state file. Unsaved buffers are
/// discarded -- the tab is gone, or `daemons stop --force` asked for it --
/// but reported on stderr (herdr's plugin command log). A daemon that is
/// already gone is a no-op beyond the file cleanup. A daemon that does not
/// quit keeps its registry entry and state, so it stays listable and
/// reachable, and is reported as an error.
fn stop_tab_key(key: &str, sidebar: &Sidebar) -> Result<()> {
    let socket = socket_path_for_key(key);
    // Doubles as the liveness probe: `None` means nothing is listening.
    if let Some(unsaved) = remote_expr(&socket, UNSAVED_BUFFERS_EXPR, sidebar) {
        if unsaved != "0" {
            eprintln!(
                "herdr-nvim: stopping nvim daemon of tab {key}; discarding {unsaved} unsaved buffer(s)"
            );
        }
        send_quit(&socket, sidebar)?;
    }
    // `key` is a filename component, already sanitized, so it goes through
    // `state::remove_key` rather than `state::remove` -- that avoids
    // sanitizing an already-sanitized key a second time.
    state::remove_file_if_exists(&registry_path(key))?;
    state::remove_key(key)
}

/// Ask the daemon to force-quit (`qa!`), then wait until it stops answering.
/// Errors if it is still answering when `QUIT_POLL_TIMEOUT` runs out.
fn send_quit(socket: &Path, sidebar: &Sidebar) -> Result<()> {
    nvim_cmd(sidebar)
        .arg("--headless")
        .arg("--server")
        .arg(socket)
        .arg("--remote-send")
        .arg("<cmd>qa!<cr>")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("failed to run nvim --remote-send for {}", socket.display()))?;
    let deadline = Instant::now() + QUIT_POLL_TIMEOUT;
    while remote_expr(socket, "1", sidebar).is_some() {
        if Instant::now() >= deadline {
            bail!(
                "nvim daemon at {} still running {}s after qa!",
                socket.display(),
                QUIT_POLL_TIMEOUT.as_secs()
            );
        }
        sleep(QUIT_POLL_INTERVAL);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::{
        config::Config,
        herdr::MockHerdr,
        state::TabId,
        test_support::{nvim_available, TestDaemon, TestEnv},
    };

    fn live(tab_id: &str) -> TabInfo {
        TabInfo {
            tab_id: TabId::new(tab_id),
            workspace_label: None,
            tab_label: None,
            tab_number: None,
        }
    }

    #[test]
    fn socket_path_respects_runtime_dir_override() {
        let env = TestEnv::new();
        #[cfg(not(windows))]
        assert_eq!(socket_path("wsX"), env.runtime_dir().join("wsX.sock"));
        #[cfg(windows)]
        {
            let _ = env;
            assert_eq!(
                socket_path("wsX"),
                PathBuf::from(r"\\.\pipe\herdr-nvim-wsX")
            );
        }
    }

    #[test]
    fn socket_path_sanitizes_colon_in_tab_id() {
        let env = TestEnv::new();
        #[cfg(not(windows))]
        assert_eq!(socket_path("wX:t1"), env.runtime_dir().join("wX_t1.sock"));
        #[cfg(windows)]
        {
            let _ = env;
            assert_eq!(
                socket_path("wX:t1"),
                PathBuf::from(r"\\.\pipe\herdr-nvim-wX_t1")
            );
        }
    }

    #[test]
    fn gc_removes_orphan_entries_and_keeps_known_tabs() {
        let env = TestEnv::new();
        // Two dead registry entries (sockets on Unix, pipe markers on
        // Windows); only "wsKeep:t1" is still a live tab.
        fs::create_dir_all(env.runtime_dir()).unwrap();
        let keep = registry_path(&tab_key("wsKeep:t1"));
        let orphan = registry_path(&tab_key("wsOrphan:t1"));
        fs::write(&keep, b"").unwrap();
        fs::write(&orphan, b"").unwrap();

        let mut herdr = MockHerdr {
            tab_infos_results: VecDeque::from([Ok(vec![live("wsKeep:t1")])]),
            ..Default::default()
        };
        gc(&mut herdr, &Sidebar::default()).unwrap();

        assert!(keep.exists(), "known tab kept");
        assert!(!orphan.exists(), "orphan tab entry removed");
    }

    #[test]
    fn gc_stops_nothing_when_herdr_is_unreachable() {
        let env = TestEnv::new();
        fs::create_dir_all(env.runtime_dir()).unwrap();
        let entry = registry_path(&tab_key("wsAny:t1"));
        fs::write(&entry, b"").unwrap();
        let mut herdr = MockHerdr {
            tab_infos_results: VecDeque::from([Err(anyhow!("no herdr"))]),
            ..Default::default()
        };
        assert!(gc(&mut herdr, &Sidebar::default()).is_err());
        assert!(entry.exists());
    }

    #[test]
    fn orphans_match_keys_against_live_tab_ids() {
        let keys = ["wA_t1", "wA_t2", "wAB_t1"].map(str::to_owned);
        assert_eq!(orphans(&keys, &[live("wA:t1"), live("wAB:t1")]), ["wA_t2"]);
        assert_eq!(orphans(&keys, &[]), keys);
    }

    // Issue #34's leak: a daemon that ignores `qa!` must not be forgotten.
    // `vim.on_key` returning "" swallows every key, including the `qa!` that
    // `--remote-send` types, while RPC (and so `--remote-expr`) still works.
    #[test]
    fn a_daemon_that_ignores_quit_keeps_its_registry_entry() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let _env = TestEnv::new();
        let config = Config::default();
        let stubborn = TestDaemon::spawn("wHnQ:t1", &config);
        let polite = TestDaemon::spawn("wHnQ:t2", &config);
        stubborn
            .eval(r#"luaeval("(function() vim.on_key(function() return '' end) return 1 end)()")"#);

        let results = stop_keys(&[stubborn.tab.key(), polite.tab.key()], &config.sidebar);
        assert!(results[0].1.is_err(), "timeout must be an error");
        assert!(results[1].1.is_ok(), "the other daemon is still stopped");
        let error = all_stopped(results).unwrap_err().to_string();
        assert!(error.contains("1 nvim daemon"), "{error}");

        stubborn.assert_alive();
        assert_eq!(daemon_keys().unwrap(), [stubborn.tab.key()]);
        polite.assert_gone();
    }
}
