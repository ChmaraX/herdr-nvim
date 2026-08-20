//! `herdr-nvim doctor` — a live self-test that asserts the load-bearing facts
//! this plugin depends on, by exercising the real herdr session and a real
//! headless nvim daemon.
//!
//! Everything that mutates state happens inside a scratch workspace labelled
//! exactly `herdr-nvim-doctor` that this command creates on entry and closes on
//! exit (via `WorkspaceGuard`'s `Drop`, plus an explicit close so cleanup can be
//! verified before returning). The daemon runtime dir and the maneuver state dir
//! are redirected into a unique temp dir so the run never touches real
//! per-workspace artifacts, and any headless nvim it spawns is quit + reaped.
//!
//! Reused, already-tested code paths (`CliHerdr` implementing `Herdr`, and the
//! real `maneuver::toggle`) cover pane splits/moves/tabs and the sidebar toggle.
//! For facts the `Herdr` trait was never scoped to expose — split ratios and
//! pane content — doctor shells out to the `herdr` CLI directly.

use std::{
    env,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread::sleep,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use crate::{
    daemon,
    herdr::{CliHerdr, Dir, Herdr},
    maneuver,
};

const WORKSPACE_LABEL: &str = "herdr-nvim-doctor";

pub fn doctor_cmd() -> Result<()> {
    let with_agent = parse_with_agent(env::args());

    // Isolate all daemon sockets and maneuver state files into a throwaway temp
    // dir so a live run never collides with (or leaves behind) real artifacts.
    let temp = env::temp_dir().join(format!("herdr-nvim-doctor-{}", std::process::id()));
    let runtime_dir = temp.join("runtime");
    let state_dir = temp.join("state");
    let _ = fs::remove_dir_all(&temp);
    env::set_var("HERDR_NVIM_RUNTIME_DIR", &runtime_dir);
    env::set_var("HERDR_NVIM_STATE_DIR", &state_dir);
    // Belt-and-suspenders: even if a check panics or an early `?` fires, this
    // guard quits stray daemons and removes the temp dir on unwind.
    let config = crate::config::load();
    let _temp_guard = TempGuard {
        dir: temp.clone(),
        sidebar: config.sidebar.clone(),
    };

    let mut herdr = CliHerdr;
    let mut ws = WorkspaceGuard::create(WORKSPACE_LABEL)?;
    println!("workspace {} ({}) created", ws.id, ws.label);
    println!();

    let mut fails = 0usize;
    run_checks(&mut herdr, &ws.id, with_agent.as_deref(), &mut fails);

    // Explicit cleanup + verification (so we can report it before returning).
    println!();
    println!("--- cleanup ---");
    ws.close();
    quit_daemons(&runtime_dir, &config.sidebar);

    let leftover = reap_leftover_nvim(&temp);
    println!("leaked headless nvim (temp {}): {leftover}", temp.display());

    match verify_workspace_gone(&ws.id) {
        Ok((gone, list)) => {
            println!("herdr workspace list:\n{}", list.trim());
            println!(
                "workspace {WORKSPACE_LABEL} ({}) still present: {}",
                ws.id, !gone
            );
            if !gone {
                fails += 1;
            }
        }
        Err(error) => {
            println!("WARN could not verify workspace cleanup: {error:#}");
            fails += 1;
        }
    }

    println!();
    if fails > 0 {
        bail!("{fails} doctor check(s) FAILED");
    }
    println!("all doctor checks OK");
    Ok(())
}

fn run_checks(h: &mut CliHerdr, ws: &str, with_agent: Option<&str>, fails: &mut usize) {
    report(fails, "D-F3  full-height-root-split", check_f3(h, ws));
    report(fails, "D-F4/F5 park-roundtrip-ratio", check_f4_f5(h, ws));
    report(
        fails,
        "D-F7  toggle-roundtrip-restores-rects",
        check_f7(h, ws),
    );

    let config = crate::config::load();
    let cwd = env::current_dir().unwrap_or_default();
    match daemon::plugin_root().and_then(|root| daemon::ensure_daemon(ws, &root, &config, &cwd)) {
        Ok(socket) => {
            report(
                fails,
                "D-F18 daemon-healthy",
                Ok(format!("socket {}", socket.display())),
            );
            report(
                fails,
                "D-F19 remote-ui-attach",
                check_f19(h, ws, &socket, &config.sidebar),
            );
        }
        Err(error) => {
            report(fails, "D-F18 daemon-healthy", Err(anyhow!("{error:#}")));
            report(
                fails,
                "D-F19 remote-ui-attach",
                Err(anyhow!("skipped: no healthy daemon")),
            );
        }
    }

    match with_agent {
        None => println!("SKIP D-F20 agent-liveness: pass --with-agent <name> to run"),
        Some(name) => report(fails, "D-F20 agent-liveness", check_f20(h, ws, name)),
    }
}

fn report(fails: &mut usize, name: &str, result: Result<String>) {
    match result {
        Ok(detail) if detail.is_empty() => println!("OK   {name}"),
        Ok(detail) => println!("OK   {name}: {detail}"),
        Err(error) => {
            println!("FAIL {name}: {error:#}");
            *fails += 1;
        }
    }
}

/// D-F3: splitting a single-pane tab produces a full-height *root* split — both
/// panes span the tab's full height. A nested (non-root) split would not.
fn check_f3(h: &mut CliHerdr, ws: &str) -> Result<String> {
    let (_tab, anchor) = h.create_tab(ws)?;
    let new_pane = h.split_pane(&anchor, Dir::Right, 0.5, false)?;

    let layout = pane_layout(&anchor)?;
    let tab_height = area_height(&layout)?;
    let new_height = pane_height(&layout, &new_pane)?;
    let anchor_height = pane_height(&layout, &anchor)?;

    if new_height != tab_height {
        bail!("new pane height {new_height} != tab height {tab_height}");
    }
    if anchor_height != tab_height {
        bail!("anchor pane height {anchor_height} != tab height {tab_height}");
    }
    Ok(format!(
        "new pane height {new_height} == tab height {tab_height} (full-height root split)"
    ))
}

/// D-F4/F5: moving a pane out to a parking tab and back with `--ratio 0.4`
/// reconstructs the split at (approximately) that ratio.
fn check_f4_f5(h: &mut CliHerdr, ws: &str) -> Result<String> {
    let (tab, anchor) = h.create_tab(ws)?;
    let mover = h.split_pane(&anchor, Dir::Right, 0.5, false)?;

    let (parking_tab, _placeholder) = h.create_tab(ws)?;
    h.move_pane(&mover, &parking_tab, Dir::Right, None, None, false)?;
    h.move_pane(&mover, &tab, Dir::Right, Some(&anchor), Some(0.4), false)?;

    let layout = pane_layout(&anchor)?;
    let ratio = first_split_ratio(&layout)?;
    if (ratio - 0.4).abs() > 0.02 {
        bail!("reconstructed split ratio {ratio:.4} not within 0.02 of 0.4");
    }
    Ok(format!(
        "split ratio {ratio:.4} within 0.02 of requested 0.4"
    ))
}

/// D-F7: a full toggle-on + toggle-off round trip on a 3-pane scratch layout
/// restores the original pane rects exactly (this drives the real
/// `maneuver::toggle` open/close path).
fn check_f7(h: &mut CliHerdr, ws: &str) -> Result<String> {
    let (tab, anchor) = h.create_tab(ws)?;
    let right = h.split_pane(&anchor, Dir::Right, 0.4, false)?;
    let _bottom_right = h.split_pane(&right, Dir::Down, 0.4, false)?;

    let mut before = h.pane_rects(&tab)?;
    before.sort_by(|a, b| a.pane_id.cmp(&b.pane_id));
    if before.len() != 3 {
        bail!(
            "expected a 3-pane scratch layout, got {} panes",
            before.len()
        );
    }

    let ctx = maneuver::Ctx {
        workspace: ws.to_owned(),
        tab: tab.clone(),
        focused_pane: anchor.clone(),
        // Not exercised by this check (the trivial `true` sidebar command below
        // needs no real cwd) -- current_dir() is a harmless, always-available
        // placeholder.
        cwd: env::current_dir().unwrap_or_default(),
    };
    // A trivial sidebar command keeps the toggle path hermetic (no daemon needed
    // here — D-F18/F19 exercise the daemon separately). The sidebar pane's shell
    // stays alive so the close half of the round trip can find and close it.
    maneuver::toggle(h, &ctx, "true").context("toggle open failed")?;
    maneuver::toggle(h, &ctx, "true").context("toggle close failed")?;

    let mut after = h.pane_rects(&tab)?;
    after.sort_by(|a, b| a.pane_id.cmp(&b.pane_id));

    if before != after {
        bail!("rects not restored:\n  before={before:?}\n  after ={after:?}");
    }
    Ok(format!(
        "{}-pane layout restored exactly after toggle round trip",
        before.len()
    ))
}

/// D-F19: attaching `nvim --remote-ui` to the daemon inside a scratch pane makes
/// that pane display nvim. We prove it config-independently by driving the
/// daemon to render a unique sentinel line, then reading it back from the pane.
/// (Real user configs — e.g. distro setups that restyle end-of-buffer
/// markers via `eob=" "` — hide the classic `~`, so a sentinel is the robust
/// marker.)
fn check_f19(
    h: &mut CliHerdr,
    ws: &str,
    socket: &Path,
    sidebar: &crate::config::Sidebar,
) -> Result<String> {
    let (_tab, pane) = h.create_tab(ws)?;
    let mut attach = String::from("exec env");
    for (key, value) in sidebar.env_override() {
        attach.push_str(&format!(" {}={}", key, daemon::shell_quote(value)));
    }
    attach.push_str(&format!(
        " {} --server {} --remote-ui",
        daemon::shell_quote(&sidebar.nvim_bin),
        socket.display()
    ));
    h.run_in_pane(&pane, &attach)?;
    // Let the remote UI attach before we drive it.
    sleep(Duration::from_millis(1500));

    let sentinel = format!("DOCTOR_NVIM_OK_{}", std::process::id());
    nvim_remote_send(
        socket,
        &format!("<Esc><Cmd>enew<CR>i{sentinel}<Esc>"),
        sidebar,
    )?;

    let mut content = String::new();
    for _ in 0..12 {
        sleep(Duration::from_millis(400));
        content = pane_read(&pane).unwrap_or_default();
        if content.contains(&sentinel) {
            return Ok("nvim remote-ui attached; buffer sentinel rendered in pane".to_owned());
        }
    }
    let preview: String = content.lines().take(3).collect::<Vec<_>>().join(" | ");
    bail!("nvim sentinel not visible in pane after remote-ui attach; saw: {preview}")
}

/// D-F20 (optional, `--with-agent <name>`): start the named agent in a scratch
/// tab and confirm herdr registers it. Best-effort bonus check; skipped by
/// default.
fn check_f20(h: &mut CliHerdr, ws: &str, name: &str) -> Result<String> {
    let (tab, _root) = h.create_tab(ws)?;
    herdr_json(&[
        "agent",
        "start",
        name,
        "--workspace",
        ws,
        "--tab",
        &tab,
        "--no-focus",
    ])
    .with_context(|| format!("failed to start agent {name}"))?;

    // Give it a moment to register, then confirm herdr can resolve it.
    for _ in 0..20 {
        sleep(Duration::from_millis(500));
        if herdr_json(&["agent", "get", name]).is_ok() {
            return Ok(format!("agent {name} started and registered"));
        }
    }
    bail!("agent {name} did not register within 10s")
}

// --- CLI arg parsing -------------------------------------------------------

fn parse_with_agent(args: impl Iterator<Item = String>) -> Option<String> {
    let mut args = args;
    while let Some(arg) = args.next() {
        if arg == "--with-agent" {
            return args.next();
        }
    }
    None
}

// --- herdr CLI helpers (for facts the Herdr trait does not expose) ---------

fn herdr_json(args: &[&str]) -> Result<Value> {
    let output = Command::new("herdr")
        .args(args)
        .output()
        .with_context(|| format!("failed to run herdr {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "herdr {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("failed to parse JSON from herdr {}", args.join(" ")))
}

fn pane_layout(pane: &str) -> Result<Value> {
    herdr_json(&["pane", "layout", "--pane", pane])
}

fn pane_read(pane: &str) -> Result<String> {
    let output = Command::new("herdr")
        .args(["pane", "read", pane, "--source", "visible"])
        .output()
        .context("failed to run herdr pane read")?;
    if !output.status.success() {
        bail!(
            "herdr pane read failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn area_height(layout: &Value) -> Result<u64> {
    layout
        .pointer("/result/layout/area/height")
        .and_then(Value::as_u64)
        .context("pane layout missing result.layout.area.height")
}

fn pane_height(layout: &Value, pane: &str) -> Result<u64> {
    layout
        .pointer("/result/layout/panes")
        .and_then(Value::as_array)
        .context("pane layout missing result.layout.panes")?
        .iter()
        .find(|entry| entry.pointer("/pane_id").and_then(Value::as_str) == Some(pane))
        .and_then(|entry| entry.pointer("/rect/height").and_then(Value::as_u64))
        .with_context(|| format!("pane {pane} not found in layout"))
}

fn first_split_ratio(layout: &Value) -> Result<f64> {
    layout
        .pointer("/result/layout/splits/0/ratio")
        .and_then(Value::as_f64)
        .context("pane layout missing result.layout.splits[0].ratio")
}

fn nvim_remote_send(socket: &Path, keys: &str, sidebar: &crate::config::Sidebar) -> Result<()> {
    let status = daemon::nvim_cmd(sidebar)
        .arg("--server")
        .arg(socket)
        .arg("--remote-send")
        .arg(keys)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to run nvim --remote-send")?;
    if !status.success() {
        bail!("nvim --remote-send exited with failure");
    }
    Ok(())
}

// --- cleanup ----------------------------------------------------------------

struct WorkspaceGuard {
    id: String,
    label: String,
    closed: bool,
}

impl WorkspaceGuard {
    fn create(label: &str) -> Result<Self> {
        let value = herdr_json(&["workspace", "create", "--label", label, "--no-focus"])?;
        let id = value
            .pointer("/result/workspace/workspace_id")
            .and_then(Value::as_str)
            .context("workspace create response missing workspace_id")?
            .to_owned();
        Ok(Self {
            id,
            label: label.to_owned(),
            closed: false,
        })
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        let _ = Command::new("herdr")
            .args(["workspace", "close", &self.id])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        self.closed = true;
    }
}

impl Drop for WorkspaceGuard {
    fn drop(&mut self) {
        self.close();
    }
}

struct TempGuard {
    dir: PathBuf,
    sidebar: crate::config::Sidebar,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        quit_daemons(&self.dir.join("runtime"), &self.sidebar);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Quit every headless nvim daemon whose socket lives in `runtime_dir`.
fn quit_daemons(runtime_dir: &Path, sidebar: &crate::config::Sidebar) {
    if let Ok(entries) = fs::read_dir(runtime_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(OsStr::to_str) != Some("sock") {
                continue;
            }
            let _ = daemon::nvim_cmd(sidebar)
                .arg("--server")
                .arg(&path)
                .arg("--remote-send")
                .arg("<Esc><Cmd>qa!<CR>")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
    sleep(Duration::from_millis(300));
}

/// Reap (SIGKILL) any headless nvim still referencing our temp dir. Returns a
/// human-readable summary for the report.
fn reap_leftover_nvim(temp: &Path) -> String {
    let output = match Command::new("pgrep")
        .arg("-fl")
        .arg(temp.to_string_lossy().as_ref())
        .output()
    {
        Ok(output) => output,
        Err(_) => return "pgrep unavailable".to_owned(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let leaked: Vec<&str> = text.lines().filter(|line| line.contains("nvim")).collect();
    if leaked.is_empty() {
        return "none".to_owned();
    }
    for line in &leaked {
        if let Some(pid) = line.split_whitespace().next() {
            let _ = Command::new("kill").arg("-9").arg(pid).status();
        }
    }
    format!("killed: {}", leaked.join("; "))
}

/// Returns `(gone, raw workspace list output)`.
fn verify_workspace_gone(workspace_id: &str) -> Result<(bool, String)> {
    let output = Command::new("herdr")
        .args(["workspace", "list"])
        .output()
        .context("failed to run herdr workspace list")?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let gone = !stdout.contains(workspace_id) && !stdout.contains(WORKSPACE_LABEL);
    Ok((gone, stdout))
}
