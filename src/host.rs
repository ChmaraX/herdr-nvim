//! Which Herdr this process serves: the Herdr TUI (0.9.x, `herdr` CLI and
//! plugin env), or the Herdr desktop app (herdr-app + herdr-core `hc`).
//!
//! The choice is explicit: `HERDR_NVIM_HOST=app` selects the desktop app. The
//! app's mods and the `herdr-app/bin/herdr-nvim` wrapper set it; nothing in
//! the TUI plugin does, so the TUI flow is the default and stays unchanged.

use std::env;

use crate::herdr::{CliHerdr, Herdr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    /// The Herdr TUI plugin host (`herdr-plugin.toml`).
    Herdr,
    /// The Herdr desktop app: mods in `herdr-app/`, terminals in herdr-core.
    App,
}

pub const HOST_ENV: &str = "HERDR_NVIM_HOST";

pub fn current() -> Host {
    from_value(env::var(HOST_ENV).ok().as_deref())
}

fn from_value(value: Option<&str>) -> Host {
    match value.map(str::trim) {
        Some(value) if value.eq_ignore_ascii_case("app") => Host::App,
        _ => Host::Herdr,
    }
}

/// The host's implementation of the `Herdr` operations.
pub fn connect() -> Box<dyn Herdr> {
    match current() {
        Host::Herdr => Box::new(CliHerdr),
        Host::App => Box::new(crate::app::AppHost::default()),
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
