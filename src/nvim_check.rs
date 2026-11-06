//! Whether the configured nvim (`[sidebar] nvim_bin`, with `nvim_env`) can run
//! herdr-nvim: found, and 0.10 or newer. Asked only when a tab's nvim has to
//! be started (`daemon::ensure_daemon`, the app's pane), so a running one costs
//! nothing. The Herdr app turns the error into its "Neovim is needed" card
//! (`main` prints it as JSON, exit 3); `app-nvim-check` is the same for the
//! installers.

use std::fmt;

use serde_json::{json, Value};

use crate::config::Sidebar;

/// The oldest Neovim herdr-nvim runs on.
pub const NEED: (u32, u32) = (0, 10);

/// The configured nvim can't run herdr-nvim: not found, or too old.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvimMissing {
    /// `nvim_bin` as configured.
    pub bin: String,
    /// The version it reported, when it ran.
    pub found: Option<String>,
}

impl fmt::Display for NvimMissing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (major, minor) = NEED;
        match &self.found {
            None => write!(
                f,
                "Neovim {major}.{minor} or newer is needed: `{}` was not found",
                self.bin
            ),
            Some(version) => write!(
                f,
                "Neovim {major}.{minor} or newer is needed: `{}` is {version}",
                self.bin
            ),
        }
    }
}

impl std::error::Error for NvimMissing {}

impl NvimMissing {
    /// What the app's card shows.
    pub fn to_json(&self) -> Value {
        json!({"bin": self.bin, "found": self.found, "need": need()})
    }
}

fn need() -> String {
    format!("{}.{}", NEED.0, NEED.1)
}

/// Pure: the version in `nvim --version`'s first line (`NVIM v0.12.6`,
/// `NVIM v0.11.0-dev-1234+gabc`), as (major, minor, patch).
pub fn parse_version(text: &str) -> Option<(u32, u32, u32)> {
    let line = text.lines().next()?;
    let version = line
        .split_whitespace()
        .find_map(|word| word.strip_prefix('v'))?;
    let mut parts = version
        .split(|c: char| !c.is_ascii_digit())
        .map(str::parse::<u32>);
    Some((
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    ))
}

/// The configured nvim's version, or why it can't run herdr-nvim.
pub fn check(sidebar: &Sidebar) -> Result<String, NvimMissing> {
    let missing = |found| NvimMissing {
        bin: sidebar.nvim_bin.clone(),
        found,
    };
    let output = crate::daemon::nvim_cmd(sidebar)
        .arg("--version")
        .output()
        .map_err(|_| missing(None))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let Some((major, minor, patch)) = parse_version(&text) else {
        return Err(missing(None));
    };
    let version = format!("{major}.{minor}.{patch}");
    if (major, minor) < NEED {
        return Err(missing(Some(version)));
    }
    Ok(version)
}

/// `herdr-nvim app-nvim-check`: `{"ok", "bin", "version", "need"}`; exit 0
/// when the configured nvim runs herdr-nvim, 3 when not (the installers).
pub fn check_cmd() -> anyhow::Result<()> {
    let config = crate::config::load();
    match check(&config.sidebar) {
        Ok(version) => {
            println!(
                "{}",
                json!({"ok": true, "bin": config.sidebar.nvim_bin, "version": version, "need": need()})
            );
            Ok(())
        }
        Err(missing) => Err(missing.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_release_and_dev_versions() {
        assert_eq!(
            parse_version("NVIM v0.12.6\nBuild type: Release\n"),
            Some((0, 12, 6))
        );
        assert_eq!(
            parse_version("NVIM v0.11.0-dev-1234+gabcdef\n"),
            Some((0, 11, 0))
        );
        assert_eq!(parse_version("NVIM v0.9.5"), Some((0, 9, 5)));
        assert_eq!(parse_version("zsh: command not found: nvim"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn a_missing_or_old_nvim_is_reported_with_what_was_found() {
        let mut sidebar = crate::config::Config::default().sidebar;
        sidebar.nvim_bin = "/nonexistent/nvim".to_owned();
        assert_eq!(
            check(&sidebar),
            Err(NvimMissing {
                bin: "/nonexistent/nvim".to_owned(),
                found: None
            })
        );
        let old = NvimMissing {
            bin: "nvim".to_owned(),
            found: Some("0.9.5".to_owned()),
        };
        assert_eq!(
            old.to_string(),
            "Neovim 0.10 or newer is needed: `nvim` is 0.9.5"
        );
    }
}
