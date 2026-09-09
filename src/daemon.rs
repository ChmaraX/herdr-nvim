use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

#[cfg(not(windows))]
use std::{ffi::OsStr, fs, io::ErrorKind};

use anyhow::{bail, Context, Result};

use crate::{
    config::{Config, Sidebar},
    herdr::{CliHerdr, Herdr},
    state::tab_key,
};

#[cfg(not(windows))]
use crate::state;

const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const HEALTH_POLL_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(test)]
pub static RUNTIME_DIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Directory that holds one `<tab>.sock` per running daemon, one per tab.
///
/// `HERDR_NVIM_RUNTIME_DIR` overrides everything (used by tests); otherwise the
/// XDG runtime dir, falling back to the platform temp dir. Windows uses named
/// pipes instead of filesystem sockets, so this directory is only used by the
/// Unix implementation.
#[cfg(not(windows))]
fn socket_dir() -> PathBuf {
    env::var_os("HERDR_NVIM_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("XDG_RUNTIME_DIR").map(|path| PathBuf::from(path).join("herdr-nvim"))
        })
        .unwrap_or_else(|| env::temp_dir().join("herdr-nvim"))
}

pub fn socket_path(tab: &str) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(format!(r"\\.\pipe\herdr-nvim-{}", tab_key(tab)))
    }
    #[cfg(not(windows))]
    {
        socket_dir().join(format!("{}.sock", tab_key(tab)))
    }
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
        remove_socket(&socket)?;
    }
    #[cfg(windows)]
    let _ = socket;

    spawn_daemon(tab, &socket, plugin_root, &config.sidebar, cwd)?;

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
fn remote_expr(socket: &Path, expr: &str, sidebar: &Sidebar) -> Option<String> {
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
    #[cfg(windows)]
    {
        // Windows named pipes cannot be enumerated through read_dir. The pipe
        // disappears with its daemon, so there is no socket cleanup to do.
        let _ = (h, sidebar);
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let dir = socket_dir();
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("failed to read runtime directory {}", dir.display()))
            }
        };

        let tabs = h.list_tabs()?;
        let known: Vec<String> = tabs.iter().map(|tab| tab_key(tab)).collect();
        for entry in entries {
            let path = entry
                .with_context(|| format!("failed to read entry in {}", dir.display()))?
                .path();
            if path.extension().and_then(OsStr::to_str) != Some("sock") {
                continue;
            }
            let Some(tab_stem) = path.file_stem().and_then(OsStr::to_str) else {
                continue;
            };
            if known.iter().any(|known_tab| known_tab == tab_stem) {
                continue;
            }

            // Orphaned: ask the daemon (if any) to quit, then unlink socket +
            // state. `tab_stem` is a filename component, already sanitized (see
            // `state::tab_key`), so it goes through `state::remove_key` rather
            // than `state::remove` -- that avoids sanitizing an already-sanitized
            // key a second time.
            let _ = send_quit(&path, sidebar);
            remove_socket(&path)?;
            state::remove_key(tab_stem)?;
        }
        Ok(())
    }
}

#[cfg(any(not(windows), test))]
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
    // Give the daemon a moment to process the quit before we unlink the socket.
    sleep(Duration::from_millis(200));
    Ok(())
}

#[cfg(not(windows))]
fn remove_socket(socket: &Path) -> Result<()> {
    match fs::remove_file(socket) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(err).with_context(|| format!("failed to remove socket {}", socket.display()))
        }
    }
}

#[cfg(all(windows, test))]
fn remove_socket(_socket: &Path) -> Result<()> {
    // Named pipes are kernel objects and are removed when their owner exits.
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        fs,
        sync::{
            atomic::{AtomicUsize, Ordering},
            MutexGuard,
        },
    };

    use super::*;

    #[cfg(not(windows))]
    use crate::{herdr::MockHerdr, state};
    #[cfg(not(windows))]
    use std::collections::VecDeque;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Redirects the socket dir (and an isolated, empty nvim config) into a
    /// unique temp dir for the duration of a test, restoring the prior env on
    /// drop. The config isolation keeps any real user init.lua out of the
    /// spawned daemon so tests are hermetic and fast.
    struct RuntimeEnvGuard {
        _lock: MutexGuard<'static, ()>,
        old_runtime: Option<OsString>,
        old_config: Option<OsString>,
        dir: PathBuf,
    }

    impl RuntimeEnvGuard {
        fn new() -> Self {
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

    fn nvim_available() -> bool {
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
        for _ in 0..50 {
            if remote_expr(socket, "1+1", &Sidebar::default()).is_none() {
                break;
            }
            sleep(Duration::from_millis(100));
        }
        let _ = remove_socket(socket);
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

    #[cfg(not(windows))]
    #[test]
    fn gc_removes_orphan_sockets_and_keeps_known_tabs() {
        let guard = RuntimeEnvGuard::new();

        // Isolate the state dir too, since gc removes orphan state files.
        let state_lock = state::STATE_DIR_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let old_state = env::var_os("HERDR_NVIM_STATE_DIR");
        env::set_var("HERDR_NVIM_STATE_DIR", guard.dir.join("state"));

        // Two dead socket files (sanitized tab ids); only "wsKeep:t1" is still a
        // live tab.
        fs::write(socket_path("wsKeep:t1"), b"").unwrap();
        fs::write(socket_path("wsOrphan:t1"), b"").unwrap();

        let mut herdr = MockHerdr {
            list_tabs_results: VecDeque::from([Ok(vec!["wsKeep:t1".to_owned()])]),
            ..Default::default()
        };
        gc(&mut herdr, &Sidebar::default()).unwrap();

        assert!(socket_path("wsKeep:t1").exists(), "known tab kept");
        assert!(
            !socket_path("wsOrphan:t1").exists(),
            "orphan tab socket removed"
        );

        match &old_state {
            Some(value) => env::set_var("HERDR_NVIM_STATE_DIR", value),
            None => env::remove_var("HERDR_NVIM_STATE_DIR"),
        }
        drop(state_lock);
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
