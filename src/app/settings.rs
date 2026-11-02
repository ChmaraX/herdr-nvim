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
pub fn app_config_cmd() -> Result<()> {
    let config = config::load();
    println!(
        "{}",
        json!({
            "tree": config.app.tree.as_str(),
            "tree_key": config.app.tree_key,
            "config": config::config_path().map(|p| p.to_string_lossy().into_owned()),
        })
    );
    Ok(())
}

/// Pure: `text` (a config file) with `[app] tree` set to `mode`, every other
/// line kept as it was.
pub(crate) fn with_tree(text: &str, mode: TreeMode) -> String {
    let line = format!("tree = \"{}\"", mode.as_str());
    let mut out: Vec<String> = Vec::new();
    let mut in_app = false;
    let mut done = false;
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.starts_with('[') {
            if in_app && !done {
                out.push(line.clone());
                done = true;
            }
            in_app = trimmed == "[app]";
        } else if in_app && !done && trimmed.split('=').next().map(str::trim) == Some("tree") {
            out.push(line.clone());
            done = true;
            continue;
        }
        out.push(raw.to_owned());
    }
    if in_app && !done {
        out.push(line.clone());
        done = true;
    }
    if !done {
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push("[app]".to_owned());
        out.push(line);
    }
    format!("{}\n", out.join("\n"))
}

/// Write `[app] tree = <mode>` into the config file.
pub(crate) fn set_tree(mode: &str) -> Result<()> {
    let mode = match mode {
        "native" => TreeMode::Native,
        "nvim" => TreeMode::Nvim,
        other => bail!("tree is native or nvim, not {other:?}"),
    };
    let path = config::config_path().context("no config folder (HOME is not set)")?;
    let text = fs::read_to_string(&path).unwrap_or_default();
    let updated = with_tree(&text, mode);
    if updated != text {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, updated).with_context(|| format!("cannot write {}", path.display()))?;
    }
    println!("tree: {} ({})", mode.as_str(), path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_is_set_without_touching_the_rest() {
        assert_eq!(with_tree("", TreeMode::Nvim), "[app]\ntree = \"nvim\"\n");
        let config = "[sidebar]\nnvim_bin = \"nvim\" # mine\n\n[picker]\nmax_files = 9\n";
        assert_eq!(
            with_tree(config, TreeMode::Native),
            format!("{config}\n[app]\ntree = \"native\"\n")
        );
        let with_app = "[app]\ntree = \"native\"\n\n[picker]\nmax_files = 9\n";
        assert_eq!(
            with_tree(with_app, TreeMode::Nvim),
            "[app]\ntree = \"nvim\"\n\n[picker]\nmax_files = 9\n"
        );
        let app_without_tree = "[app]\n[picker]\nmax_files = 9\n";
        assert_eq!(
            with_tree(app_without_tree, TreeMode::Nvim),
            "[app]\ntree = \"nvim\"\n[picker]\nmax_files = 9\n"
        );
    }

    #[test]
    fn tree_key_defaults_to_leader_e_and_can_be_off() {
        let parse = |text: &str| {
            toml::from_str::<crate::config::Config>(text)
                .unwrap()
                .app
                .tree_key
        };
        assert_eq!(parse(""), "<leader>e");
        assert_eq!(parse("[app]\ntree = \"native\"\n"), "<leader>e");
        assert_eq!(parse("[app]\ntree_key = \"\"\n"), "");
        assert_eq!(parse("[app]\ntree_key = \"<C-n>\"\n"), "<C-n>");
        assert_eq!(
            with_tree("[app]\ntree_key = \"\"\n", TreeMode::Nvim),
            "[app]\ntree_key = \"\"\ntree = \"nvim\"\n"
        );
    }

    #[test]
    fn the_written_file_parses_back() {
        let text = with_tree("[picker]\nmax_files = 9\n", TreeMode::Nvim);
        let parsed: crate::config::Config = toml::from_str(&text).unwrap();
        assert_eq!(parsed.app.tree, TreeMode::Nvim);
        assert_eq!(parsed.app.tree_key, "<leader>e");
        assert_eq!(parsed.picker.max_files, 9);
    }
}
