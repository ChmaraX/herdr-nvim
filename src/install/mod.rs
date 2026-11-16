//! `herdr-nvim app-install [--uninstall]`: herdr-nvim installs itself into
//! the Herdr app, the same way on every OS. The bootstraps (`install.sh`,
//! `install.ps1`) only download, verify and unpack a release bundle and run
//! its binary with this command; a checkout's `cargo build --release` runs
//! it the same way.
//!
//! Every step converges: a second run changes nothing, and a run after a
//! crash repairs what the crash left first.

mod locate;
mod mods;
mod prompt;
mod runtime;
mod source;
mod state;

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use anyhow::{bail, Context, Result};

use crate::app::{env::AppEnv, launch_record::InstallRecord};
use runtime::RuntimeStore;
use source::Source;

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
    // From here this process serves the app it found (its daemons' folder,
    // its herdr-app), and so does every program it runs (`herdr-app`, `hc`,
    // a runtime). `main` dispatched this command before choosing a host, and
    // no thread runs yet.
    app.export();
    crate::host::choose();
    // A foreign mod folder stops the run before it writes anything, the
    // lock file included; under the lock the check is made again for keeps.
    state::claim(&app.home)?;
    let lock = mods::InstallLock::take(&app.home)
        .with_context(|| format!("cannot lock {}", app.home.display()))?;
    let owned = state::claim(&app.home)?;
    mods::recover(&app.home, &owned)?;
    store.sweep_leftovers();
    if request.uninstall {
        let result = uninstall(&request, &app, &store, &owned);
        lock.remove();
        result
    } else {
        install(&request, &app, &store, &owned)
    }
}

fn install(
    request: &Request,
    app: &AppEnv,
    store: &RuntimeStore,
    owned: &state::Owned,
) -> Result<()> {
    let source = Source::locate(request.source.as_deref())?;
    ensure_nvim(request.install_nvim)?;
    let runtime = store.put(&source)?;
    let record = InstallRecord {
        schema: InstallRecord::SCHEMA,
        home: app.home.clone(),
        app_bin: app.app_bin.clone(),
        runtime: runtime.path().to_path_buf(),
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
    crate::app::rpc::check_hc().context("nothing changed")?;
    // What can fail without the person's say is checked above. After the
    // switch only the launcher's own `app-setup install` can fail the
    // install; the rest is best effort, reported.
    let swapped = mods::stage(app, &source, &record, owned)?.swap()?;
    mods::retire_pick(&app.home);
    // The watcher runs the runtime it started with: ended (the next nvim
    // pane starts it on this one), so it doesn't hold that runtime.
    if let Err(error) = crate::app::watch::stop() {
        eprintln!("herdr-nvim: {error:#}; the old watcher may keep its runtime until it ends");
    }
    // Through the launcher, as the mods run it: a launcher that doesn't
    // work fails the install (and nothing is pruned).
    run(
        swapped.launcher(),
        None,
        &[&["app-setup", "install"], &width[..]].concat(),
    )?;
    if let Some(tree) = &request.tree {
        run(swapped.launcher(), None, &["app-config", "tree", tree])?;
    }
    report_prune(store.prune(Some(&runtime), &runtime::live_runtimes()));
    println!("herdr-nvim: installed in {}", app.home.display());
    Ok(())
}

/// Takes it all out: the app's Files button back, the nvims (those with
/// unsaved work kept unless `--force`), the mods, the keys and layout edits,
/// the runtimes nothing runs. A folder Windows holds is reported, and the
/// rest still goes.
fn uninstall(
    request: &Request,
    app: &AppEnv,
    store: &RuntimeStore,
    owned: &state::Owned,
) -> Result<()> {
    // The Files button back first, if `tree = "nvim"` took it out (the
    // controller does it, so before the mods go).
    if crate::app::cli::app_do("herdr-nvim.uninstall", &serde_json::json!({})).is_ok() {
        thread::sleep(Duration::from_millis(500));
    }
    if let Err(error) = crate::daemon::inventory::stop_all(request.force) {
        eprintln!("herdr-nvim: {error:#}");
        eprintln!(
            "herdr-nvim: kept the nvims above; save (:wa) and quit them, or run the uninstall \
             with --force to discard their work"
        );
    }
    if let Err(error) = crate::app::watch::stop() {
        eprintln!("herdr-nvim: {error:#}; the watcher ends with the app's next nvim pane");
    }
    let kept = mods::remove(&app.home, owned);
    crate::app::setup::uninstall_keys_and_layouts(&app.home)?;
    let report = store.prune(None, &runtime::live_runtimes());
    let locked = !report.kept_locked.is_empty();
    report_prune(report);
    let _ = std::fs::remove_dir(&store.dir);
    let nvim = crate::nvim_get::dest()?;
    if nvim.is_dir() {
        println!(
            "herdr-nvim: kept the Neovim it downloaded, in {} (delete that folder if nothing else uses it)",
            nvim.display()
        );
    }
    if !kept.is_empty() || locked {
        bail!(
            "uninstalled from {}, except what is still in use (above); close those programs \
             (nvim panes, Explorer windows) and run the uninstall again",
            app.home.display()
        );
    }
    println!("herdr-nvim: uninstalled from {}", app.home.display());
    Ok(())
}

fn report_prune(report: runtime::PruneReport) {
    for dir in report.kept_live {
        println!("herdr-nvim: kept {}: a running nvim uses it", dir.display());
    }
    for dir in report.kept_locked {
        println!(
            "herdr-nvim: kept {} (in use; the next install removes it)",
            dir.display()
        );
    }
}

/// Runs a herdr-nvim: a runtime with `app`'s context in argv, or the
/// launcher (`None`: it reads its own record). Its output is the person's.
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

/// Neovim 0.10+, before anything changes: without it nothing would open.
fn ensure_nvim(yes: bool) -> Result<()> {
    let Err(missing) = crate::nvim_get::usable() else {
        return Ok(());
    };
    let method = crate::nvim_get::method()?;
    let how = crate::nvim_get::describe(&method);
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
    crate::nvim_get::install(&method).context("Neovim was not installed; nothing changed")
}
