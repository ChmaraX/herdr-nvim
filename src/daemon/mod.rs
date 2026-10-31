//! The per-tab headless nvim daemon: spawning it and attaching the sidebar's
//! UI to it. Its registry and shutdown live in `registry`, the herdr close
//! hooks in `events`, and the `herdr-nvim daemons` command in `inventory`.

pub(crate) mod events;
pub(crate) mod inventory;
pub(crate) mod process;
pub(crate) mod registry;

use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::{self, sleep},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};

use crate::{
    config::{Config, Sidebar},
    state::TabId,
};

use registry::socket_path;

const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How long `ensure_daemon` may take in all: probing an existing daemon
/// (one that does not answer in time is declared unresponsive) and, if there
/// is none, waiting for a fresh one to answer. Generous, because a heavy
/// config (LazyVim) can keep a healthy daemon's main loop busy for seconds,
/// right after spawn above all.
pub(crate) const HEALTH_POLL_TIMEOUT: Duration = Duration::from_secs(10);

/// How long one request to a daemon that is already up may take. A healthy
/// daemon answers in milliseconds; this only runs out when its main loop is
/// blocked or the process is stopped.
pub(crate) const RPC_TIMEOUT: Duration = Duration::from_secs(5);

/// How often a request's `nvim --server` client is checked for completion.
const CLIENT_POLL_INTERVAL: Duration = Duration::from_millis(10);

const READY_POLL_INTERVAL: Duration = Duration::from_millis(50);
// Short: only waits out the tail of an already-in-flight `maneuver::open`,
// never a fresh operation, so a stale size is preferable to a hung-looking pane.
const READY_POLL_TIMEOUT: Duration = Duration::from_secs(2);

/// Build a `Command` invoking the configured nvim binary with the configured
/// environment overrides. Shared by every call site that talks to a daemon
/// (health checks, `--remote-ui` attach, `--remote-send` quit, `--remote`
/// open) so both `sidebar.nvim_bin` and `sidebar.nvim_env` apply uniformly,
/// not just to spawning the daemon itself.
pub(crate) fn nvim_cmd(sidebar: &Sidebar) -> Command {
    let mut command = Command::new(&sidebar.nvim_bin);
    for (key, value) in sidebar.env_override() {
        command.env(key, value);
    }
    command
}

/// Ensure a per-tab headless nvim daemon is listening on its socket,
/// returning the socket path. If a healthy daemon already exists this is a
/// no-op; otherwise a detached daemon is spawned and polled until healthy.
///
/// All of it, the first probe included, takes at most `HEALTH_POLL_TIMEOUT`.
/// A daemon that holds its socket but does not answer in that time is an
/// error, never a reason to spawn a second daemon over it: that would
/// orphan the stuck one, still holding its memory.
pub fn ensure_daemon(
    tab: &str,
    plugin_root: &Path,
    config: &Config,
    cwd: &Path,
) -> Result<PathBuf> {
    let deadline = Instant::now() + HEALTH_POLL_TIMEOUT;
    let socket = socket_path(tab);
    let reply = remote_expr(&socket, "1+1", &config.sidebar, HEALTH_POLL_TIMEOUT)?;
    match liveness(&socket, reply, None) {
        Liveness::Answered(_) => {
            // Also (re)register an already-running daemon, so one spawned
            // before the Windows marker existed still becomes discoverable.
            #[cfg(windows)]
            registry::register_daemon(tab)?;
            return Ok(socket);
        }
        Liveness::Unresponsive => bail!(
            "the nvim daemon for tab {tab} is busy or hung (no reply within {}s); \
             if it stays that way, `herdr-nvim daemons stop {tab} --force` kills it \
             and loses its unsaved work",
            HEALTH_POLL_TIMEOUT.as_secs()
        ),
        Liveness::Gone => {}
    }

    #[cfg(not(windows))]
    {
        let dir = socket
            .parent()
            .with_context(|| format!("socket path has no parent: {}", socket.display()))?;
        fs::create_dir_all(dir)
            .with_context(|| format!("failed to create runtime directory {}", dir.display()))?;
        // A stale (dead) socket file would make `nvim --listen` fail to bind.
        // We only get here when nothing accepted the health check and no
        // process listens on the socket, so any file present is dead.
        crate::state::remove_file_if_exists(&socket)?;
    }

    spawn_daemon(tab, &socket, plugin_root, &config.sidebar, cwd)?;
    #[cfg(windows)]
    registry::register_daemon(tab)?;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!(
                "nvim daemon for tab {tab} did not become healthy within {}s",
                HEALTH_POLL_TIMEOUT.as_secs()
            );
        }
        if let Reply::Answered(_) = remote_expr(&socket, "1+1", &config.sidebar, remaining)? {
            return Ok(socket);
        }
        sleep(HEALTH_POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
    }
}

fn spawn_daemon(
    tab: &str,
    socket: &Path,
    plugin_root: &Path,
    sidebar: &Sidebar,
    cwd: &Path,
) -> Result<()> {
    // The pre-init `set rtp+=` below is not enough on its own: configs that
    // rebuild runtimepath from scratch during startup (any plugin manager with
    // an rtp reset, e.g. lazy.nvim's default `performance.rtp.reset = true`)
    // drop our appended path before VimEnter fires. So the VimEnter callback
    // re-appends the plugin root to rtp
    // *after* the user's config has loaded, then requires the plugin. The whole
    // thing stays wrapped in `pcall` so a missing/broken plugin never crashes
    // the daemon.
    let vim_enter = format!(
        "lua vim.api.nvim_create_autocmd('VimEnter',{{callback=function() \
         pcall(function() vim.opt.rtp:append({root:?}); require('herdr-nvim').setup() end) end}})",
        root = plugin_root.display().to_string()
    );
    // Plumb the tab's identity into the daemon explicitly. The daemon is
    // per-tab and persistent, and it may be spawned through a plugin pane (the
    // `pick-file` picker) whose only context is `HERDR_PLUGIN_CONTEXT_JSON` --
    // herdr does not export the flat `HERDR_WORKSPACE_ID`/`HERDR_TAB_ID` into a
    // plugin pane. Relying on inherited env therefore leaves the daemon (and
    // thus `agents.lua`) with no workspace/tab scope on that path, so the agent
    // picker opens even for an unambiguous target (issue #20). Setting them here
    // from the tab id we already have makes scoping deterministic regardless of
    // which pane first spawned the daemon.
    let workspace = TabId::new(tab).workspace().to_owned();

    let mut command = nvim_cmd(sidebar);
    command
        .arg("--headless")
        .arg("--listen")
        .arg(socket)
        .arg("--cmd")
        .arg(format!("set rtp+={}", plugin_root.display()))
        .arg("--cmd")
        .arg(&vim_enter)
        .env("HERDR_WORKSPACE_ID", &workspace)
        .env("HERDR_TAB_ID", tab)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach_command(&mut command);
    let child = command
        .spawn()
        .context("failed to spawn nvim daemon (is nvim installed?)")?;

    // Read the pid (useful for diagnostics) and drop the handle without waiting:
    // the daemon is now a session leader in its own session and keeps running,
    // tracked from here on only by its socket.
    let _pid = child.id();
    drop(child);
    Ok(())
}

/// Detach a child that must survive the pane process which launched it.
pub(crate) fn detach_command(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        // SAFETY: the pre_exec closure only calls setsid(2), which is
        // async-signal-safe and does not touch shared parent state.
        unsafe {
            command.pre_exec(|| {
                extern "C" {
                    fn setsid() -> i32;
                }
                setsid();
                Ok(())
            });
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        // CREATE_NO_WINDOW keeps the headless daemon out of the user's console,
        // while CREATE_NEW_PROCESS_GROUP keeps it independent from the pane's
        // console process group. Unlike DETACHED_PROCESS, it still permits the
        // redirected standard handles used above.
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
}

/// What one request's `nvim --server` client saw (see `request`). Only the
/// client's view: a hung daemon refuses connections too once dozens of
/// requests have timed out on it (its listen queue is full), so a refusal
/// is no proof that the daemon is gone. Nothing destructive acts on a
/// `Reply`; `liveness` turns it into the `Liveness` that those take.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Reply<T = String> {
    /// It answered; for `--remote-expr`, with the trimmed result.
    Answered(T),
    /// The client could not connect (nvim's E247).
    Refused,
    /// Something accepted the connection but sent no answer in time.
    TimedOut,
}

impl<T> Reply<T> {
    pub(crate) fn map<U>(self, f: impl FnOnce(T) -> U) -> Reply<U> {
        match self {
            Self::Answered(answer) => Reply::Answered(f(answer)),
            Self::Refused => Reply::Refused,
            Self::TimedOut => Reply::TimedOut,
        }
    }

    /// The answer, or an error saying why there is none.
    pub(crate) fn answer(self, socket: &Path) -> Result<T> {
        match self {
            Self::Answered(answer) => Ok(answer),
            Self::Refused => Err(anyhow!(
                "no nvim daemon accepts connections on {}",
                socket.display()
            )),
            Self::TimedOut => Err(anyhow!(
                "the nvim daemon on {} is busy or hung (no reply in time)",
                socket.display()
            )),
        }
    }
}

/// Whether the daemon on a socket is there, as `liveness` decides it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Liveness<T = String> {
    /// It answered.
    Answered(T),
    /// Its process holds the socket but does not answer in time: a daemon
    /// whose main loop is blocked, or a stopped process.
    Unresponsive,
    /// No process listens on the socket.
    Gone,
}

impl<T> Liveness<T> {
    pub(crate) fn answered(&self) -> Option<&T> {
        match self {
            Self::Answered(answer) => Some(answer),
            Self::Unresponsive | Self::Gone => None,
        }
    }
}

/// The one way to learn whether the daemon on `socket` is there, from a
/// client's `reply`: a refusal is checked against the process table, where
/// an nvim still listening on `socket` makes it `Unresponsive`. `table` is
/// one already loaded, if any; else one is loaded when the verdict needs it.
/// Without a socket file or a readable process table the client's verdict
/// stands.
pub(crate) fn liveness<T>(
    socket: &Path,
    reply: Reply<T>,
    table: Option<&process::ProcessTable>,
) -> Liveness<T> {
    match reply {
        Reply::Answered(answer) => Liveness::Answered(answer),
        Reply::TimedOut => Liveness::Unresponsive,
        Reply::Refused if !socket.exists() => Liveness::Gone,
        Reply::Refused => {
            let loaded;
            let table = match table {
                Some(table) => Some(table),
                None => {
                    loaded = process::ProcessTable::load();
                    loaded.as_ref()
                }
            };
            if table.is_some_and(|table| !table.listeners(socket).is_empty()) {
                Liveness::Unresponsive
            } else {
                Liveness::Gone
            }
        }
    }
}

/// Send one request to the daemon on `socket` as an `nvim --server <socket>
/// <args>` client, giving it `timeout` to answer. A client still waiting
/// then is killed and reaped, never left behind. Every request to a daemon
/// goes through here; only the long-lived `--remote-ui` attach does not.
///
/// `Err` when there is no verdict on the daemon: the client could not be
/// run, or the daemon answered with an error.
pub(crate) fn request(
    socket: &Path,
    args: &[&str],
    sidebar: &Sidebar,
    timeout: Duration,
) -> Result<Reply> {
    let mut child = nvim_cmd(sidebar)
        .arg("--headless")
        .arg("--server")
        .arg(socket)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to run {}", sidebar.nvim_bin))?;
    // Drain the pipes on the side, so a large answer cannot fill one and
    // stall the client. Killing the client closes them and ends these.
    fn drain(mut pipe: impl Read + Send + 'static) -> thread::JoinHandle<String> {
        thread::spawn(move || {
            let mut out = Vec::new();
            let _ = pipe.read_to_end(&mut out);
            String::from_utf8_lossy(&out).trim().to_owned()
        })
    }
    let stdout = drain(child.stdout.take().expect("stdout is piped"));
    let stderr = drain(child.stderr.take().expect("stderr is piped"));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(Some(status)),
            Ok(None) if Instant::now() < deadline => sleep(CLIENT_POLL_INTERVAL),
            waited => {
                let _ = child.kill();
                let _ = child.wait();
                break waited.map(|_| None);
            }
        }
    };
    let (stdout, stderr) = (
        stdout.join().unwrap_or_default(),
        stderr.join().unwrap_or_default(),
    );
    match status.context("failed to wait for an nvim --server client")? {
        Some(status) if status.success() => Ok(Reply::Answered(stdout)),
        // E247: "Failed to connect to '<socket>'".
        Some(_) if stderr.contains("E247:") => Ok(Reply::Refused),
        Some(status) => bail!(
            "nvim --server {} {} failed ({status}): {stderr}",
            socket.display(),
            args.first().copied().unwrap_or_default()
        ),
        None => Ok(Reply::TimedOut),
    }
}

/// Evaluate a vimscript expression on the daemon via `--remote-expr`.
pub(crate) fn remote_expr(
    socket: &Path,
    expr: &str,
    sidebar: &Sidebar,
    timeout: Duration,
) -> Result<Reply> {
    request(socket, &["--remote-expr", expr], sidebar, timeout)
}

/// Quote `s` as a Vimscript single-quoted string, where a literal quote is
/// represented by two consecutive quotes.
fn vim_string_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Update the persistent daemon with the identity of the sidebar pane whose UI
/// is about to attach. A daemon is reused across sidebar toggles, while herdr
/// creates a fresh pane each time, so the process environment inherited when
/// the daemon first spawned cannot be treated as current.
fn refresh_daemon_pane_id(socket: &Path, pane: &str, sidebar: &Sidebar) -> Result<()> {
    let expr = format!("setenv('HERDR_PANE_ID', {})", vim_string_literal(pane));
    remote_expr(socket, &expr, sidebar, RPC_TIMEOUT)
        .and_then(|reply| reply.answer(socket))
        .with_context(|| {
            format!("failed to update nvim daemon with current sidebar pane id {pane}")
        })?;
    Ok(())
}

/// Quote `s` with single quotes, escaping any single quotes it already
/// contains (`'` -> `'\''`) -- used where a value is spliced into a command
/// executed by a shell (e.g. the nvim bin in doctor's `pane run` probe), not
/// just displayed, so it is always quoted unconditionally.
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Runs inside the sidebar pane: ensure the tab's daemon is up, then replace
/// this process with `nvim --remote-ui` attached to it.
///
/// The pane is spawned by herdr via `plugin pane open --entrypoint sidebar`
/// (non-interactive, no shell echo). herdr sets `HERDR_TAB_ID` and
/// `HERDR_PANE_ID` for plugin panes so this command can select the tab daemon
/// and refresh it with the live sidebar pane identity. The pane's own cwd (set
/// via `--cwd`) is where the daemon spawns.
pub fn sidebar_cmd() -> Result<()> {
    let tab = env::var("HERDR_TAB_ID")
        .context("herdr-nvim sidebar requires HERDR_TAB_ID (set by herdr for plugin panes)")?;
    let pane = env::var("HERDR_PANE_ID")
        .context("herdr-nvim sidebar requires HERDR_PANE_ID (set by herdr for plugin panes)")?;
    let cwd = env::current_dir().context("herdr-nvim sidebar could not resolve its cwd")?;
    let plugin_root = plugin_root()?;
    let config = crate::config::load();
    let socket = ensure_daemon(&tab, &plugin_root, &config, &cwd)?;
    wait_for_layout_ready();
    refresh_daemon_pane_id(&socket, &pane, &config.sidebar)?;

    attach_remote_ui(&socket, &config.sidebar)
}

#[cfg(unix)]
fn attach_remote_ui(socket: &Path, sidebar: &Sidebar) -> Result<()> {
    use std::os::unix::process::CommandExt;

    let error = nvim_cmd(sidebar)
        .arg("--server")
        .arg(socket)
        .arg("--remote-ui")
        .exec();
    Err(error).context("failed to exec nvim --remote-ui")
}

#[cfg(windows)]
fn attach_remote_ui(socket: &Path, sidebar: &Sidebar) -> Result<()> {
    let status = nvim_cmd(sidebar)
        .arg("--server")
        .arg(socket)
        .arg("--remote-ui")
        .status()
        .context("failed to run nvim --remote-ui")?;
    if !status.success() {
        bail!("nvim --remote-ui failed (exit {status})");
    }
    Ok(())
}

/// Blocks until `maneuver::open` signals layout is settled via
/// `HERDR_NVIM_READY_MARKER`, or `READY_POLL_TIMEOUT` elapses; a no-op if
/// the env var is unset, so this can never turn into an unconditional stall.
fn wait_for_layout_ready() {
    let Some(marker) = env::var_os("HERDR_NVIM_READY_MARKER").map(PathBuf::from) else {
        return;
    };
    let deadline = Instant::now() + READY_POLL_TIMEOUT;
    while !marker.exists() {
        if Instant::now() >= deadline {
            return;
        }
        sleep(READY_POLL_INTERVAL);
    }
    // Removal is best-effort: a leftover marker is nonced, unreachable by
    // later opens, and collected by `state::sweep_stale_markers`.
    let _ = fs::remove_file(&marker);
}

/// Locate the plugin root (the directory containing `lua/herdr-nvim`) so the
/// daemon can `require('herdr-nvim')`. `HERDR_NVIM_PLUGIN_ROOT` overrides;
/// otherwise walk up from this executable.
pub(crate) fn plugin_root() -> Result<PathBuf> {
    if let Some(root) = env::var_os("HERDR_NVIM_PLUGIN_ROOT") {
        return Ok(PathBuf::from(root));
    }
    let exe = env::current_exe().context("failed to resolve current executable path")?;
    let mut dir = exe.parent();
    while let Some(candidate) = dir {
        if candidate.join("lua").join("herdr-nvim").is_dir() {
            return Ok(candidate.to_path_buf());
        }
        dir = candidate.parent();
    }
    bail!("could not locate plugin root (set HERDR_NVIM_PLUGIN_ROOT)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{nvim_available, TestDaemon, TestEnv};

    // Regression test for issue #20: a daemon spawned through the pick-file
    // picker inherits only HERDR_PLUGIN_CONTEXT_JSON (plugin panes get no flat
    // vars). The daemon must still end up with HERDR_WORKSPACE_ID/HERDR_TAB_ID
    // set from its tab id, so agents.lua can scope M.list()/resolve() and skip
    // the picker for an unambiguous target.
    #[test]
    fn daemon_gets_flat_identity_even_without_inherited_flat_vars() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let _env = TestEnv::new();
        let config = Config::default();

        // Mimic the picker/finisher parent env: JSON blob present, flat vars
        // absent -- exactly what the plugin pane hands down.
        env::remove_var("HERDR_WORKSPACE_ID");
        env::remove_var("HERDR_TAB_ID");
        env::set_var(
            "HERDR_PLUGIN_CONTEXT_JSON",
            r#"{"workspace_id":"w39","tab_id":"w39:t1"}"#,
        );
        let daemon = TestDaemon::spawn("w39:t1", &config);
        env::remove_var("HERDR_PLUGIN_CONTEXT_JSON");

        // What agents.lua reads inside the daemon must now be populated.
        assert_eq!(
            daemon.eval("$HERDR_WORKSPACE_ID"),
            "w39",
            "daemon must have HERDR_WORKSPACE_ID"
        );
        assert_eq!(
            daemon.eval("$HERDR_TAB_ID"),
            "w39:t1",
            "daemon must have HERDR_TAB_ID"
        );
    }

    #[test]
    fn ensure_daemon_spawns_then_is_idempotent_against_real_nvim() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let _env = TestEnv::new();
        let plugin_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let config = Config::default();

        let daemon = TestDaemon::spawn("wD", &config);
        #[cfg(not(windows))]
        assert!(
            daemon.socket.exists(),
            "socket file should exist after spawn"
        );

        let socket_again =
            ensure_daemon("wD", &plugin_root, &config, &plugin_root).expect("second ensure_daemon");
        assert_eq!(daemon.socket, socket_again);
        assert_eq!(
            daemon.eval("getpid()"),
            daemon.pid.to_string(),
            "second ensure_daemon must reuse the daemon, not spawn a new one"
        );
    }

    // Regression test for issue #61: a daemon that holds its socket but never
    // answers RPC (here: SIGSTOPped, like the reporter's wedged daemon) must
    // not block `ensure_daemon` -- and so pick-file/open-link -- forever. It
    // must give up within the health deadline with an error, and must not
    // replace the wedged daemon with a twin on the same socket.
    #[cfg(unix)]
    #[test]
    fn ensure_daemon_gives_up_on_an_unresponsive_daemon() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let _env = TestEnv::new();
        let plugin_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let config = Config::default();
        let daemon = TestDaemon::spawn("w61:t1", &config);
        daemon.hang();

        let (tx, rx) = std::sync::mpsc::channel();
        let root = plugin_root.clone();
        let started = Instant::now();
        let worker = std::thread::spawn(move || {
            let result = ensure_daemon("w61:t1", &root, &Config::default(), &root);
            let _ = tx.send(result.map_err(|e| format!("{e:#}")));
        });
        // Deadline plus slack for nvim startup on a slow CI runner.
        let outcome = rx.recv_timeout(HEALTH_POLL_TIMEOUT + Duration::from_secs(5));
        let elapsed = started.elapsed();
        // Unwedge before asserting, so a failure cannot leave the worker
        // thread or the daemon stuck.
        daemon.resume();
        let _ = worker.join();

        let outcome = outcome.unwrap_or_else(|_| {
            panic!("ensure_daemon still blocked on an unresponsive daemon after {elapsed:?}")
        });
        assert!(
            outcome.is_err(),
            "ensure_daemon must report an unresponsive daemon, got {outcome:?}"
        );
        assert_eq!(
            daemon.eval("getpid()"),
            daemon.pid.to_string(),
            "the wedged daemon must not be replaced by a twin on its socket"
        );
    }

    #[test]
    fn vim_string_literal_escapes_quotes() {
        assert_eq!(vim_string_literal("w1:p'3"), "'w1:p''3'");
    }

    #[test]
    fn nvim_cmd_applies_env_overrides_to_child() {
        let sidebar = Sidebar::default();
        let plain = nvim_cmd(&sidebar);
        assert!(
            plain.get_envs().all(|(key, _)| key != "NVIM_APPNAME"),
            "default sidebar must not override the environment"
        );

        let sidebar = Sidebar {
            nvim_env: vec![
                "NVIM_APPNAME=myapp".to_owned(),
                "HERDR_NVIM_EXTRA=1=2".to_owned(),
                "garbage-entry".to_owned(), // malformed: filtered out by env_override
            ],
            ..Default::default()
        };
        let cmd = nvim_cmd(&sidebar);
        let mut appname = None;
        let mut extra = None;
        for (key, value) in cmd.get_envs() {
            match key.to_str() {
                Some("NVIM_APPNAME") => appname = value.map(|v| v.to_string_lossy().into_owned()),
                Some("HERDR_NVIM_EXTRA") => extra = value.map(|v| v.to_string_lossy().into_owned()),
                _ => {}
            }
        }
        assert_eq!(appname.as_deref(), Some("myapp"));
        assert_eq!(extra.as_deref(), Some("1=2"), "value may contain an = sign");
    }
}
