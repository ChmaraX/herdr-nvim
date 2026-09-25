//! Shared harness for the CLI tests: a sandbox that points every path the
//! binary touches (runtime dir, state dir, config home) at a throwaway temp
//! dir, a scriptable fake `herdr` on PATH (Unix), real nvim daemons
//! registered the way `ensure_daemon` registers them, and a kept artifact
//! log under `target/test-artifacts/<test>/`.

#![allow(dead_code)] // each test binary uses a different subset

use std::{
    env,
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

pub fn nvim_available() -> bool {
    Command::new("nvim")
        .arg("--version")
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub struct Sandbox {
    pub dir: PathBuf,
}

impl Sandbox {
    pub fn new(name: &str) -> Self {
        let dir = env::temp_dir().join(format!("hn-cli-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("runtime")).unwrap();
        fs::create_dir_all(dir.join("bin")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Answers `herdr api snapshot` from `snapshot.json` when present;
            // otherwise fails like an unreachable herdr.
            let herdr = dir.join("bin").join("herdr");
            fs::write(
                &herdr,
                format!(
                    "#!/bin/sh\nif [ \"$1 $2\" = \"api snapshot\" ] && [ -f '{0}' ]; then cat '{0}'; exit 0; fi\n\
                     echo 'fake herdr: unreachable' >&2\nexit 1\n",
                    dir.join("snapshot.json").display()
                ),
            )
            .unwrap();
            fs::set_permissions(&herdr, fs::Permissions::from_mode(0o755)).unwrap();
        }
        Self { dir }
    }

    pub fn runtime(&self) -> PathBuf {
        self.dir.join("runtime")
    }

    /// Make the fake herdr report exactly these `(tab id, workspace label,
    /// tab label)` tabs.
    pub fn herdr_tabs(&self, tabs: &[(&str, &str, &str)]) {
        let workspace = |id: &str| id.split(':').next().unwrap().to_owned();
        let snapshot = serde_json::json!({"result": {"snapshot": {
            "tabs": tabs.iter().map(|(id, _, label)| serde_json::json!({
                "tab_id": id, "workspace_id": workspace(id), "label": label,
            })).collect::<Vec<_>>(),
            "workspaces": tabs.iter().map(|(id, ws_label, _)| serde_json::json!({
                "workspace_id": workspace(id), "label": ws_label,
            })).collect::<Vec<_>>(),
        }}});
        fs::write(self.dir.join("snapshot.json"), snapshot.to_string()).unwrap();
    }

    pub fn herdr_unreachable(&self) {
        let _ = fs::remove_file(self.dir.join("snapshot.json"));
    }

    /// Run the real binary as herdr would, inside the sandbox.
    pub fn run(&self, args: &[&str], event_json: Option<&str>) -> Output {
        let path = env::join_paths(
            std::iter::once(self.dir.join("bin"))
                .chain(env::split_paths(&env::var_os("PATH").unwrap_or_default())),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_herdr-nvim"));
        // The binary puts `$HERDR_BIN_PATH`'s dir and `~/.local/bin` ahead of
        // PATH (`path::augment_path`); aim both at the sandbox so the fake
        // herdr wins and the user's real herdr is never asked.
        command
            .args(args)
            .env("PATH", path)
            .env("HERDR_BIN_PATH", self.dir.join("bin").join("herdr"))
            .env("HOME", &self.dir)
            .env("HERDR_NVIM_RUNTIME_DIR", self.runtime())
            .env("HERDR_NVIM_STATE_DIR", self.dir.join("state"))
            .env("XDG_CONFIG_HOME", self.dir.join("xdg-config"))
            .env_remove("HERDR_NVIM_CONFIG")
            .env_remove("HERDR_PLUGIN_EVENT_JSON");
        if let Some(json) = event_json {
            command.env("HERDR_PLUGIN_EVENT_JSON", json);
        }
        command.output().expect("run herdr-nvim")
    }

    /// Registry entry of `tab`, as `ensure_daemon` creates it.
    pub fn entry(&self, tab: &str) -> PathBuf {
        #[cfg(not(windows))]
        let ext = "sock";
        #[cfg(windows)]
        let ext = "pipe";
        self.runtime()
            .join(format!("{}.{ext}", tab.replace(':', "_")))
    }

    /// A registry entry nothing answers on (a daemon that died uncleanly).
    pub fn stale_entry(&self, tab: &str) -> PathBuf {
        let entry = self.entry(tab);
        fs::write(&entry, tab).unwrap();
        entry
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A real headless nvim registered for `tab` in the sandbox, spawned the way
/// `ensure_daemon` does it (listen address, identity env), minus the plugin.
/// Killed and reaped on drop.
pub struct Daemon {
    pub tab: String,
    pub entry: PathBuf,
    address: PathBuf,
    child: Child,
}

impl Daemon {
    pub fn spawn(sandbox: &Sandbox, tab: &str) -> Self {
        Self::spawn_with(sandbox, tab, &[])
    }

    /// A daemon that swallows every key, so `qa!` never reaches it: it keeps
    /// answering RPC but cannot be quit.
    pub fn spawn_unquittable(sandbox: &Sandbox, tab: &str) -> Self {
        Self::spawn_with(
            sandbox,
            tab,
            &["--cmd", "lua vim.on_key(function() return '' end)"],
        )
    }

    fn spawn_with(sandbox: &Sandbox, tab: &str, extra: &[&str]) -> Self {
        let entry = sandbox.entry(tab);
        #[cfg(not(windows))]
        let address = entry.clone();
        #[cfg(windows)]
        let address = PathBuf::from(format!(r"\\.\pipe\herdr-nvim-{}", tab.replace(':', "_")));
        let child = Command::new("nvim")
            .args(["--headless", "--clean", "--listen"])
            .arg(&address)
            .args(extra)
            .env("HERDR_TAB_ID", tab)
            .env("HERDR_WORKSPACE_ID", tab.split(':').next().unwrap())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn nvim");
        #[cfg(windows)]
        fs::write(&entry, tab).unwrap();
        let daemon = Self {
            tab: tab.to_owned(),
            entry,
            address,
            child,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while daemon.eval("1+1").as_deref() != Some("2") {
            assert!(Instant::now() < deadline, "{tab} daemon never came up");
            sleep(Duration::from_millis(50));
        }
        fs::create_dir_all(sandbox.dir.join("state")).unwrap();
        fs::write(daemon.state_file(sandbox), b"{}").unwrap();
        daemon
    }

    pub fn state_file(&self, sandbox: &Sandbox) -> PathBuf {
        sandbox
            .dir
            .join("state")
            .join(format!("{}.json", self.tab.replace(':', "_")))
    }

    pub fn eval(&self, expr: &str) -> Option<String> {
        let output = Command::new("nvim")
            .args(["--headless", "--clean", "--server"])
            .arg(&self.address)
            .args(["--remote-expr", expr])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    pub fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Wait (briefly) for the process to exit; `true` once it has.
    pub fn exited(&mut self) -> bool {
        for _ in 0..40 {
            if !self.running() {
                return true;
            }
            sleep(Duration::from_millis(50));
        }
        false
    }

    pub fn assert_alive(&mut self, sandbox: &Sandbox) {
        let tab = self.tab.clone();
        assert!(self.running(), "{tab} daemon process died");
        assert_eq!(
            self.eval("1+1").as_deref(),
            Some("2"),
            "{tab} not answering"
        );
        assert!(self.entry.exists(), "{tab} registry entry removed");
        assert!(
            self.state_file(sandbox).exists(),
            "{tab} state file removed"
        );
    }

    pub fn assert_gone(&mut self, sandbox: &Sandbox) {
        let tab = self.tab.clone();
        assert!(self.exited(), "{tab} daemon process still running");
        assert!(!self.entry.exists(), "{tab} registry entry left behind");
        assert!(
            !self.state_file(sandbox).exists(),
            "{tab} state file left behind"
        );
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A human-readable record of an E2E run, rewritten after every step so it
/// survives a failing assertion. Kept at `target/test-artifacts/<test>/log.md`.
pub struct Artifact {
    path: PathBuf,
    text: String,
}

impl Artifact {
    pub fn new(test: &str) -> Self {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-artifacts")
            .join(test);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("log.md");
        println!("artifact: {}", path.display());
        let mut artifact = Self {
            path,
            text: String::new(),
        };
        artifact.note(&format!("# {test}\n"));
        artifact
    }

    pub fn note(&mut self, text: &str) {
        self.text.push_str(text);
        self.text.push('\n');
        fs::write(&self.path, &self.text).unwrap();
    }

    /// Record one binary invocation and what came of it.
    pub fn step(&mut self, title: &str, input: &str, output: &Output) {
        let mut text = String::new();
        let _ = writeln!(text, "## {title}\n");
        if !input.is_empty() {
            let _ = writeln!(text, "input:\n```\n{input}\n```");
        }
        let _ = writeln!(text, "exit: {:?}", output.status.code());
        for (name, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
            let body = String::from_utf8_lossy(bytes);
            if !body.trim().is_empty() {
                let _ = writeln!(text, "{name}:\n```\n{}\n```", body.trim_end());
            }
        }
        self.note(&text);
    }
}
