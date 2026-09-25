//! Shared test fixtures. Tests redirect process-wide env vars (the daemon
//! runtime dir, the state dir, the nvim config home), so every test that
//! touches any of them goes through the one `TestEnv`, serialized by one lock
//! -- there is no lock order to get wrong.

use std::{
    env,
    ffi::OsString,
    fs,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex, MutexGuard,
    },
};

use crate::{
    config::{Config, Sidebar},
    daemon::{ensure_daemon, registry, remote_expr},
    state::{self, TabId},
};

static ENV_LOCK: Mutex<()> = Mutex::new(());
static COUNTER: AtomicUsize = AtomicUsize::new(0);

const VARS: [&str; 3] = [
    "HERDR_NVIM_RUNTIME_DIR",
    "HERDR_NVIM_STATE_DIR",
    "XDG_CONFIG_HOME",
];

/// Points the runtime dir (`<dir>/runtime`, not created up front), the state
/// dir (`<dir>/state`) and an empty nvim config home into a unique temp dir
/// while alive, restoring the prior env and deleting the dir on drop. The
/// config isolation keeps any real user init.lua out of spawned daemons.
pub(crate) struct TestEnv {
    _lock: MutexGuard<'static, ()>,
    old: Vec<Option<OsString>>,
    pub(crate) dir: PathBuf,
}

impl TestEnv {
    pub(crate) fn new() -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let dir = env::temp_dir().join(format!(
            "hn-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let old = VARS.iter().map(env::var_os).collect();
        env::set_var("HERDR_NVIM_RUNTIME_DIR", dir.join("runtime"));
        env::set_var("HERDR_NVIM_STATE_DIR", dir.join("state"));
        env::set_var("XDG_CONFIG_HOME", dir.join("xdg-config"));
        Self {
            _lock: lock,
            old,
            dir,
        }
    }

    pub(crate) fn runtime_dir(&self) -> PathBuf {
        self.dir.join("runtime")
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        for (var, old) in VARS.iter().zip(&self.old) {
            match old {
                Some(value) => env::set_var(var, value),
                None => env::remove_var(var),
            }
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub(crate) fn nvim_available() -> bool {
    #[cfg(not(windows))]
    let which = "which";
    #[cfg(windows)]
    let which = "where.exe";
    Command::new(which)
        .arg("nvim")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Whether the OS still runs `pid`. The test process is the daemons'
/// parent and never reaps them, so on Unix an exited daemon lingers as a
/// zombie -- that counts as dead.
pub(crate) fn process_alive(pid: u32) -> bool {
    #[cfg(not(windows))]
    {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
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

pub(crate) fn kill(pid: u32) {
    #[cfg(not(windows))]
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
    #[cfg(windows)]
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output();
}

/// nvim stops its jobs on exit, but asynchronously; give it a moment.
pub(crate) fn wait_dead(pid: u32) {
    for _ in 0..40 {
        if !process_alive(pid) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// A real daemon spawned for a test tab, with the pid it reported. Killed on
/// drop (after a polite quit), so a failing assertion never leaks a process.
pub(crate) struct TestDaemon {
    pub(crate) tab: TabId,
    pub(crate) socket: PathBuf,
    pub(crate) pid: u32,
}

impl TestDaemon {
    pub(crate) fn spawn(tab: &str, config: &Config) -> Self {
        let plugin_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let socket = ensure_daemon(tab, &plugin_root, config, &plugin_root).expect("ensure_daemon");
        let pid = remote_expr(&socket, "getpid()", &config.sidebar)
            .expect("daemon pid")
            .parse()
            .expect("numeric pid");
        // Give the daemon a sidebar state file so its cleanup is observable.
        let state_file = state::state_path(tab);
        fs::create_dir_all(state_file.parent().unwrap()).unwrap();
        fs::write(&state_file, b"{}").unwrap();
        Self {
            tab: TabId::new(tab),
            socket,
            pid,
        }
    }

    /// Evaluate `expr` on the daemon; panics if it does not answer.
    pub(crate) fn eval(&self, expr: &str) -> String {
        remote_expr(&self.socket, expr, &Sidebar::default())
            .unwrap_or_else(|| panic!("{} daemon did not answer {expr}", self.tab))
    }

    /// Leave a modified, listed buffer in the daemon. `noswapfile`: parallel
    /// tests may point HOME somewhere unwritable, and a failed swap-file
    /// creation would abort the edit.
    pub(crate) fn dirty_a_buffer(&self) {
        self.eval(r#"execute('setlocal noswapfile | call setline(1, "unsaved")')"#);
    }

    pub(crate) fn assert_alive(&self) {
        let tab = &self.tab;
        assert!(process_alive(self.pid), "{tab} daemon process died");
        assert_eq!(self.eval("1+1"), "2", "{tab} daemon stopped answering");
        assert!(
            registry::registry_path(&tab.key()).exists(),
            "{tab} registry entry removed"
        );
        assert!(
            state::state_path(tab.as_str()).exists(),
            "{tab} state file removed"
        );
    }

    pub(crate) fn assert_gone(&self) {
        let tab = &self.tab;
        assert!(
            !process_alive(self.pid),
            "{tab} daemon process still running"
        );
        assert!(
            remote_expr(&self.socket, "1+1", &Sidebar::default()).is_none(),
            "{tab} daemon still answering"
        );
        assert!(
            !registry::registry_path(&tab.key()).exists(),
            "{tab} registry entry left behind"
        );
        assert!(
            !state::state_path(tab.as_str()).exists(),
            "{tab} state file left behind"
        );
    }
}

impl Drop for TestDaemon {
    fn drop(&mut self) {
        if process_alive(self.pid) {
            kill(self.pid);
        }
        #[cfg(not(windows))]
        let _ = state::remove_file_if_exists(&self.socket);
    }
}
