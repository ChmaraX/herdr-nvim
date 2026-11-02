//! The Herdr desktop app host (herdr-app + herdr-core `hc`), selected with
//! `HERDR_NVIM_HOST=app` (see `host`). The mods in `herdr-app/mods/` call
//! these through the `herdr-app/bin/herdr-nvim` wrapper.
//!
//! Ids: a pane is a core terminal address (`t7`); a tab is
//! `<space id>:<tab id>`, so `TabId::workspace()` is the space, exactly as the
//! TUI's `w26:t2` gives its workspace.

mod cli;
pub(crate) mod commands;
mod doc;
mod host;
mod layout_patch;
mod node;
pub(crate) mod rpc;
mod session;
pub(crate) mod settings;
pub(crate) mod setup;
pub(crate) mod sidebar;
pub(crate) mod stale;
pub(crate) mod tree;
pub(crate) mod watch;

pub use host::{App, AppHost};

/// The composite tab id the shared code uses (`TabId`).
pub(crate) fn tab_id(space: &str, tab: &str) -> String {
    format!("{space}:{tab}")
}

/// The app's own tab id inside a composite one.
pub(crate) fn raw_tab(tab: &str) -> &str {
    tab.split_once(':').map_or(tab, |(_, tab)| tab)
}
