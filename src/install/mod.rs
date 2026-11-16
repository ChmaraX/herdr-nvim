//! `herdr-nvim app-install [--uninstall]`: herdr-nvim installs itself into
//! the Herdr app, the same way on every OS. The bootstraps (`install.sh`,
//! `install.ps1`) only download, verify and unpack a release bundle and run
//! its binary with this command; a checkout's `cargo build --release` runs
//! it the same way.
//!
//! Every step converges: a second run changes nothing, and a run after a
//! crash repairs what the crash left first.

mod locate;
pub(crate) mod mods;
pub(crate) mod osfs;
mod prompt;
mod runtime;
mod source;
mod state;

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};

use crate::app::{env::AppEnv, launch_record::InstallRecord};
use mods::ModName;
use runtime::RuntimeStore;
use source::Source;
use state::Ownership;

const USAGE: &str = "usage: herdr-nvim app-install [--home DIR] [--app-bin PATH] [--source DIR] \
                     [--panel-width PX] [--tree native|nvim] [--install-nvim]\n       \
                     herdr-nvim app-install --uninstall [--force] [--home DIR] [--app-bin PATH]";

#[derive(Debug, Default, PartialEq)]
struct Request {
    uninstall: bool,
    force: bool,
    home: Option<PathBuf>,
    app_bin: Option<PathBuf>,
    source: Option<PathBuf>,
    panel_width: Option<String>,
    tree: Option<String>,
    install_nvim: bool,
}

fn parse(args: &[String]) -> Result<Request> {
    let mut request = Request::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next()
                .cloned()
                .with_context(|| format!("{name} needs a value\n{USAGE}"))
        };
        match arg.as_str() {
            "--uninstall" => request.uninstall = true,
            "--force" => request.force = true,
            "--install-nvim" => request.install_nvim = true,
            "--home" => request.home = Some(value(arg)?.into()),
            "--app-bin" => request.app_bin = Some(value(arg)?.into()),
            "--source" => request.source = Some(value(arg)?.into()),
            "--panel-width" => request.panel_width = Some(value(arg)?),
            "--tree" => match value(arg)?.as_str() {
                tree @ ("native" | "nvim") => request.tree = Some(tree.to_owned()),
                other => bail!("--tree is native or nvim, not {other}"),
            },
            "-h" | "--help" => bail!("{USAGE}"),
            other => bail!("unknown argument {other}\n{USAGE}"),
        }
    }
    Ok(request)
}

pub fn app_install_cmd() -> Result<()> {
    let args: Vec<String> = crate::argv::args().skip(2).collect();
    let request = parse(&args)?;
    let store = RuntimeStore::open()?;
    runtime::refuse_running_from(&store)?;
    let app = locate::locate(request.home.as_deref(), request.app_bin.as_deref())?;
    for name in ModName::ALL {
        if let Ownership::Foreign = state::read_mod(&name.live(&app.home)) {
            bail!(
                "{} is not herdr-nvim's: move it away first; nothing changed",
                name.live(&app.home).display()
            );
        }
    }
    mods::recover(&app.home)?;
    store.sweep_leftovers();
    if request.uninstall {
        uninstall(&request, &app, &store)
    } else {
        install(&request, &app, &store)
    }
}

fn install(request: &Request, app: &AppEnv, store: &RuntimeStore) -> Result<()> {
    let source = Source::locate(request.source.as_deref())?;
    ensure_nvim(&source, request.install_nvim)?;
    let runtime = store.put(&source)?;
    let record = InstallRecord {
        schema: InstallRecord::SCHEMA,
        home: app.home.clone(),
        app_bin: app.app_bin.clone(),
        runtime: runtime.path.clone(),
        by: format!("herdr-nvim {}", env!("CARGO_PKG_VERSION")),
    };
    let width: Vec<&str> = match &request.panel_width {
        Some(width) => vec!["--panel-width", width],
        None => vec![],
    };
    // Nothing of the person's changes until the install is known to fit:
    // the width and the layout it would edit, checked by the new runtime.
    run(
        &record.runtime_exe(),
        Some(app),
        &[&["app-setup", "check"], &width[..]].concat(),
    )
    .context("nothing changed")?;
    let swapped = mods::stage(app, &source, &record)?.swap()?;
    mods::retire_pick(&app.home);
    // The watcher runs the runtime it started with: ended (the next nvim
    // pane starts it on this one), so it doesn't hold that runtime.
    stop_watcher();
    report_prune(store.prune(
        Some(&runtime),
        &runtime::live_runtimes(&record.runtime_exe(), app),
    ));
    // Through the launcher, as the mods run it: a launcher that doesn't
    // work fails the install.
    run(
        &swapped.launcher,
        None,
        &[&["app-setup", "install"], &width[..]].concat(),
    )?;
    if let Some(tree) = &request.tree {
        run(&swapped.launcher, None, &["app-config", "tree", tree])?;
    }
    println!("herdr-nvim: installed in {}", app.home.display());
    Ok(())
}

fn uninstall(request: &Request, app: &AppEnv, store: &RuntimeStore) -> Result<()> {
    let me = std::env::current_exe().context("cannot find the herdr-nvim binary")?;
    // The Files button back first, if `tree = "nvim"` took it out (the
    // controller does it; the mods go below).
    let restored = Command::new(&app.app_bin)
        .args(["do", "herdr-nvim.uninstall"])
        .env("HERDR_APP_HOME", &app.home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if restored {
        thread::sleep(Duration::from_millis(500));
    }
    run(&me, Some(app), &["app-setup", "uninstall"])?;
    // Unsaved work is never thrown away unasked: those nvims are listed and
    // kept, with the runtime they run.
    let force: &[&str] = if request.force { &["--force"] } else { &[] };
    if run(
        &me,
        Some(app),
        &[&["daemons", "stop", "--all"], force].concat(),
    )
    .is_err()
    {
        eprintln!(
            "herdr-nvim: kept the nvims above; save (:wa) and quit them, or run the uninstall \
             with --force to discard their work"
        );
    }
    stop_watcher();
    mods::remove(&app.home)?;
    report_prune(store.prune(None, &runtime::live_runtimes(&me, app)));
    let _ = std::fs::remove_dir(&store.dir);
    println!("herdr-nvim: uninstalled from {}", app.home.display());
    let nvim = store.dir.with_file_name("nvim");
    if nvim.is_dir() {
        println!(
            "herdr-nvim: kept the Neovim it downloaded, in {} (delete that folder if nothing else uses it)",
            nvim.display()
        );
    }
    Ok(())
}

fn report_prune(report: runtime::PruneReport) {
    for dir in report.kept_locked {
        println!(
            "herdr-nvim: kept {} (in use; the next install removes it)",
            dir.display()
        );
    }
}

/// Runs a herdr-nvim (`exe`, with `app`'s context in argv when given; the
/// launcher brings its own). Its output is the person's.
fn run(exe: &Path, app: Option<&AppEnv>, args: &[&str]) -> Result<()> {
    let mut command = Command::new(exe);
    if let Some(app) = app {
        command.args(app.prefix());
    }
    let status = command
        .args(args)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("cannot run {}", exe.display()))?;
    if !status.success() {
        bail!("{} {} failed", exe.display(), args.join(" "));
    }
    Ok(())
}

/// Ends the watcher terminal (`herdr-nvim-watch`) and waits until it is gone.
fn stop_watcher() {
    let hc = |args: &[&str]| {
        crate::app::rpc::hc()
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    if !hc(&["kill", crate::app::watch::NAME]) {
        return;
    }
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(3) && hc(&["get", crate::app::watch::NAME]) {
        thread::sleep(Duration::from_millis(100));
    }
}

/// Neovim 0.10+, before anything changes: without it nothing would open.
fn ensure_nvim(source: &Source, yes: bool) -> Result<()> {
    let sidebar = crate::config::load().sidebar;
    let Err(missing) = crate::nvim_check::check(&sidebar) else {
        return Ok(());
    };
    let how = nvim_how();
    let yes = yes
        || prompt::ask(
            &format!("herdr-nvim: {missing}.\nInstall it now, {how}?"),
            true,
        )
        .unwrap_or(false);
    if !yes {
        bail!(
            "{missing}. Install it (or run this again with --install-nvim: {how}); nothing changed"
        );
    }
    install_nvim(source)
}

#[cfg(unix)]
fn nvim_how() -> String {
    let brew = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .any(|dir| dir.join("brew").is_file());
    if brew {
        "Homebrew's (brew install neovim)".to_owned()
    } else {
        "Neovim's own release, into ~/.local/share/herdr-nvim/nvim (no sudo)".to_owned()
    }
}

#[cfg(unix)]
fn install_nvim(source: &Source) -> Result<()> {
    let script = source.tree.join("herdr-app/install-nvim.sh");
    let ok = Command::new("sh")
        .arg(&script)
        .arg("--herdr-nvim")
        .arg(&source.exe)
        .status()
        .is_ok_and(|status| status.success());
    if !ok {
        bail!("Neovim was not installed; nothing changed");
    }
    Ok(())
}

#[cfg(not(unix))]
fn nvim_how() -> String {
    "winget install Neovim.Neovim".to_owned()
}

#[cfg(not(unix))]
fn install_nvim(_: &Source) -> Result<()> {
    bail!("install Neovim 0.10+ yourself (winget install Neovim.Neovim), then run this again")
}
