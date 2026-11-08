//! Remote editor operations. The window stays local; Neovim, its socket and
//! supervisor run on the tab's machine. No remote Rust/Python installation is needed.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{cli, doc, sidebar};
use crate::config::{self, Sidebar};

const WORKER: &str = include_str!("../../lua/herdr-nvim/remote.lua");

pub(crate) fn machine(id: &str) -> Option<&str> {
    id.split_once('/').map(|(machine, _)| machine)
}

fn raw<'a>(machine: &str, id: &'a str) -> Result<&'a str> {
    let (prefix, id) = id
        .split_once('/')
        .context("remote IDs need a machine prefix")?;
    if prefix != machine || id.is_empty() || id.contains('/') || machine.is_empty() {
        bail!("ID belongs to a different machine or is malformed");
    }
    Ok(id)
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Serialize)]
struct Request {
    space: String,
    tab: String,
    key: String,
    nvim: String,
    env: BTreeMap<String, String>,
    signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    col: Option<u32>,
}

impl Request {
    fn new(ctx: &cli::AppCtx, machine: &str, settings: &Sidebar) -> Result<Self> {
        let space = raw(machine, &ctx.space)?.to_owned();
        let tab = raw(machine, &ctx.tab)?.to_owned();
        let terminal = ctx
            .terminal
            .as_deref()
            .map(|t| raw(machine, t).map(str::to_owned))
            .transpose()?;
        let overrides = settings.env_override();
        if overrides.len() != settings.nvim_env.len() {
            bail!("invalid nvim_env entry for remote machine {machine}");
        }
        let env: BTreeMap<_, _> = overrides
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect();
        let signature = digest(&serde_json::to_vec(
            &json!({"nvim": settings.nvim_bin, "env": env, "worker": digest(WORKER.as_bytes())}),
        )?);
        Ok(Self {
            key: digest(format!("{space}:{tab}").as_bytes())[..20].to_owned(),
            space,
            tab,
            terminal,
            env,
            signature,
            nvim: settings.nvim_bin.clone(),
            cwd: ctx.cwd.clone(),
            file: None,
            line: None,
            col: None,
        })
    }
}

struct Session {
    machine: String,
    request: Request,
    settings: Sidebar,
    script: String,
    _lock: File,
}

impl Session {
    fn new(ctx: &cli::AppCtx) -> Result<Self> {
        let machine = machine(&ctx.tab).context("not a remote tab")?.to_owned();
        let config = config::load();
        let settings = config.remote_sidebar(&machine);
        let request = Request::new(ctx, &machine, &settings)?;
        let _lock = lock(&format!("{machine}:{}:{}", request.space, request.tab))?;
        let mut session = Self {
            machine,
            request,
            settings,
            script: String::new(),
            _lock,
        };
        session.script = session.deploy()?;
        Ok(session)
    }

    fn qualify(&self, id: &str) -> String {
        format!("{}/{id}", self.machine)
    }

    fn execute(&self, args: &[String]) -> Result<String> {
        let mut command = Command::new(super::rpc::hc_bin());
        command.args([
            "exec",
            "--on",
            &self.machine,
            "--timeout",
            "20",
            "--",
            "env",
        ]);
        for (key, value) in &self.request.env {
            command.arg(format!("{key}={value}"));
        }
        let output = command
            .args(args)
            .stdin(Stdio::null())
            .output()
            .context("cannot run hc exec")?;
        if !output.status.success() {
            bail!(
                "remote Neovim on {}: {}",
                self.machine,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        String::from_utf8(output.stdout).context("remote helper returned invalid UTF-8")
    }

    fn lua(&self, code: &str) -> Result<Value> {
        let output = self.execute(&[
            self.request.nvim.clone(),
            "--clean".into(),
            "--headless".into(),
            "-c".into(),
            format!("lua {code}"),
            "-c".into(),
            "qa".into(),
        ])?;
        serde_json::from_str(&output).context("remote deployment returned invalid JSON")
    }

    fn deploy(&self) -> Result<String> {
        let sha = digest(WORKER.as_bytes());
        let code = format!(
            "local u=vim.uv; assert(u.os_uname().sysname~='Windows_NT','Remote Neovim requires a POSIX host'); local r=u.os_tmpdir()..'/herdr-nvim-remote-'..u.getuid(); local st=u.fs_lstat(r);if not st then u.fs_mkdir(r,448);st=u.fs_lstat(r) end;assert(st and st.type=='directory' and st.uid==u.getuid(),'Unsafe remote runtime directory');assert(u.fs_chmod(r,448)); local p=r..'/worker-{sha}.lua'; local s=nil; if not u.fs_stat(p) then s=r..'/.upload-'..u.os_getpid()..'-'..u.hrtime(); local f=assert(io.open(s,'wb'));f:close() end; io.stdout:write(vim.json.encode({{worker=p,stage=s}})..'\\n')"
        );
        let deployed = self.lua(&code)?;
        let worker = deployed["worker"]
            .as_str()
            .context("missing remote worker path")?
            .to_owned();
        if let Some(stage) = deployed.get("stage").and_then(Value::as_str) {
            // Keep each request below restrictive machine transports' line limits.
            for chunk in WORKER.as_bytes().chunks(768) {
                let text = std::str::from_utf8(chunk)?;
                self.lua(&format!("local f=assert(io.open({},'ab'));f:write({});f:close();io.stdout:write('true\\n')", serde_json::to_string(stage)?, serde_json::to_string(text)?))?;
            }
            self.lua(&format!("local f=assert(io.open({},'rb'));local d=f:read('*a');f:close();assert(vim.fn.sha256(d)=='{sha}','Incomplete helper upload');assert(vim.uv.fs_rename({},{}));io.stdout:write('true\\n')", serde_json::to_string(stage)?, serde_json::to_string(stage)?, serde_json::to_string(&worker)?))?;
        }
        Ok(worker)
    }

    fn call(&self, operation: &str) -> Result<Value> {
        let text = self.execute(&[
            self.request.nvim.clone(),
            "--clean".into(),
            "-l".into(),
            self.script.clone(),
            operation.into(),
            serde_json::to_string(&self.request)?,
        ])?;
        serde_json::from_str(&text).context("remote Neovim returned invalid JSON")
    }

    fn info(&self) -> Result<(Value, doc::Doc)> {
        let info = self.call("info")?;
        let doc = doc::parse(&info["doc"]).context("remote layout document is unavailable")?;
        if doc::tab(&doc, &self.request.tab).map(|(s, _)| s.id.as_str())
            != Some(self.request.space.as_str())
        {
            bail!("remote tab does not belong to the requested space");
        }
        Ok((info, doc))
    }

    fn screens(&self, info: &Value) -> Result<Vec<String>> {
        let screens: Vec<_> = info["terminals"]
            .as_array()
            .context("missing remote terminals")?
            .iter()
            .filter(|t| {
                t["state"] == "live"
                    && t.pointer("/labels/herdr-nvim.remote-role") == Some(&json!("ui"))
            })
            .filter_map(|t| t["address"].as_str().map(str::to_owned))
            .collect();
        if screens.len() > 1 {
            bail!("the remote tab has multiple Neovim UI clients; close the extra clients first");
        }
        Ok(screens)
    }

    fn hide(&self, info: &Value, doc: &doc::Doc) -> Result<Value> {
        for screen in self.screens(info)? {
            if doc::pane_of(doc, &screen).is_some() {
                if doc::terminals_of(doc, &self.request.tab).len() < 2 {
                    bail!("cannot hide the remote tab's only terminal; split another pane first");
                }
                let address = self.qualify(&screen);
                cli::app_do("terminal.park", &json!({"terminal": address}))?;
                return Ok(json!({"closed": address}));
            }
        }
        Ok(json!({}))
    }

    fn show(&self, info: &Value, doc: &doc::Doc, state: &Value, focus: bool) -> Result<Value> {
        let screens = self.screens(info)?;
        let mut terminal = screens.first().cloned();
        if let Some(screen) = &terminal {
            if doc::pane_of(doc, screen).is_none() {
                let own = doc::terminals_of(doc, &self.request.tab);
                if own.len() == 1 {
                    let anchor = self.qualify(&own[0]);
                    cli::app_do("pane.focus", &json!({"terminal": anchor}))?;
                    let pane =
                        self.qualify(&doc::pane_of(doc, &own[0]).context("missing anchor pane")?.0);
                    wait_for(|| {
                        Ok(cli::app_status()?
                            .get("focused_pane")
                            .and_then(Value::as_str)
                            == Some(pane.as_str()))
                    })?;
                    cli::app_do(
                        "terminal.keep",
                        &json!({"terminal": self.qualify(screen), "place": sidebar::direction(self.settings.position)}),
                    )?;
                } else {
                    self.kill(screen)?;
                    terminal = None;
                }
            }
        }
        if terminal.is_none() {
            let root = self
                .qualify(&doc::root_of(doc, &self.request.tab).context("missing remote tab root")?);
            let mut argv = vec!["env".to_owned()];
            argv.extend(
                self.request
                    .env
                    .iter()
                    .map(|(key, value)| format!("{key}={value}")),
            );
            argv.extend([
                state["nvim"]
                    .as_str()
                    .context("missing remote Neovim executable")?
                    .to_owned(),
                "--clean".into(),
                "--server".into(),
                state["socket"]
                    .as_str()
                    .context("missing remote Neovim socket")?
                    .to_owned(),
                "--remote-ui".into(),
            ]);
            let made = cli::app_do(
                "pane.split",
                &json!({"pane": root, "direction": sidebar::direction(self.settings.position), "argv": argv}),
            )?;
            let address = made["terminal"]
                .as_str()
                .context("remote split returned no terminal")?;
            let id = raw(&self.machine, address)?.to_owned();
            let output = Command::new(super::rpc::hc_bin())
                .args([
                    "label",
                    address,
                    &format!("herdr-nvim.sidebar={}", self.request.tab),
                    &format!("herdr-nvim.remote={}", self.request.key),
                    "herdr-nvim.remote-role=ui",
                ])
                .stdin(Stdio::null())
                .output()?;
            if !output.status.success() {
                bail!("could not label the remote Neovim UI client");
            }
            terminal = Some(id);
        }
        let address = self.qualify(&terminal.context("no remote UI client")?);
        wait_for(|| {
            let listing = cli::app_listing()?;
            Ok(listing["spaces"]
                .as_array()
                .into_iter()
                .flatten()
                .flat_map(|s| s["tabs"].as_array().into_iter().flatten())
                .flat_map(|t| t["terminals"].as_array().into_iter().flatten())
                .any(|t| t["terminal"].as_str() == Some(address.as_str())))
        })?;
        if focus {
            cli::app_do("pane.focus", &json!({"terminal": address}))?;
        }
        let mut result = json!({"terminal": address});
        if let Some(notice) = state.get("notice") {
            result["notice"] = notice.clone();
        }
        Ok(result)
    }

    fn kill(&self, terminal: &str) -> Result<()> {
        let output = Command::new(super::rpc::hc_bin())
            .args(["kill", &self.qualify(terminal)])
            .stdin(Stdio::null())
            .output()?;
        if !output.status.success() {
            bail!("could not end the parked remote UI client");
        }
        Ok(())
    }
}

fn lock(key: &str) -> Result<File> {
    let root = crate::state::state_dir().join("remote-locks");
    fs::create_dir_all(&root)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(digest(key.as_bytes())))?;
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < until => {
                thread::sleep(Duration::from_millis(50))
            }
            Err(error) => {
                return Err(error).context("another remote editor action is running for this tab")
            }
        }
    }
}

fn wait_for(mut check: impl FnMut() -> Result<bool>) -> Result<()> {
    let until = Instant::now() + Duration::from_secs(3);
    while !check()? {
        if Instant::now() >= until {
            bail!("the app did not finish changing remote pane focus");
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

/// Handle remote editor commands before local-only host code touches paths/core IDs.
pub(crate) fn dispatch(mode: &str, args: &[String]) -> Result<bool> {
    if !matches!(
        mode,
        "toggle"
            | "open"
            | "open-link"
            | "app-current"
            | "app-find"
            | "pick-file"
            | "fs"
            | "app-preview"
            | "app-remote-stop"
    ) {
        return Ok(false);
    }
    if mode == "app-find" && cli::flag(args, "--ctx")?.is_none() {
        return Ok(false);
    }
    let ctx = if let Some(tab) = cli::flag(args, "--tab")? {
        if machine(tab).is_none() {
            return Ok(false);
        }
        let listing = cli::app_listing()?;
        let space = cli::flag(args, "--space")?
            .map(str::to_owned)
            .or_else(|| {
                listing["spaces"].as_array()?.iter().find(|s| {
                    s["tabs"]
                        .as_array()
                        .is_some_and(|tabs| tabs.iter().any(|t| t["id"] == tab))
                })?["id"]
                    .as_str()
                    .map(str::to_owned)
            })
            .context("no remote tab in the app")?;
        cli::AppCtx {
            space,
            tab: tab.to_owned(),
            terminal: None,
            cwd: None,
        }
    } else {
        cli::read_ctx(args)?
    };
    if machine(&ctx.space).is_none() && machine(&ctx.tab).is_none() {
        if mode == "app-remote-stop" {
            bail!("app-remote-stop requires a remote tab");
        }
        return Ok(false);
    }
    if !matches!(mode, "toggle" | "open" | "app-current" | "app-remote-stop") {
        bail!("{mode} is not available for remote tabs yet; use the remote Neovim's own explorer and plugins");
    }
    if cli::flag(args, "--preview")?.is_some() {
        bail!("rendered previews are not available for remote tabs yet");
    }
    let mut session = Session::new(&ctx)?;
    let (info, doc) = session.info()?;
    let result = match mode {
        "app-current" => {
            json!({"file": info.get("file").cloned().unwrap_or(Value::Null), "watch": null, "tab_root": info["tab_root"]})
        }
        "app-remote-stop" => {
            session.call("stop")?;
            json!({"stopped": true})
        }
        "toggle"
            if matches!(sidebar::want_from_args(args)?, sidebar::Want::Close)
                || (matches!(sidebar::want_from_args(args)?, sidebar::Want::Flip)
                    && session
                        .screens(&info)?
                        .iter()
                        .any(|t| doc::pane_of(&doc, t).is_some())) =>
        {
            session.hide(&info, &doc)?
        }
        _ => {
            if mode == "open" {
                let text = cli::positional(args, &["--tab", "--space", "--ctx"])
                    .context("open needs a remote file path")?;
                let (file, line, col) = super::commands::split_location(text);
                session.request.file = Some(file);
                session.request.line = line;
                session.request.col = col;
            }
            let state = session.call("open")?;
            session.show(&info, &doc, &state, !args.iter().any(|a| a == "--no-focus"))?
        }
    };
    println!("{result}");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_ids_are_machine_scoped_before_any_side_effect() {
        let ctx = cli::AppCtx {
            space: "box/s1".into(),
            tab: "box/t1".into(),
            terminal: Some("box/t7".into()),
            cwd: None,
        };
        let request = Request::new(&ctx, "box", &Sidebar::default()).unwrap();
        assert_eq!(
            (
                &request.space[..],
                &request.tab[..],
                request.terminal.as_deref()
            ),
            ("s1", "t1", Some("t7"))
        );
        for wrong in ["t7", "other/t7", "box/", "box/other/t7"] {
            assert!(raw("box", wrong).is_err());
        }
        let wrong = cli::AppCtx {
            tab: "other/t1".into(),
            ..ctx
        };
        assert!(Request::new(&wrong, "box", &Sidebar::default()).is_err());
    }
    #[test]
    fn absent_paths_are_omitted_instead_of_lua_null_values() {
        let ctx = cli::AppCtx {
            space: "box/s1".into(),
            tab: "box/t1".into(),
            ..Default::default()
        };
        let request =
            serde_json::to_value(Request::new(&ctx, "box", &Sidebar::default()).unwrap()).unwrap();
        assert!(request.get("cwd").is_none());
        assert!(request.get("terminal").is_none());
        assert!(request.get("file").is_none());
    }
    #[test]
    fn malformed_remote_environment_is_rejected_without_echoing_values() {
        let ctx = cli::AppCtx {
            space: "box/s1".into(),
            tab: "box/t1".into(),
            ..Default::default()
        };
        let settings = Sidebar {
            nvim_env: vec!["BAD KEY=private-value".into()],
            ..Default::default()
        };
        let error = Request::new(&ctx, "box", &settings)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("invalid nvim_env"));
        assert!(!error.contains("private-value"));
    }

    #[test]
    fn worker_version_and_machine_settings_are_part_of_the_signature() {
        let ctx = cli::AppCtx {
            space: "box/s1".into(),
            tab: "box/t1".into(),
            ..Default::default()
        };
        let a = Request::new(&ctx, "box", &Sidebar::default()).unwrap();
        let settings = Sidebar {
            nvim_env: vec!["NVIM_APPNAME=work".into()],
            ..Default::default()
        };
        let b = Request::new(&ctx, "box", &settings).unwrap();
        assert_eq!(a.key, b.key);
        assert_ne!(a.signature, b.signature);
        assert_eq!(b.env["NVIM_APPNAME"], "work");
        assert!(WORKER.is_ascii());
    }
}
