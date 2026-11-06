//! Which Herdr this process serves: the Herdr TUI (0.9.x, `herdr` CLI and
//! plugin env), or the Herdr desktop app (herdr-app + herdr-core `hc`).
//!
//! The choice is explicit: `HERDR_NVIM_HOST=app` selects the desktop app. The
//! app's mods and the `herdr-app/bin/herdr-nvim` wrapper set it; nothing in
//! the TUI plugin does, so the TUI flow is the default and stays unchanged.
//!
//! `main` chooses once ([`choose`]); the shared code never asks which host it
//! runs under, it calls the chosen host's [`HostOps`]: the few steps that
//! differ (opening and focusing the tab's nvim pane, a daemon's environment,
//! its socket folder, who the sidebar pane is).

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

use anyhow::{Context, Result};

use crate::{
    config::Sidebar,
    herdr::{CliHerdr, Herdr},
    maneuver::{self, Ctx},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    /// The Herdr TUI plugin host (`herdr-plugin.toml`).
    Herdr,
    /// The Herdr desktop app: mods in `herdr-app/`, terminals in herdr-core.
    App,
}

pub const HOST_ENV: &str = "HERDR_NVIM_HOST";

static CHOSEN: OnceLock<Host> = OnceLock::new();

/// Chooses the host from `HERDR_NVIM_HOST`, once per process (`main`).
pub fn choose() -> Host {
    *CHOSEN.get_or_init(|| from_value(env::var(HOST_ENV).ok().as_deref()))
}

fn from_value(value: Option<&str>) -> Host {
    match value.map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case("app") => Host::App,
        _ => Host::Herdr,
    }
}

/// The tab's nvim pane after [`HostOps::ensure_sidebar`]: its pane (TUI) or
/// terminal (app), and a notice for the person, if any (the app's tab nvim
/// runs an older herdr-nvim and was kept for its unsaved work).
#[derive(Debug, Default)]
pub struct Opened {
    pub pane: String,
    pub notice: Option<String>,
}

/// What `pick-file` found, for the host to show (`HostOps::show_pick`).
pub enum Pick {
    /// No agent pane to read in this workspace (why).
    NoAgent(anyhow::Error),
    /// The agent's session, git and screen gave no file.
    Nothing,
    /// The candidates are in the handoff file; the picker runs in `cwd`.
    Ready { handoff: String, cwd: PathBuf },
}

/// The steps that differ between the hosts.
pub trait HostOps: Sync {
    /// The host's answers to the shared queries (agents, tabs, pane output).
    fn herdr(&self) -> Box<dyn Herdr>;
    /// The tab this command was run for.
    fn ctx(&self) -> Result<Ctx>;
    /// `pick-file`'s last step: show the picker (or say why there is none).
    fn show_pick(&self, pick: Pick) -> Result<()>;
    /// The tab's nvim pane, opened if it isn't.
    fn ensure_sidebar(&self, ctx: &Ctx) -> Result<Opened>;
    /// The tab's nvim pane if it is open: its daemon runs then, and a file
    /// can go straight to it.
    fn shown_sidebar(&self, _ctx: &Ctx) -> Option<String> {
        None
    }
    /// Show `path` rendered instead of in nvim, when this host can and nvim
    /// can't show it (an image in the Herdr app). Whether it did.
    fn preview(&self, _ctx: &Ctx, _path: &str) -> Result<bool> {
        Ok(false)
    }
    /// Give the tab's nvim pane the keyboard. Best-effort: says why on stderr.
    fn focus(&self, pane: &str);
    /// What a daemon for `tab` gets on top of the shared command line.
    fn daemon_env(&self, _command: &mut Command, _tab: &str, _plugin_root: &Path) {}
    /// The daemons' own folder under the shared socket folder: each host
    /// reaps the daemons whose tab it doesn't know, so neither may see the
    /// other's.
    fn socket_subdir(&self) -> Option<&'static str> {
        None
    }
    /// Where the tab's nvim starts when a file is opened and it isn't running.
    fn nvim_folder(&self, ctx: &Ctx) -> PathBuf {
        ctx.cwd.clone()
    }
    /// Inside the sidebar pane: its tab, its own pane id, its folder.
    fn sidebar_identity(&self) -> Result<(String, String, PathBuf)>;
    /// Inside the sidebar pane, its daemon up, before nvim's window attaches.
    fn sidebar_ready(&self, _socket: &Path, _sidebar: &Sidebar) {}
}

/// The Herdr TUI.
pub struct Tui;

impl HostOps for Tui {
    fn herdr(&self) -> Box<dyn Herdr> {
        Box::new(CliHerdr)
    }

    /// herdr's plugin env (`HERDR_PANE_ID`, …).
    fn ctx(&self) -> Result<Ctx> {
        maneuver::read_ctx(&mut CliHerdr)
    }

    /// A floating popup pane; nothing to pick is a toast, no agent an error.
    fn show_pick(&self, pick: Pick) -> Result<()> {
        match pick {
            Pick::NoAgent(error) => Err(error),
            Pick::Nothing => {
                crate::bridge::notify_no_candidates();
                Ok(())
            }
            Pick::Ready { handoff, .. } => crate::bridge::open_picker(&handoff),
        }
    }

    fn ensure_sidebar(&self, ctx: &Ctx) -> Result<Opened> {
        Ok(Opened {
            pane: maneuver::ensure_open(&mut CliHerdr, ctx)?,
            notice: None,
        })
    }

    fn focus(&self, pane: &str) {
        let result = Command::new("herdr")
            .args(["agent", "focus", pane])
            .status();
        match result {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!("herdr-nvim: could not focus sidebar pane {pane} (exit {status})")
            }
            Err(error) => eprintln!("herdr-nvim: could not focus sidebar pane {pane}: {error}"),
        }
    }

    /// herdr sets `HERDR_TAB_ID` and `HERDR_PANE_ID` for plugin panes; the
    /// pane's own cwd (set via `--cwd`) is where the daemon spawns.
    fn sidebar_identity(&self) -> Result<(String, String, PathBuf)> {
        let tab = env::var("HERDR_TAB_ID")
            .context("herdr-nvim sidebar requires HERDR_TAB_ID (set by herdr for plugin panes)")?;
        let pane = env::var("HERDR_PANE_ID")
            .context("herdr-nvim sidebar requires HERDR_PANE_ID (set by herdr for plugin panes)")?;
        let cwd = env::current_dir().context("herdr-nvim sidebar could not resolve its cwd")?;
        Ok((tab, pane, cwd))
    }
}

/// The host `main` chose; the TUI before it chose (unit tests).
pub fn ops() -> &'static dyn HostOps {
    match CHOSEN.get().copied().unwrap_or(Host::Herdr) {
        Host::Herdr => &Tui,
        Host::App => &crate::app::App,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_explicit_app_value_selects_the_app() {
        assert_eq!(from_value(None), Host::Herdr);
        assert_eq!(from_value(Some("")), Host::Herdr);
        assert_eq!(from_value(Some("herdr")), Host::Herdr);
        assert_eq!(from_value(Some("app")), Host::App);
        assert_eq!(from_value(Some(" APP ")), Host::App);
    }
}
