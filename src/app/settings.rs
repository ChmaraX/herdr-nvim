//! herdr-nvim's own settings for the Herdr app, in its config file
//! (`config::config_path`: `$HERDR_NVIM_CONFIG`, else
//! `${XDG_CONFIG_HOME:-~/.config}/herdr-nvim/config.toml`), section `[app]`:
//!
//! ```toml
//! [app]
//! tree = "native"        # or "nvim"
//! tree_key = "<leader>e"  # shows/hides the native tree; "" for none
//! ```
//!
//! Everything reads it at the moment it acts (the mods through
//! `herdr-nvim app-config`), so switching needs no reinstall.

use std::fs;

use anyhow::{bail, Context, Result};
use serde_json::json;

use crate::config::{self, TreeMode};

/// `herdr-nvim app-config`: the settings the mods and the nvim plugin need.
/// `herdr-nvim app-config tree <native|nvim>` sets `[app] tree`;
/// `herdr-nvim app-config nvim-bin <path>` sets `[sidebar] nvim_bin` (the
/// Neovim install, `install-nvim.sh`).
pub fn app_config_cmd() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let value = || args.get(1).map(String::as_str).unwrap_or_default();
    match args.first().map(String::as_str) {
        None => {}
        Some("tree") => return set_tree(value()),
        Some("nvim-bin") if !value().is_empty() => {
            return write_setting("sidebar", "nvim_bin", value())
        }
        Some(other) => bail!(
            "usage: herdr-nvim app-config [tree native|nvim | nvim-bin <path>], not {other:?}"
        ),
    }
    let config = config::load();
    println!(
        "{}",
        json!({
            "tree": config.app.tree.as_str(),
            "tree_key": config.app.tree_key,
            // How the controller splits for a Preview / Source swap, and
            // for nvim beside the tab.
            "across": super::sidebar::swap_direction(),
            "beside": super::sidebar::direction(config.sidebar.position),
            "config": config::config_path().map(|p| p.to_string_lossy().into_owned()),
        })
    );
    Ok(())
}

/// Pure: `text` (a config file) with `[section] key` set to `value`;
/// comments, order and everything else kept as they were (`toml_edit`).
pub(crate) fn with_setting(text: &str, section: &str, key: &str, value: &str) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = text.parse().context("the config is not valid TOML")?;
    let table = doc
        .entry(section)
        .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
    let Some(table) = table.as_table_like_mut() else {
        bail!("[{section}] in the config is not a table");
    };
    table.insert(key, toml_edit::value(value));
    Ok(doc.to_string())
}

/// Write `[app] tree = <mode>` into the config file.
pub(crate) fn set_tree(mode: &str) -> Result<()> {
    let mode = match mode {
        "native" => TreeMode::Native,
        "nvim" => TreeMode::Nvim,
        other => bail!("tree is native or nvim, not {other:?}"),
    };
    write_setting("app", "tree", mode.as_str())
}

/// Write `[section] key = <value>` into the config file.
fn write_setting(section: &str, key: &str, value: &str) -> Result<()> {
    let path = config::config_path().context("no config folder (HOME is not set)")?;
    // No file yet: start from nothing. Any other read error stops here: the
    // file is the person's, never replaced by one holding only `[app]`.
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", path.display())),
    };
    let updated =
        with_setting(&text, section, key, value).with_context(|| format!("{}", path.display()))?;
    if updated != text {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        // Whole or not at all: a temp file beside it, renamed over it.
        let tmp = path.with_extension("toml.herdr-nvim.tmp");
        fs::write(&tmp, updated).with_context(|| format!("cannot write {}", tmp.display()))?;
        fs::rename(&tmp, &path).with_context(|| format!("cannot replace {}", path.display()))?;
    }
    println!("{key}: {value} ({})", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_is_set_without_touching_the_rest() {
        let set =
            |text: &str, mode: TreeMode| with_setting(text, "app", "tree", mode.as_str()).unwrap();
        assert_eq!(set("", TreeMode::Nvim), "[app]\ntree = \"nvim\"\n");
        let with_app = "[app] # mine\ntree = \"native\" # was\n\n[picker]\nmax_files = 9\n";
        let out = set(with_app, TreeMode::Nvim);
        assert!(out.starts_with("[app] # mine\ntree = \"nvim\""), "{out}");
        assert!(out.ends_with("[picker]\nmax_files = 9\n"), "{out}");
        let dotted = "app.tree_key = \"\"\n[sidebar]\nnvim_bin = \"nvim\" # mine\n";
        let out = set(dotted, TreeMode::Nvim);
        assert!(out.contains("nvim_bin = \"nvim\" # mine"), "{out}");
        let parsed: crate::config::Config = toml::from_str(&out).unwrap();
        assert_eq!(
            (parsed.app.tree, parsed.app.tree_key.as_str()),
            (TreeMode::Nvim, "")
        );
        assert!(with_setting("app = 3\n", "app", "tree", "nvim").is_err());
    }

    #[test]
    fn the_written_file_parses_back() {
        let text = with_setting("[picker]\nmax_files = 9\n", "app", "tree", "nvim").unwrap();
        // Where install-nvim.sh points herdr-nvim at the Neovim it installed.
        let text = with_setting(&text, "sidebar", "nvim_bin", "/x/nvim/bin/nvim").unwrap();
        let parsed: crate::config::Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed.app.tree, TreeMode::Nvim);
        assert_eq!(parsed.app.tree_key, "<leader>e");
        assert_eq!(parsed.picker.max_files, 9);
        assert_eq!(parsed.sidebar.nvim_bin, "/x/nvim/bin/nvim");
    }
}
