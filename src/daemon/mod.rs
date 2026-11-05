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
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread::{self, sleep},
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};

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
/// orphan the stuck one, still holding its memory. One that refuses
/// connections while its process listens is asked again until then: it may
/// be one that another caller has just spawned, not accepting yet.
pub fn ensure_daemon(
    tab: &str,
    plugin_root: &Path,
    config: &Config,
    cwd: &Path,
) -> Result<PathBuf> {
    let deadline = Instant::now() + HEALTH_POLL_TIMEOUT;
    let socket = socket_path(tab);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let reply = remote_expr(&socket, "1+1", &config.sidebar, remaining)?;
        match liveness(&socket, reply, None)? {
            Liveness::Answered(_) => {
                // Also (re)register an already-running daemon, so one spawned
                // before the Windows marker existed still becomes discoverable.
                #[cfg(windows)]
                registry::register_daemon(tab)?;
                return Ok(socket);
            }
            Liveness::Unresponsive if Instant::now() + HEALTH_POLL_INTERVAL < deadline => {
                sleep(HEALTH_POLL_INTERVAL)
            }
            Liveness::Unresponsive => bail!(
                "the nvim daemon for tab {tab} is busy or hung (no reply within {}s); \
                 if it stays that way, `herdr-nvim daemons stop {tab} --force` kills it \
                 and loses its unsaved work",
                HEALTH_POLL_TIMEOUT.as_secs()
            ),
            Liveness::Gone => break,
        }
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
        match remote_expr(&socket, "1+1", &config.sidebar, remaining)? {
            Reply::Answered(_) => return Ok(socket),
            Reply::Refused | Reply::TimedOut => {}
            Reply::Failed(error) => bail!("{error}"),
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
    /// The client ran and failed, saying why: the daemon answered with an
    /// error, or dropped the connection (one exiting mid-request does).
    Failed(String),
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
/// client's `reply`. On Unix a refusal is checked against the process
/// table: an nvim still listening on `socket` (even one whose socket file
/// was deleted) makes it `Unresponsive`, and a table that cannot be read is
/// an error, never `Gone`. A `Failed` client says nothing about liveness
/// either, so it is an error too. `table` is one already loaded, if any; else one
/// is loaded when the verdict needs it. Windows reads no command lines, so
/// there a refusal stands and no table is loaded.
pub(crate) fn liveness<T>(
    socket: &Path,
    reply: Reply<T>,
    table: Option<&process::ProcessTable>,
) -> Result<Liveness<T>> {
    Ok(match reply {
        Reply::Answered(answer) => Liveness::Answered(answer),
        Reply::TimedOut => Liveness::Unresponsive,
        Reply::Failed(error) => bail!("{error}"),
        #[cfg(windows)]
        Reply::Refused => {
            let _ = (socket, table);
            Liveness::Gone
        }
        #[cfg(unix)]
        Reply::Refused => {
            let loaded;
            let table = match table {
                Some(table) => table,
                None => {
                    loaded = process::ProcessTable::load()?;
                    &loaded
                }
            };
            if table.listeners(socket)?.is_empty() {
                Liveness::Gone
            } else {
                Liveness::Unresponsive
            }
        }
    })
}

/// Send one request to the daemon on `socket` as an `nvim --server <socket>
/// <args>` client, giving it `timeout` to answer. A client still waiting
/// then is killed and reaped, never left behind: on Unix it runs in its own
/// process group, and the whole group is killed, so an `nvim_bin` launcher
/// that starts the real nvim as its child does not leave that one behind.
/// Every request to a daemon goes through here; only the long-lived
/// `--remote-ui` attach does not. The client runs with `--clean`.
///
/// `Err` only when the client could not be run, killed or reaped, so it
/// says nothing about the daemon.
pub(crate) fn request(
    socket: &Path,
    args: &[&str],
    sidebar: &Sidebar,
    timeout: Duration,
) -> Result<Reply> {
    let mut command = nvim_cmd(sidebar);
    command
        .arg("--headless")
        // The client needs none of the user's config, and must not start
        // its plugins if `--remote` falls back to editing locally.
        .arg("--clean")
        .arg("--server")
        .arg(socket)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .spawn()
        .map_err(|error| run_error(&sidebar.nvim_bin, error))?;
    let stdout = drain(child.stdout.take().expect("stdout is piped"));
    let stderr = drain(child.stderr.take().expect("stderr is piped"));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => sleep(CLIENT_POLL_INTERVAL),
            Ok(None) => {
                kill_client(&mut child)?;
                break None;
            }
            Err(error) => {
                let _ = kill_client(&mut child);
                return Err(error).context("failed to wait for an nvim --server client");
            }
        }
    };
    let drained = Instant::now() + DRAIN_TIMEOUT;
    let (stdout, stderr) = (output(stdout, drained), output(stderr, drained));
    match status {
        Some(status) if status.success() => Ok(Reply::Answered(stdout)),
        // E247: "Failed to connect to '<socket>'". A `--remote` client that
        // gets it does not exit: it "edits locally" until it is killed.
        _ if stderr.contains("E247:") => Ok(Reply::Refused),
        Some(status) => Ok(Reply::Failed(format!(
            "nvim --server {} {} failed ({status}): {stderr}",
            socket.display(),
            args.first().copied().unwrap_or_default()
        ))),
        None => Ok(Reply::TimedOut),
    }
}

/// How long a request waits, once its client is reaped, for the rest of
/// the client's output.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// Read `pipe` to its end on the side, so a large answer cannot fill it and
/// stall the client.
fn drain(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut out = Vec::new();
        let _ = pipe.read_to_end(&mut out);
        let _ = sender.send(String::from_utf8_lossy(&out).trim().to_owned());
    });
    receiver
}

/// What `drain` read, waiting no later than `deadline`. Once the client and
/// its process group are dead the pipe is closed, but a process that left
/// the group could still hold it open; that must not block the caller, so
/// the reader is then left to finish on its own.
fn output(drained: mpsc::Receiver<String>, deadline: Instant) -> String {
    drained
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .unwrap_or_default()
}

/// Kill a request's client, with everything it started (its process
/// group, Unix), and reap it.
fn kill_client(child: &mut Child) -> Result<()> {
    #[cfg(unix)]
    let killed = Command::new("kill")
        .args(["-s", "KILL", "--", &format!("-{}", child.id())])
        .output()
        .context("could not run kill")
        .and_then(|output| {
            if !output.status.success() {
                bail!(
                    "kill -s KILL -- -{} failed ({}): {}",
                    child.id(),
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Ok(())
        });
    #[cfg(windows)]
    let killed = child
        .kill()
        .context("failed to kill an nvim --server client");
    if killed.is_err() {
        let _ = child.kill();
    }
    child
        .wait()
        .context("failed to reap an nvim --server client")?;
    killed
}

/// `request` for a caller that needs the answer: the answer, or an error
/// saying why there is none.
pub(crate) fn ask(
    socket: &Path,
    args: &[&str],
    sidebar: &Sidebar,
    timeout: Duration,
) -> Result<String> {
    match request(socket, args, sidebar, timeout)? {
        Reply::Answered(answer) => Ok(answer),
        Reply::Refused => bail!("no nvim daemon accepts connections on {}", socket.display()),
        Reply::TimedOut => bail!(
            "the nvim daemon on {} is busy or hung (no reply within {}s)",
            socket.display(),
            timeout.as_secs()
        ),
        Reply::Failed(error) => bail!("{error}"),
    }
}

/// `ask` the daemon for the value of a vimscript expression
/// (`--remote-expr`), giving it `RPC_TIMEOUT`.
pub(crate) fn ask_expr(socket: &Path, expr: &str, sidebar: &Sidebar) -> Result<String> {
    ask(socket, &["--remote-expr", expr], sidebar, RPC_TIMEOUT)
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
    ask_expr(socket, &expr, sidebar).with_context(|| {
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
/// The error for an nvim client that could not be run. A health check runs
/// `nvim_bin` before anything else, so this is where a missing nvim shows.
/// herdr hands the plugin the herdr server's PATH, which may not be the
/// person's, so "No such file or directory" alone does not tell them what to
/// change: this names the `nvim_bin` setting and its file.
fn run_error(bin: &str, error: std::io::Error) -> anyhow::Error {
    if error.kind() != std::io::ErrorKind::NotFound {
        return anyhow::Error::new(error).context(format!("failed to run {bin}"));
    }
    let file = crate::config::config_file_display();
    if Path::new(bin).components().count() > 1 {
        return anyhow::anyhow!("`{bin}` was not found: fix `nvim_bin` under [sidebar] in {file}");
    }
    let find = if cfg!(windows) { "where" } else { "command -v" };
    anyhow::anyhow!(
        "`{bin}` was not found on the PATH herdr gave herdr-nvim: set `nvim_bin` \
         under [sidebar] in {file} to nvim's full path (`{find} {bin}` prints it)"
    )
}

/// The sidebar pane's entry point. herdr closes a plugin pane the moment it
/// exits, so an error that stops the sidebar from starting is printed in the
/// pane, which waits for Enter: otherwise the person sees it flash and close.
pub fn sidebar_cmd() -> Result<()> {
    let (socket, sidebar) = start_sidebar().inspect_err(show_start_failure)?;
    let attached = attach_remote_ui(&socket, &sidebar);
    // On Unix the attach `exec`s nvim, so it returns only if nvim could not
    // start. On Windows it returns when the session ends, which is not a
    // start failure.
    if cfg!(unix) {
        attached.inspect_err(show_start_failure)
    } else {
        attached
    }
}

fn show_start_failure(error: &anyhow::Error) {
    use std::io::{BufRead, IsTerminal};

    if !std::io::stdin().is_terminal() {
        return;
    }
    eprintln!(
        "herdr-nvim: the nvim sidebar could not start: {error:#}\n\nPress Enter to close this pane."
    );
    let _ = std::io::stdin().lock().read_line(&mut String::new());
}

/// Everything before the attach: the tab's daemon, up and told this pane's
/// id. Returns its socket and the settings the attach needs.
fn start_sidebar() -> Result<(PathBuf, Sidebar)> {
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
    Ok((socket, config.sidebar))
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

    // herdr hands the plugin the herdr server's PATH, which may lack the
    // folder nvim is in. The error is what the person sees in the sidebar
    // pane, so it names what was missing and the setting that fixes it.
    #[test]
    fn a_missing_nvim_names_the_nvim_bin_setting() {
        let _env = TestEnv::new();
        let plugin_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        for bin in ["herdr-nvim-test-no-such-nvim", "/nonexistent/bin/nvim"] {
            let mut config = Config::default();
            config.sidebar.nvim_bin = bin.to_owned();
            let error = ensure_daemon("wM:t1", &plugin_root, &config, &plugin_root)
                .map(drop)
                .map_err(|e| format!("{e:#}"))
                .unwrap_err();
            assert!(error.contains(&format!("`{bin}` was not found")), "{error}");
            assert!(error.contains("`nvim_bin` under [sidebar]"), "{error}");
        }
    }

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
            outcome
                .as_ref()
                .is_err_and(|error| error.contains("busy or hung")),
            "ensure_daemon must report an unresponsive daemon, got {outcome:?}"
        );
        assert_eq!(
            daemon.eval("getpid()"),
            daemon.pid.to_string(),
            "the wedged daemon must not be replaced by a twin on its socket"
        );
    }

    // Issue #61 (principles review of PR #63): a daemon that another caller
    // has just spawned runs, `--listen <socket>` argv and all, a moment
    // before it accepts connections. A refusal then is a daemon starting up,
    // not a hung one: `ensure_daemon` must wait for it within its deadline,
    // and not spawn a twin over it. The stand-in is an nvim with that argv
    // (after `--`, as file names) that starts listening only after 1.5 s.
    #[cfg(unix)]
    #[test]
    fn ensure_daemon_waits_for_a_daemon_that_is_still_starting() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let _env = TestEnv::new();
        let plugin_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let config = Config::default();
        let socket = socket_path("w61:t3");
        fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listen_late = format!(
            "lua vim.uv.sleep(1500) vim.fn.serverstart({:?})",
            socket.to_string_lossy()
        );
        let starting = Command::new("nvim")
            .args([
                "--headless",
                "--clean",
                "--cmd",
                &listen_late,
                "--",
                "--listen",
            ])
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn nvim");
        // Killed on drop.
        let daemon = TestDaemon {
            tab: TabId::new("w61:t3"),
            socket: socket.clone(),
            pid: starting.id(),
        };
        let reply = remote_expr(&socket, "1+1", &config.sidebar, RPC_TIMEOUT).unwrap();
        assert_eq!(
            reply,
            Reply::Refused,
            "precondition: it does not accept yet"
        );

        let outcome = ensure_daemon("w61:t3", &plugin_root, &config, &plugin_root);
        assert_eq!(outcome.map_err(|e| format!("{e:#}")), Ok(socket));
        assert_eq!(
            daemon.eval("getpid()"),
            daemon.pid.to_string(),
            "the starting daemon must not be replaced by a twin on its socket"
        );
    }

    /// Command lines of the `nvim --server` clients still talking to `socket`.
    #[cfg(unix)]
    fn clients_of(socket: &Path) -> Vec<String> {
        let output = Command::new("ps")
            .args(["-A", "-ww", "-o", "args="])
            .output()
            .expect("run ps");
        let needle = format!("--server {}", socket.display());
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains(&needle))
            .map(str::to_owned)
            .collect()
    }

    // Issue #61 (review of PR #63): `nvim_bin` may be a launcher that starts
    // the real nvim as its child instead of `exec`ing it (a wrapper script,
    // bob's shim). A request to a hung daemon through it must still give up
    // at its deadline, and leave no `nvim --server` client behind.
    #[cfg(unix)]
    #[test]
    fn a_request_through_a_launcher_shim_is_bounded_and_leaves_no_client() {
        use std::os::unix::fs::PermissionsExt;

        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let env = TestEnv::new();
        let config = Config::default();
        let daemon = TestDaemon::spawn("w61:t2", &config);
        daemon.hang();
        let nvim = Command::new("sh")
            .args(["-c", "command -v nvim"])
            .output()
            .expect("find nvim");
        let nvim = String::from_utf8_lossy(&nvim.stdout).trim().to_owned();
        let shim = env.dir.join("nvim-shim");
        fs::write(&shim, format!("#!/bin/sh\n{} \"$@\"\n", shell_quote(&nvim))).unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
        let sidebar = Sidebar {
            nvim_bin: shim.to_string_lossy().into_owned(),
            ..Default::default()
        };

        let (tx, rx) = std::sync::mpsc::channel();
        let socket = daemon.socket.clone();
        let started = Instant::now();
        let worker = std::thread::spawn(move || {
            let reply = remote_expr(&socket, "1+1", &sidebar, Duration::from_secs(1));
            let _ = tx.send(reply.map_err(|e| format!("{e:#}")));
        });
        let outcome = rx.recv_timeout(Duration::from_secs(5));
        let elapsed = started.elapsed();
        let leftover = clients_of(&daemon.socket);
        // Unwedge before asserting, so a failure leaves nothing stuck.
        daemon.resume();
        let _ = worker.join();

        let outcome = outcome
            .unwrap_or_else(|_| panic!("a request through a shim still blocked after {elapsed:?}"));
        assert_eq!(outcome, Ok(Reply::TimedOut));
        assert!(leftover.is_empty(), "clients left behind: {leftover:?}");
    }

    // Issue #61 (review of PR #63): `--remote` that cannot connect prints
    // E247 and then "edits locally": it becomes a whole headless nvim of its
    // own, which never exits. That is a refusal, not a hung daemon, and the
    // stand-in nvim must not load the user's config (plugins, LSPs) first.
    #[test]
    fn a_remote_open_with_nothing_listening_is_refused_without_user_config() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let env = TestEnv::new();
        let marker = env.dir.join("config-loaded");
        let init = env.dir.join("xdg-config").join("nvim").join("init.lua");
        fs::create_dir_all(init.parent().unwrap()).unwrap();
        fs::write(
            &init,
            format!(
                "vim.fn.writefile({{'loaded'}}, {:?})",
                marker.to_string_lossy()
            ),
        )
        .unwrap();
        let socket = env.dir.join("nothing-here.sock");
        let file = env.dir.join("picked.txt").to_string_lossy().into_owned();

        let reply = request(
            &socket,
            &["--remote", &file],
            &Sidebar::default(),
            Duration::from_secs(2),
        );
        assert_eq!(reply.map_err(|e| format!("{e:#}")), Ok(Reply::Refused));
        assert!(!marker.exists(), "the client loaded the user's config");
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
