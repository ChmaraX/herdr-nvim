use std::{
    env,
    ffi::OsStr,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};

use crate::{
    config::Config,
    herdr::{CliHerdr, Herdr},
    state,
};

const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
const HEALTH_POLL_TIMEOUT: Duration = Duration::from_secs(10);

/// Directory that holds one `<workspace>.sock` per running daemon.
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

pub fn socket_path(workspace: &str) -> PathBuf {
    socket_dir().join(format!("{workspace}.sock"))
}

/// Ensure a per-workspace headless nvim daemon is listening on its socket,
/// returning the socket path. If a healthy daemon already exists this is a
/// no-op; otherwise a detached daemon is spawned and polled until healthy.
pub fn ensure_daemon(workspace: &str, plugin_root: &Path, config: &Config) -> Result<PathBuf> {
    let socket = socket_path(workspace);
    if daemon_healthy(&socket) {
        return Ok(socket);
    }

    let dir = socket
        .parent()
        .with_context(|| format!("socket path has no parent: {}", socket.display()))?;
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create runtime directory {}", dir.display()))?;
    // A stale (dead) socket file would make `nvim --listen` fail to bind. We
    // only get here after the health check failed, so any file present is dead.
    remove_socket(&socket)?;

    spawn_daemon(&socket, plugin_root, &config.sidebar.nvim_bin)?;

    let deadline = Instant::now() + HEALTH_POLL_TIMEOUT;
    loop {
        if daemon_healthy(&socket) {
            return Ok(socket);
        }
        if Instant::now() >= deadline {
            bail!(
                "nvim daemon for workspace {workspace} did not become healthy within {}s",
                HEALTH_POLL_TIMEOUT.as_secs()
            );
        }
        sleep(HEALTH_POLL_INTERVAL);
    }
}

fn spawn_daemon(socket: &Path, plugin_root: &Path, nvim_bin: &str) -> Result<()> {
    // The pre-init `set rtp+=` below is not enough on its own: LazyVim (and any
    // lazy.nvim config with the default `performance.rtp.reset = true`) rebuilds
    // runtimepath from scratch during startup, dropping our appended path before
    // VimEnter fires. So the VimEnter callback re-appends the plugin root to rtp
    // *after* the user's config has loaded, then requires the plugin. The whole
    // thing stays wrapped in `pcall` so a missing/broken plugin never crashes
    // the daemon.
    let vim_enter = format!(
        "lua vim.api.nvim_create_autocmd('VimEnter',{{callback=function() \
         pcall(function() vim.opt.rtp:append({root:?}); require('herdr-nvim').setup() end) end}})",
        root = plugin_root.display().to_string()
    );
    // The daemon must outlive the sidebar pane that spawns it (that is the whole
    // point of a persistent per-workspace daemon). It is spawned from inside the
    // sidebar pane's process, and when herdr closes that pane it tears down the
    // pane's SESSION -- every process sharing the pane's controlling terminal is
    // killed, regardless of process group or where it sits in the ppid tree
    // (verified live: a bare child, a child in its own process group, and even a
    // child reparented to init all die; only a process in its OWN session
    // survives). So put the daemon in a fresh session via setsid(2) before exec,
    // detaching it from the pane's controlling terminal so the pane-close
    // teardown can no longer reach it.
    use std::os::unix::process::CommandExt;

    let mut command = Command::new(nvim_bin);
    command
        .arg("--headless")
        .arg("--listen")
        .arg(socket)
        .arg("--cmd")
        .arg(format!("set rtp+={}", plugin_root.display()))
        .arg("--cmd")
        .arg(&vim_enter)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: the pre_exec closure runs in the forked child after fork() and
    // before exec(). It only calls setsid(2), which is async-signal-safe and
    // touches no memory shared with the parent, so it is safe in this context.
    // setsid returns -1 if the caller is already a process-group leader; a fresh
    // fork never is, so it succeeds, but we ignore the result either way because
    // a failure here must not abort the (otherwise valid) exec.
    unsafe {
        command.pre_exec(|| {
            extern "C" {
                fn setsid() -> i32;
            }
            setsid();
            Ok(())
        });
    }
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

fn daemon_healthy(socket: &Path) -> bool {
    remote_expr(socket, "1+1").as_deref() == Some("2")
}

/// Evaluate a vimscript expression on the daemon via `--remote-expr`, returning
/// the trimmed stdout, or `None` if the daemon is unreachable.
fn remote_expr(socket: &Path, expr: &str) -> Option<String> {
    let output = Command::new("nvim")
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

/// Shell command a sidebar pane runs to become the nvim UI: it re-executes this
/// binary in `sidebar` mode (see `sidebar_cmd`).
pub fn sidebar_shell_cmd(_workspace: &str) -> String {
    let path = std::env::current_exe()
        .ok()
        .and_then(|path| path.into_os_string().into_string().ok())
        .unwrap_or_else(|| "herdr-nvim".to_owned());
    let path = if path.contains(' ') {
        format!("'{}'", path.replace('\'', "'\\''"))
    } else {
        path
    };
    format!("exec {path} sidebar")
}

/// Runs inside the sidebar pane: ensure the workspace daemon is up, then replace
/// this process with `nvim --remote-ui` attached to it.
pub fn sidebar_cmd() -> Result<()> {
    use std::os::unix::process::CommandExt;

    let workspace = env::var("HERDR_WORKSPACE_ID").context("HERDR_WORKSPACE_ID is not set")?;
    let plugin_root = plugin_root()?;
    let config = crate::config::load();
    let socket = ensure_daemon(&workspace, &plugin_root, &config)?;

    let error = Command::new("nvim")
        .arg("--server")
        .arg(&socket)
        .arg("--remote-ui")
        .exec();
    Err(error).context("failed to exec nvim --remote-ui")
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

/// Garbage-collect daemons whose workspace no longer exists: quit the daemon and
/// remove its socket and state file.
pub fn gc_cmd() -> Result<()> {
    let mut herdr = CliHerdr;
    gc(&mut herdr)
}

fn gc(h: &mut dyn Herdr) -> Result<()> {
    let dir = socket_dir();
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(err)
                .with_context(|| format!("failed to read runtime directory {}", dir.display()))
        }
    };

    let workspaces = h.list_workspaces()?;
    for entry in entries {
        let path = entry
            .with_context(|| format!("failed to read entry in {}", dir.display()))?
            .path();
        if path.extension().and_then(OsStr::to_str) != Some("sock") {
            continue;
        }
        let Some(workspace) = path.file_stem().and_then(OsStr::to_str) else {
            continue;
        };
        if workspaces.iter().any(|known| known == workspace) {
            continue;
        }

        // Orphaned: ask the daemon (if any) to quit, then unlink socket + state.
        let _ = send_quit(&path);
        remove_socket(&path)?;
        state::remove(workspace)?;
    }
    Ok(())
}

fn send_quit(socket: &Path) -> Result<()> {
    Command::new("nvim")
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

fn remove_socket(socket: &Path) -> Result<()> {
    match fs::remove_file(socket) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(err).with_context(|| format!("failed to remove socket {}", socket.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        ffi::OsString,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex, MutexGuard,
        },
    };

    use super::*;
    use crate::herdr::MockHerdr;

    static RUNTIME_DIR_LOCK: Mutex<()> = Mutex::new(());
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
        Command::new("which")
            .arg("nvim")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// Best-effort daemon shutdown so no stray `nvim --headless` survives a test.
    fn stop_daemon(socket: &Path) {
        let _ = send_quit(socket);
        for _ in 0..50 {
            if remote_expr(socket, "1+1").is_none() {
                break;
            }
            sleep(Duration::from_millis(100));
        }
        let _ = remove_socket(socket);
    }

    #[test]
    fn socket_path_respects_runtime_dir_override() {
        let guard = RuntimeEnvGuard::new();
        assert_eq!(socket_path("wsX"), guard.dir.join("wsX.sock"));
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

        let socket = ensure_daemon("wD", &plugin_root, &config).expect("first ensure_daemon");
        assert!(socket.exists(), "socket file should exist after spawn");

        let pid1 = remote_expr(&socket, "getpid()").expect("daemon should report a pid");
        assert!(!pid1.is_empty());

        let socket_again =
            ensure_daemon("wD", &plugin_root, &config).expect("second ensure_daemon");
        assert_eq!(socket, socket_again);

        let pid2 = remote_expr(&socket, "getpid()").expect("daemon should still report a pid");
        assert_eq!(
            pid1, pid2,
            "second ensure_daemon must reuse the daemon, not spawn a new one"
        );

        stop_daemon(&socket);
    }

    #[test]
    fn gc_removes_orphan_sockets_and_keeps_known_workspaces() {
        let guard = RuntimeEnvGuard::new();

        // Isolate the state dir too, since gc removes orphan state files.
        let state_lock = state::STATE_DIR_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let old_state = env::var_os("HERDR_NVIM_STATE_DIR");
        env::set_var("HERDR_NVIM_STATE_DIR", guard.dir.join("state"));

        // Two dead socket files; only "wsKeep" is still a live workspace.
        fs::write(socket_path("wsKeep"), b"").unwrap();
        fs::write(socket_path("wsOrphan"), b"").unwrap();

        let mut herdr = MockHerdr {
            list_workspaces_results: VecDeque::from([Ok(vec!["wsKeep".to_owned()])]),
            ..Default::default()
        };
        gc(&mut herdr).unwrap();

        assert!(socket_path("wsKeep").exists(), "known workspace kept");
        assert!(
            !socket_path("wsOrphan").exists(),
            "orphan workspace socket removed"
        );

        match &old_state {
            Some(value) => env::set_var("HERDR_NVIM_STATE_DIR", value),
            None => env::remove_var("HERDR_NVIM_STATE_DIR"),
        }
        drop(state_lock);
    }
}
