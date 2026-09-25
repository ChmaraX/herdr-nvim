use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use std::{ffi::OsStr, io::ErrorKind};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use crate::{
    config::{Config, Sidebar},
    herdr::{CliHerdr, Herdr},
    state::{self, tab_key},
};

const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const HEALTH_POLL_TIMEOUT: Duration = Duration::from_secs(10);

const READY_POLL_INTERVAL: Duration = Duration::from_millis(50);
// Short: only waits out the tail of an already-in-flight `maneuver::open`,
// never a fresh operation, so a stale size is preferable to a hung-looking pane.
const READY_POLL_TIMEOUT: Duration = Duration::from_secs(2);

#[cfg(test)]
pub static RUNTIME_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
fn registry_path(key: &str) -> PathBuf {
    socket_dir().join(format!("{key}.{REGISTRY_EXT}"))
}

/// The workspace id embedded in a tab id. Tab ids are `<workspace>:<tab>`, so
/// the workspace is the prefix before the first `:`. A tab id without a `:`
/// (never expected in practice) is returned unchanged rather than panicking.
fn workspace_of_tab(tab: &str) -> &str {
    tab.split_once(':').map_or(tab, |(workspace, _)| workspace)
}

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
pub fn ensure_daemon(
    tab: &str,
    plugin_root: &Path,
    config: &Config,
    cwd: &Path,
) -> Result<PathBuf> {
    let socket = socket_path(tab);
    if daemon_healthy(&socket, &config.sidebar) {
        // Also (re)register an already-running daemon, so one spawned before
        // the Windows marker existed still becomes discoverable.
        #[cfg(windows)]
        register_daemon(tab)?;
        return Ok(socket);
    }

    #[cfg(not(windows))]
    {
        let dir = socket
            .parent()
            .with_context(|| format!("socket path has no parent: {}", socket.display()))?;
        fs::create_dir_all(dir)
            .with_context(|| format!("failed to create runtime directory {}", dir.display()))?;
        // A stale (dead) socket file would make `nvim --listen` fail to bind.
        // We only get here after the health check failed, so any file present
        // is dead.
        remove_file_if_exists(&socket)?;
    }

    spawn_daemon(tab, &socket, plugin_root, &config.sidebar, cwd)?;
    #[cfg(windows)]
    register_daemon(tab)?;

    let deadline = Instant::now() + HEALTH_POLL_TIMEOUT;
    loop {
        if daemon_healthy(&socket, &config.sidebar) {
            return Ok(socket);
        }
        if Instant::now() >= deadline {
            bail!(
                "nvim daemon for tab {tab} did not become healthy within {}s",
                HEALTH_POLL_TIMEOUT.as_secs()
            );
        }
        sleep(HEALTH_POLL_INTERVAL);
    }
}

/// Windows only: record the daemon in `socket_dir` (named pipes themselves
/// cannot be listed). The marker holds the raw tab id for diagnostics.
#[cfg(windows)]
fn register_daemon(tab: &str) -> Result<()> {
    let marker = registry_path(&tab_key(tab));
    let dir = socket_dir();
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create runtime directory {}", dir.display()))?;
    fs::write(&marker, tab)
        .with_context(|| format!("failed to write daemon marker {}", marker.display()))
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
    let workspace = workspace_of_tab(tab);

    let mut command = nvim_cmd(sidebar);
    command
        .arg("--headless")
        .arg("--listen")
        .arg(socket)
        .arg("--cmd")
        .arg(format!("set rtp+={}", plugin_root.display()))
        .arg("--cmd")
        .arg(&vim_enter)
        .env("HERDR_WORKSPACE_ID", workspace)
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

fn daemon_healthy(socket: &Path, sidebar: &Sidebar) -> bool {
    remote_expr(socket, "1+1", sidebar).as_deref() == Some("2")
}

/// Evaluate a vimscript expression on the daemon via `--remote-expr`, returning
/// the trimmed stdout, or `None` if the daemon is unreachable.
pub(crate) fn remote_expr(socket: &Path, expr: &str, sidebar: &Sidebar) -> Option<String> {
    let output = nvim_cmd(sidebar)
        .arg("--headless")
        .arg("--server")
        .arg(socket)
        .arg("--remote-expr")
        .arg(expr)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
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
/// (non-interactive, no shell echo). herdr sets `HERDR_TAB_ID` for the pane so
/// it knows which tab's daemon to attach to, and the pane's own cwd (set via
/// `--cwd`) is where the daemon spawns.
pub fn sidebar_cmd() -> Result<()> {
    let tab = env::var("HERDR_TAB_ID")
        .context("herdr-nvim sidebar requires HERDR_TAB_ID (set by herdr for plugin panes)")?;
    let cwd = env::current_dir().context("herdr-nvim sidebar could not resolve its cwd")?;
    let plugin_root = plugin_root()?;
    let config = crate::config::load();
    let socket = ensure_daemon(&tab, &plugin_root, &config, &cwd)?;
    wait_for_layout_ready();

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

/// Garbage-collect daemons whose tab no longer exists: quit the daemon and
/// remove its socket and state file.
pub fn gc_cmd() -> Result<()> {
    let mut herdr = CliHerdr;
    let config = crate::config::load();
    gc(&mut herdr, &config.sidebar)
}

/// `pub(crate)` so `maneuver::toggle` can run an opportunistic, best-effort gc
/// on every toggle to reap stale per-tab daemons from closed tabs.
pub(crate) fn gc(h: &mut dyn Herdr, sidebar: &Sidebar) -> Result<()> {
    let Some(keys) = daemon_keys()? else {
        return Ok(());
    };
    let known: Vec<String> = h.list_tabs()?.iter().map(|tab| tab_key(tab)).collect();
    for key in keys {
        if !known.contains(&key) {
            stop_tab_key(&key, sidebar)?;
        }
    }
    Ok(())
}

/// Sanitized tab keys of every registered daemon (see `socket_dir`), or
/// `None` when the runtime directory does not exist yet (nothing to reap).
pub(crate) fn daemon_keys() -> Result<Option<Vec<String>>> {
    let dir = socket_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
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
    Ok(Some(keys))
}

/// Vimscript: number of listed buffers with unsaved changes.
pub(crate) const UNSAVED_BUFFERS_EXPR: &str =
    "len(filter(getbufinfo({'bufmodified':1}),'v:val.listed'))";

/// Stop the daemon for an already-sanitized tab key (a `socket_dir` entry
/// stem, see `state::tab_key`): force-quit it if it is still running, then
/// drop its registry entry and sidebar state file. Unsaved buffers are
/// discarded -- the tab is gone, or `daemons stop --force` asked for it --
/// but reported on stderr (herdr's plugin command log). A daemon that is already gone is a no-op
/// beyond the file cleanup.
pub(crate) fn stop_tab_key(key: &str, sidebar: &Sidebar) -> Result<()> {
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
    remove_file_if_exists(&registry_path(key))?;
    state::remove_key(key)
}

/// A close event herdr delivered to the `on-event` hook.
#[derive(Debug, PartialEq)]
enum CloseEvent {
    Tab(String),
    Workspace(String),
}

/// Parse `HERDR_PLUGIN_EVENT_JSON`. Anything that is not a well-formed
/// `tab_closed`/`workspace_closed` event yields `None` (ignored).
fn parse_close_event(raw: &str) -> Option<CloseEvent> {
    let value: Value = serde_json::from_str(raw).ok()?;
    let event = value
        .get("event")
        .or_else(|| value.pointer("/data/type"))
        .and_then(Value::as_str)?;
    let id = |pointer: &str| {
        value
            .pointer(pointer)
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    };
    match event {
        "tab_closed" => id("/data/tab_id").map(CloseEvent::Tab),
        "workspace_closed" => id("/data/workspace_id").map(CloseEvent::Workspace),
        _ => None,
    }
}

/// Stop the daemon(s) a close event makes obsolete. A workspace close does
/// not fire `tab_closed` for its tabs, so it stops every daemon whose key
/// starts with `<workspace>_` (tab ids are `<workspace>:<tab>`); the trailing
/// `_` keeps workspace `w7` from matching `w7B`'s tabs. Best effort: one
/// daemon failing to stop does not spare the rest.
fn handle_close_event(event: &CloseEvent, sidebar: &Sidebar) -> Result<()> {
    match event {
        CloseEvent::Tab(tab) => stop_tab_key(&tab_key(tab), sidebar),
        CloseEvent::Workspace(workspace) => {
            let prefix = format!("{}_", tab_key(workspace));
            let mut result = Ok(());
            for key in daemon_keys()?.unwrap_or_default() {
                if key.starts_with(&prefix) {
                    if let Err(err) = stop_tab_key(&key, sidebar) {
                        result = result.and(Err(err));
                    }
                }
            }
            result
        }
    }
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

/// Ask the daemon to force-quit (`qa!`), then wait until it stops answering
/// so callers can rely on it being gone.
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
    while Instant::now() < deadline && remote_expr(socket, "1", sidebar).is_some() {
        sleep(QUIT_POLL_INTERVAL);
    }
    Ok(())
}

/// Remove a file, treating "already gone" as success.
fn remove_file_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to remove {}", path.display())),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        ffi::OsString,
        fs,
        sync::{
            atomic::{AtomicUsize, Ordering},
            MutexGuard,
        },
    };

    use super::*;

    use crate::herdr::MockHerdr;
    use std::collections::VecDeque;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Redirects the socket dir (and an isolated, empty nvim config) into a
    /// unique temp dir for the duration of a test, restoring the prior env on
    /// drop. The config isolation keeps any real user init.lua out of the
    /// spawned daemon so tests are hermetic and fast.
    pub(crate) struct RuntimeEnvGuard {
        _lock: MutexGuard<'static, ()>,
        old_runtime: Option<OsString>,
        old_config: Option<OsString>,
        dir: PathBuf,
    }

    impl RuntimeEnvGuard {
        pub(crate) fn new() -> Self {
            let lock = RUNTIME_DIR_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let dir = env::temp_dir().join(format!(
                "hn-daemon-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();

            let old_runtime = env::var_os("HERDR_NVIM_RUNTIME_DIR");
            let old_config = env::var_os("XDG_CONFIG_HOME");
            env::set_var("HERDR_NVIM_RUNTIME_DIR", &dir);
            env::set_var("XDG_CONFIG_HOME", dir.join("xdg-config"));

            Self {
                _lock: lock,
                old_runtime,
                old_config,
                dir,
            }
        }
    }

    impl Drop for RuntimeEnvGuard {
        fn drop(&mut self) {
            match &self.old_runtime {
                Some(value) => env::set_var("HERDR_NVIM_RUNTIME_DIR", value),
                None => env::remove_var("HERDR_NVIM_RUNTIME_DIR"),
            }
            match &self.old_config {
                Some(value) => env::set_var("XDG_CONFIG_HOME", value),
                None => env::remove_var("XDG_CONFIG_HOME"),
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    pub(crate) fn nvim_available() -> bool {
        #[cfg(not(windows))]
        {
            return Command::new("which")
                .arg("nvim")
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false);
        }

        #[cfg(windows)]
        {
            return Command::new("where.exe")
                .arg("nvim")
                .output()
                .map(|output| output.status.success())
                .unwrap_or(false);
        }
    }

    /// Best-effort daemon shutdown so no stray `nvim --headless` survives a test.
    fn stop_daemon(socket: &Path) {
        let _ = send_quit(socket, &Sidebar::default());
        #[cfg(not(windows))]
        let _ = remove_file_if_exists(socket);
    }

    /// Points `HERDR_NVIM_STATE_DIR` into the runtime guard's temp dir while
    /// alive: stopping a daemon removes its tab's state file, which must never
    /// touch the real user state dir. Create it BEFORE the `RuntimeEnvGuard`:
    /// the state lock must be taken first, matching the maneuver tests' lock
    /// order, or the two test modules can deadlock.
    pub(crate) struct StateEnvGuard {
        _lock: MutexGuard<'static, ()>,
        old: Option<OsString>,
    }

    impl StateEnvGuard {
        pub(crate) fn new() -> Self {
            let lock = state::STATE_DIR_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let old = env::var_os("HERDR_NVIM_STATE_DIR");
            Self { _lock: lock, old }
        }

        pub(crate) fn point_into(&self, runtime: &RuntimeEnvGuard) {
            env::set_var("HERDR_NVIM_STATE_DIR", runtime.dir.join("state"));
        }
    }

    impl Drop for StateEnvGuard {
        fn drop(&mut self) {
            match &self.old {
                Some(value) => env::set_var("HERDR_NVIM_STATE_DIR", value),
                None => env::remove_var("HERDR_NVIM_STATE_DIR"),
            }
        }
    }

    /// Whether the OS still runs `pid`. The test process is the daemons'
    /// parent and never reaps them, so on Unix an exited daemon lingers as a
    /// zombie -- that counts as dead.
    pub(crate) fn process_alive(pid: &str) -> bool {
        #[cfg(not(windows))]
        {
            let output = Command::new("ps")
                .args(["-o", "stat=", "-p", pid])
                .output()
                .expect("run ps");
            let stat = String::from_utf8_lossy(&output.stdout);
            let stat = stat.trim();
            !stat.is_empty() && !stat.starts_with('Z')
        }
        #[cfg(windows)]
        {
            let output = Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
                .output()
                .expect("run tasklist");
            String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
        }
    }

    /// A real daemon spawned for a test tab, with the pid it reported. Quits
    /// the daemon on drop, so a failing assertion never leaks a process.
    pub(crate) struct TestDaemon {
        pub(crate) tab: &'static str,
        pub(crate) socket: PathBuf,
        pub(crate) pid: String,
    }

    impl TestDaemon {
        pub(crate) fn spawn(tab: &'static str, config: &Config) -> Self {
            let plugin_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let socket =
                ensure_daemon(tab, &plugin_root, config, &plugin_root).expect("ensure_daemon");
            let pid = remote_expr(&socket, "getpid()", &config.sidebar).expect("daemon pid");
            // Give the daemon a sidebar state file so its cleanup is observable.
            let state_file = state::state_path(tab);
            fs::create_dir_all(state_file.parent().unwrap()).unwrap();
            fs::write(&state_file, b"{}").unwrap();
            Self { tab, socket, pid }
        }

        pub(crate) fn assert_alive(&self, sidebar: &Sidebar) {
            assert!(process_alive(&self.pid), "{} daemon process died", self.tab);
            assert_eq!(
                remote_expr(&self.socket, "1+1", sidebar).as_deref(),
                Some("2"),
                "{} daemon stopped answering",
                self.tab
            );
            assert!(
                registry_path(&tab_key(self.tab)).exists(),
                "{} registry entry removed",
                self.tab
            );
            assert!(
                state::state_path(self.tab).exists(),
                "{} state file removed",
                self.tab
            );
        }

        pub(crate) fn assert_gone(&self, sidebar: &Sidebar) {
            assert!(
                !process_alive(&self.pid),
                "{} daemon process still running",
                self.tab
            );
            assert!(
                remote_expr(&self.socket, "1+1", sidebar).is_none(),
                "{} daemon still answering",
                self.tab
            );
            assert!(
                !registry_path(&tab_key(self.tab)).exists(),
                "{} registry entry left behind",
                self.tab
            );
            assert!(
                !state::state_path(self.tab).exists(),
                "{} state file left behind",
                self.tab
            );
        }
    }

    impl Drop for TestDaemon {
        fn drop(&mut self) {
            stop_daemon(&self.socket);
        }
    }

    // The event-hook scenario end to end: real daemons, real close events as
    // herdr delivers them, observed through the actual processes and files.
    // `wHnAB` is the prefix trap -- closing workspace `wHnA` must not touch it.
    #[test]
    fn close_events_stop_exactly_the_closed_tabs_daemons() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }
        let state = StateEnvGuard::new();
        let runtime = RuntimeEnvGuard::new();
        state.point_into(&runtime);
        let config = Config::default();
        let sidebar = &config.sidebar;

        let a1 = TestDaemon::spawn("wHnA:t1", &config);
        let a2 = TestDaemon::spawn("wHnA:t2", &config);
        let ab1 = TestDaemon::spawn("wHnAB:t1", &config);
        let b1 = TestDaemon::spawn("wHnB:t1", &config);
        // Unsaved work in the closing tab is discarded, not a blocker.
        // `noswapfile`: parallel tests may point HOME somewhere unwritable,
        // and a failed swap-file creation would abort the edit.
        remote_expr(
            &a1.socket,
            r#"execute('setlocal noswapfile | call setline(1, "unsaved")')"#,
            sidebar,
        )
        .expect("dirty a buffer");
        assert_eq!(
            remote_expr(&a1.socket, UNSAVED_BUFFERS_EXPR, sidebar).as_deref(),
            Some("1")
        );
        let all = [&a1, &a2, &ab1, &b1];
        for daemon in all {
            daemon.assert_alive(sidebar);
        }

        let deliver = |raw: &str| {
            if let Some(event) = parse_close_event(raw) {
                handle_close_event(&event, sidebar).expect("handle event");
            }
        };

        deliver(
            r#"{"event":"tab_closed","data":{"type":"tab_closed","tab_id":"wHnA:t1","workspace_id":"wHnA"}}"#,
        );
        a1.assert_gone(sidebar);
        for daemon in [&a2, &ab1, &b1] {
            daemon.assert_alive(sidebar);
        }

        deliver(
            r#"{"event":"workspace_closed","data":{"type":"workspace_closed","workspace_id":"wHnA","workspace":{"workspace_id":"wHnA","label":"x"}}}"#,
        );
        a2.assert_gone(sidebar);
        ab1.assert_alive(sidebar);
        b1.assert_alive(sidebar);

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
        ab1.assert_alive(sidebar);
        b1.assert_alive(sidebar);
    }

    #[test]
    fn malformed_or_foreign_event_json_is_ignored() {
        for raw in [
            "",
            "not json",
            "{}",
            "[]",
            r#"{"event":"tab_closed"}"#,
            r#"{"event":"tab_closed","data":{"tab_id":""}}"#,
            r#"{"event":"tab_closed","data":{"tab_id":7}}"#,
            r#"{"event":"workspace_closed","data":{}}"#,
            r#"{"event":"tab_created","data":{"tab_id":"w1:t1"}}"#,
        ] {
            assert_eq!(parse_close_event(raw), None, "{raw:?}");
        }
    }

    #[test]
    fn socket_path_respects_runtime_dir_override() {
        let _guard = RuntimeEnvGuard::new();
        #[cfg(not(windows))]
        assert_eq!(socket_path("wsX"), _guard.dir.join("wsX.sock"));
        #[cfg(windows)]
        assert_eq!(
            socket_path("wsX"),
            PathBuf::from(r"\\.\pipe\herdr-nvim-wsX")
        );
    }

    #[test]
    fn socket_path_sanitizes_colon_in_tab_id() {
        let _guard = RuntimeEnvGuard::new();
        #[cfg(not(windows))]
        assert_eq!(socket_path("wX:t1"), _guard.dir.join("wX_t1.sock"));
        #[cfg(windows)]
        assert_eq!(
            socket_path("wX:t1"),
            PathBuf::from(r"\\.\pipe\herdr-nvim-wX_t1")
        );
    }

    #[test]
    fn workspace_of_tab_takes_the_prefix_before_the_colon() {
        assert_eq!(workspace_of_tab("w39:t1"), "w39");
        // Only the first colon splits; the rest belongs to the tab segment.
        assert_eq!(workspace_of_tab("w39:t1:x"), "w39");
        // No colon / empty: return the input unchanged rather than panicking.
        assert_eq!(workspace_of_tab("w39"), "w39");
        assert_eq!(workspace_of_tab(""), "");
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
        let _guard = RuntimeEnvGuard::new();
        let plugin_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let config = Config::default();

        // Mimic the picker/finisher parent env: JSON blob present, flat vars
        // absent -- exactly what the plugin pane hands down.
        env::remove_var("HERDR_WORKSPACE_ID");
        env::remove_var("HERDR_TAB_ID");
        env::set_var(
            "HERDR_PLUGIN_CONTEXT_JSON",
            r#"{"workspace_id":"w39","tab_id":"w39:t1"}"#,
        );

        let socket =
            ensure_daemon("w39:t1", &plugin_root, &config, &plugin_root).expect("ensure_daemon");

        // What agents.lua reads inside the daemon must now be populated.
        let ws = remote_expr(&socket, "$HERDR_WORKSPACE_ID", &config.sidebar);
        let tab = remote_expr(&socket, "$HERDR_TAB_ID", &config.sidebar);

        env::remove_var("HERDR_PLUGIN_CONTEXT_JSON");
        stop_daemon(&socket);

        assert_eq!(
            ws.as_deref(),
            Some("w39"),
            "daemon must have HERDR_WORKSPACE_ID"
        );
        assert_eq!(
            tab.as_deref(),
            Some("w39:t1"),
            "daemon must have HERDR_TAB_ID"
        );
    }

    #[test]
    fn ensure_daemon_spawns_then_is_idempotent_against_real_nvim() {
        if !nvim_available() {
            eprintln!("skipping: nvim not found on PATH");
            return;
        }

        let _guard = RuntimeEnvGuard::new();
        let plugin_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let config = Config::default();

        // Reuse plugin_root (a real, already-existing directory) as the cwd
        // too -- no need for a second temp dir just for this.
        let socket =
            ensure_daemon("wD", &plugin_root, &config, &plugin_root).expect("first ensure_daemon");
        #[cfg(not(windows))]
        assert!(socket.exists(), "socket file should exist after spawn");
        #[cfg(windows)]
        assert!(remote_expr(&socket, "1+1", &config.sidebar).is_some());

        let pid1 =
            remote_expr(&socket, "getpid()", &config.sidebar).expect("daemon should report a pid");
        assert!(!pid1.is_empty());

        let socket_again =
            ensure_daemon("wD", &plugin_root, &config, &plugin_root).expect("second ensure_daemon");
        assert_eq!(socket, socket_again);

        let pid2 = remote_expr(&socket, "getpid()", &config.sidebar)
            .expect("daemon should still report a pid");
        assert_eq!(
            pid1, pid2,
            "second ensure_daemon must reuse the daemon, not spawn a new one"
        );

        stop_daemon(&socket);
    }

    #[test]
    fn gc_removes_orphan_sockets_and_keeps_known_tabs() {
        // Isolate the state dir too, since gc removes orphan state files.
        let state = StateEnvGuard::new();
        let guard = RuntimeEnvGuard::new();
        state.point_into(&guard);

        // Two dead registry entries (sockets on Unix, pipe markers on
        // Windows); only "wsKeep:t1" is still a live tab.
        let keep = registry_path(&tab_key("wsKeep:t1"));
        let orphan = registry_path(&tab_key("wsOrphan:t1"));
        fs::write(&keep, b"").unwrap();
        fs::write(&orphan, b"").unwrap();

        let mut herdr = MockHerdr {
            list_tabs_results: VecDeque::from([Ok(vec!["wsKeep:t1".to_owned()])]),
            ..Default::default()
        };
        gc(&mut herdr, &Sidebar::default()).unwrap();

        assert!(keep.exists(), "known tab kept");
        assert!(!orphan.exists(), "orphan tab entry removed");
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
