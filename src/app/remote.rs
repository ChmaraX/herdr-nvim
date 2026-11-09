//! Remote editor panes: local window commands, remote Neovim and supervision.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::{cli, doc, node, sidebar};
use crate::config::{self, Sidebar, SidebarPosition};

const WORKER: &str = include_str!("../../lua/herdr-nvim/remote.lua");
const BOOTSTRAP: &str = r#"
local u = vim.uv
assert(u.os_uname().sysname ~= 'Windows_NT', 'Remote Neovim requires a POSIX host')
local root = u.os_tmpdir() .. '/herdr-nvim-remote-' .. u.getuid()
local stat = u.fs_lstat(root)
if not stat then u.fs_mkdir(root, 448); stat = u.fs_lstat(root) end
assert(stat and stat.type == 'directory' and stat.uid == u.getuid(), 'Unsafe remote runtime directory')
assert(u.fs_chmod(root, 448))
local worker = root .. '/worker-' .. sha .. '.lua'
local stage
if not u.fs_stat(worker) then
  stage = root .. '/.upload-' .. u.os_getpid() .. '-' .. u.hrtime()
  assert(io.open(stage, 'wb')):close()
end
io.stdout:write(vim.json.encode({worker=worker,stage=stage}) .. '\n')
"#;

pub(crate) fn machine(id: &str) -> Option<&str> {
    id.split_once('/').map(|(machine, _)| machine)
}

pub(crate) fn is_remote(ctx: &cli::AppCtx) -> bool {
    machine(&ctx.space).is_some() || machine(&ctx.tab).is_some()
}

pub(crate) fn require_local(ctx: &cli::AppCtx) -> Result<()> {
    if is_remote(ctx) {
        bail!(
            "this command is not available for remote tabs; use Neovim's own explorer and plugins"
        );
    }
    Ok(())
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
        Ok(Self {
            key: digest(format!("{space}:{tab}").as_bytes())[..20].to_owned(),
            space,
            tab,
            terminal,
            cwd: ctx.cwd.clone(),
            nvim: settings.nvim_bin.clone(),
            env: overrides
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect(),
            file: None,
            line: None,
            col: None,
        })
    }
}

#[derive(Deserialize)]
struct Info {
    doc: Value,
    terminals: Vec<RemoteTerminal>,
}

#[derive(Deserialize)]
struct RemoteTerminal {
    address: String,
    state: String,
    labels: BTreeMap<String, String>,
}

struct Session {
    machine: String,
    request: Request,
    position: SidebarPosition,
    script: String,
    _lock: File,
}

impl Session {
    fn new(ctx: &cli::AppCtx) -> Result<Self> {
        let machine = machine(&ctx.tab).context("not a remote tab")?.to_owned();
        let settings = config::load().remote_sidebar(&machine);
        let request = Request::new(ctx, &machine, &settings)?;
        let lock = lock(&format!("{machine}:{}:{}", request.space, request.tab))?;
        let script = deploy(&machine, &request)?;
        Ok(Self {
            machine,
            request,
            position: settings.position,
            script,
            _lock: lock,
        })
    }

    fn qualify(&self, id: &str) -> String {
        format!("{}/{id}", self.machine)
    }

    fn call(&self, operation: &str) -> Result<Value> {
        let text = execute(
            &self.machine,
            &self.request,
            &[
                self.request.nvim.clone(),
                "--clean".into(),
                "-l".into(),
                self.script.clone(),
                operation.into(),
                serde_json::to_string(&self.request)?,
            ],
        )?;
        serde_json::from_str(&text).context("remote Neovim returned invalid JSON")
    }

    fn info(&self) -> Result<(doc::Doc, Option<String>)> {
        let info: Info = serde_json::from_value(self.call("info")?)?;
        let doc = doc::parse(&info.doc).context("remote layout document is unavailable")?;
        if doc::tab(&doc, &self.request.tab).map(|(s, _)| s.id.as_str())
            != Some(self.request.space.as_str())
        {
            bail!("remote tab does not belong to the requested space");
        }
        let mut screens = info.terminals.iter().filter(|t| {
            t.state == "live"
                && t.labels
                    .get("herdr-nvim.remote-role")
                    .is_some_and(|role| role == "ui")
        });
        let screen = screens.next().map(|t| t.address.clone());
        if screens.next().is_some() {
            bail!("the remote tab has multiple Neovim UI clients; close the extra clients first");
        }
        Ok((doc, screen))
    }

    fn hide(&self, doc: &doc::Doc, screen: Option<&str>) -> Result<Value> {
        let Some(screen) = screen.filter(|screen| doc::pane_of(doc, screen).is_some()) else {
            return Ok(json!({}));
        };
        let tab = doc::tab(doc, &self.request.tab)
            .context("missing remote tab")?
            .1;
        let panes = node::walk(&tab.root)
            .filter(|n| {
                node::terminal(n).is_some()
                    || n.pointer("/content/mod").and_then(Value::as_str).is_some()
            })
            .count();
        if panes < 2 {
            bail!("cannot hide the remote tab's only pane; split another pane first");
        }
        let address = self.qualify(screen);
        cli::app_do("terminal.park", &json!({"terminal": address}))?;
        wait_for(|| Ok(!cli::app_listing()?.contains_terminal(&address)))?;
        Ok(json!({"closed": address}))
    }

    fn show(
        &self,
        doc: &doc::Doc,
        screen: Option<&str>,
        state: &Value,
        focus: bool,
    ) -> Result<Value> {
        let mut terminal = screen.map(str::to_owned);
        if let Some(screen) = &terminal {
            if doc::pane_of(doc, screen).is_none() {
                let own = doc::terminals_of(doc, &self.request.tab);
                if own.len() == 1 {
                    cli::app_do("pane.focus", &json!({"terminal": self.qualify(&own[0])}))?;
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
                        &json!({"terminal": self.qualify(screen), "place": sidebar::direction(self.position)}),
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
                &json!({"pane": root, "direction": sidebar::direction(self.position), "argv": argv}),
            )?;
            let address = made["terminal"]
                .as_str()
                .context("remote split returned no terminal")?;
            let id = raw(&self.machine, address)?.to_owned();
            if let Err(error) = hc(&[
                "label".into(),
                address.into(),
                format!("herdr-nvim.sidebar={}", self.request.tab),
                format!("herdr-nvim.remote={}", self.request.key),
                "herdr-nvim.remote-role=ui".into(),
            ]) {
                if let Err(cleanup) = self.kill(&id) {
                    eprintln!("herdr-nvim: UI cleanup failed: {cleanup}");
                }
                return Err(error).context("could not label the remote Neovim UI client");
            }
            terminal = Some(id);
        }
        let address = self.qualify(&terminal.context("no remote UI client")?);
        wait_for(|| Ok(cli::app_listing()?.contains_terminal(&address)))?;
        if focus {
            cli::app_do("pane.focus", &json!({"terminal": address}))?;
        }
        Ok(json!({"terminal": address}))
    }

    fn kill(&self, terminal: &str) -> Result<()> {
        hc(&["kill".into(), self.qualify(terminal)]).map(|_| ())
    }
}

fn hc(args: &[String]) -> Result<String> {
    let output = Command::new(super::rpc::hc_bin())
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("cannot run hc")?;
    if !output.status.success() {
        bail!("hc: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    String::from_utf8(output.stdout).context("hc returned invalid UTF-8")
}

fn execute(machine: &str, request: &Request, args: &[String]) -> Result<String> {
    let mut command: Vec<String> = ["exec", "--on", machine, "--timeout", "20", "--", "env"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    command.extend(
        request
            .env
            .iter()
            .map(|(key, value)| format!("{key}={value}")),
    );
    command.extend_from_slice(args);
    hc(&command).with_context(|| format!("remote Neovim on {machine}"))
}

fn lua(machine: &str, request: &Request, code: &str) -> Result<Value> {
    let text = execute(
        machine,
        request,
        &[
            request.nvim.clone(),
            "--clean".into(),
            "--headless".into(),
            "-c".into(),
            format!("lua {code}"),
            "-c".into(),
            "qa".into(),
        ],
    )?;
    serde_json::from_str(&text).context("remote deployment returned invalid JSON")
}

fn deploy(machine: &str, request: &Request) -> Result<String> {
    let sha = digest(WORKER.as_bytes());
    let deployed = lua(
        machine,
        request,
        &format!("local sha={};{BOOTSTRAP}", serde_json::to_string(&sha)?),
    )?;
    let worker = deployed["worker"]
        .as_str()
        .context("missing remote worker path")?
        .to_owned();
    if let Some(stage) = deployed.get("stage").and_then(Value::as_str) {
        // Some machine transports have restrictive request-line limits.
        for text in upload_chunks(WORKER) {
            lua(machine, request, &format!("local f=assert(io.open({},'ab'));f:write({});f:close();io.stdout:write('true\\n')", serde_json::to_string(stage)?, serde_json::to_string(text)?))?;
        }
        lua(machine, request, &format!("local f=assert(io.open({},'rb'));local d=f:read('*a');f:close();assert(vim.fn.sha256(d)=='{sha}','Incomplete helper upload');assert(vim.uv.fs_rename({},{}));io.stdout:write('true\\n')", serde_json::to_string(stage)?, serde_json::to_string(stage)?, serde_json::to_string(&worker)?))?;
    }
    Ok(worker)
}

fn upload_chunks(mut text: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    while !text.is_empty() {
        let mut end = text.len().min(768);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(&text[..end]);
        text = &text[end..];
    }
    chunks
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
            bail!("the app did not finish changing the remote pane");
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

pub(crate) fn open(
    ctx: &cli::AppCtx,
    file: String,
    line: Option<u32>,
    col: Option<u32>,
    focus: bool,
) -> Result<Value> {
    let mut session = Session::new(ctx)?;
    let (doc, screen) = session.info()?;
    session.request.file = Some(file);
    session.request.line = line;
    session.request.col = col;
    let state = session.call("open")?;
    session.show(&doc, screen.as_deref(), &state, focus)
}

pub(crate) fn toggle(ctx: &cli::AppCtx, want: sidebar::Want) -> Result<Value> {
    let session = Session::new(ctx)?;
    let (doc, screen) = session.info()?;
    let shown = screen
        .as_deref()
        .is_some_and(|screen| doc::pane_of(&doc, screen).is_some());
    if want == sidebar::Want::Close || (want == sidebar::Want::Flip && shown) {
        session.hide(&doc, screen.as_deref())
    } else {
        session.show(&doc, screen.as_deref(), &session.call("open")?, true)
    }
}

pub(crate) fn current(ctx: &cli::AppCtx) -> Result<Value> {
    Session::new(ctx)?.call("current")
}

pub(crate) fn stop_cmd() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let session = Session::new(&cli::target_ctx(&args)?)?;
    session.call("stop")?;
    println!("{}", json!({"stopped": true}));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> cli::AppCtx {
        cli::AppCtx {
            space: "box/s1".into(),
            tab: "box/t1".into(),
            ..Default::default()
        }
    }

    #[test]
    fn remote_ids_are_machine_scoped_before_any_side_effect() {
        let context = cli::AppCtx {
            terminal: Some("box/t7".into()),
            ..ctx()
        };
        let request = Request::new(&context, "box", &Sidebar::default()).unwrap();
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
        assert!(Request::new(
            &cli::AppCtx {
                tab: "other/t1".into(),
                ..context
            },
            "box",
            &Sidebar::default()
        )
        .is_err());
    }

    #[test]
    fn absent_paths_are_omitted_instead_of_lua_null_values() {
        let request =
            serde_json::to_value(Request::new(&ctx(), "box", &Sidebar::default()).unwrap())
                .unwrap();
        for key in ["cwd", "terminal", "file", "line", "col"] {
            assert!(request.get(key).is_none());
        }
    }

    #[test]
    fn malformed_remote_environment_is_rejected_without_echoing_values() {
        let settings = Sidebar {
            nvim_env: vec!["BAD KEY=private-value".into()],
            ..Default::default()
        };
        let error = Request::new(&ctx(), "box", &settings)
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("invalid nvim_env"));
        assert!(!error.contains("private-value"));
    }

    #[test]
    fn remote_environment_overrides_keep_the_last_value() {
        let settings = Sidebar {
            nvim_env: vec!["NVIM_APPNAME=old".into(), "NVIM_APPNAME=work".into()],
            ..Default::default()
        };
        assert_eq!(
            Request::new(&ctx(), "box", &settings).unwrap().env["NVIM_APPNAME"],
            "work"
        );
    }

    #[test]
    fn uploads_preserve_unicode_without_exceeding_the_byte_limit() {
        let source = format!("{}雪😀{}", "a".repeat(767), "雪".repeat(500));
        let chunks = upload_chunks(&source);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 768));
        assert_eq!(chunks.concat(), source);
    }
}
