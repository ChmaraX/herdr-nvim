//! The native file tree's backend. The app draws the tree; the tab's nvim does
//! the work, so its buffers and language servers stay in step exactly as with
//! LazyVim's explorer:
//!
//! - `fs <request JSON> --ctx <tab>`: a file operation, run by the tab's nvim
//!   (`lua/herdr-nvim/fs.lua`); prints its JSON result.
//! - `app-find --root <dir> --query <text>`: fuzzy search of a folder with fff.
//! - `app-current --ctx <tab>`: the file the tab's nvim shows, the file that
//!   nvim rewrites whenever that changes (for the tree to watch), and the
//!   tab folder's real path (nvim's names for files in it).
//! - `app-reveal [--toggle]`: from inside the tab's nvim: focus the tree on
//!   the current file (`<leader>E`, `<leader>fe`); with `--toggle` (the tree
//!   key, `<leader>e`), hide the tree instead when it shows, keeping nvim.

use std::{env, time::Duration};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::{config, daemon, state};

use super::{cli, raw_tab, sidebar};

const NVIM_WAIT: Duration = Duration::from_secs(10);

/// `herdr-nvim fs '<request>' --ctx <tab>`.
pub fn fs_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let request = args
        .first()
        .context("usage: herdr-nvim fs '<request JSON>' --ctx <tab>")?;
    serde_json::from_str::<Value>(request).context("the request is not JSON")?;
    let ctx = sidebar::ctx(&cli::read_ctx(&args)?)?;
    let config = config::load();
    let socket = daemon::ensure_daemon(&ctx.tab, &daemon::plugin_root()?, &config, &ctx.cwd)?;
    let expr = format!(
        "luaeval(\"require('herdr-nvim.fs').run(_A)\", {})",
        daemon::vim_string_literal(request)
    );
    let Some(out) = daemon::remote_expr_within(&socket, &expr, &config.sidebar, NVIM_WAIT) else {
        bail!("the tab's nvim did not answer (is it waiting at a prompt?)");
    };
    let result: Value =
        serde_json::from_str(&out).with_context(|| format!("nvim answered {out:?}"))?;
    println!("{result}");
    if let Some(error) = result.get("error").and_then(Value::as_str) {
        bail!("{error}");
    }
    Ok(())
}

/// `herdr-nvim app-find --root <dir> --query <text>`: up to 100 paths relative
/// to `root`, best first.
pub fn find_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let root = cli::flag(&args, "--root")?.context("--root is required")?;
    let query = cli::flag(&args, "--query")?.unwrap_or_default();
    let index = crate::fff::FffIndex::open(root, config::load().picker.frecency)
        .context("cannot index that folder")?;
    let prefix = format!("{}/", root.trim_end_matches('/'));
    let hits: Vec<String> = index
        .search(query)
        .into_iter()
        .map(|hit| {
            hit.path
                .strip_prefix(&prefix)
                .unwrap_or(&hit.path)
                .to_owned()
        })
        .collect();
    println!("{}", json!({ "paths": hits }));
    Ok(())
}

/// `herdr-nvim app-current --ctx <tab>`.
pub fn current_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let ctx = sidebar::ctx(&cli::read_ctx(&args)?)?;
    let watch = state::current_file_path(&ctx.tab);
    if let Some(dir) = watch.parent() {
        // The tree watches this folder before nvim first writes into it.
        std::fs::create_dir_all(dir)?;
    }
    // The tree works with real paths (/private/tmp on macOS, not /tmp), so
    // one tab's folder is one tree whichever of its panes had the focus.
    let real_root = std::fs::canonicalize(&ctx.cwd).unwrap_or_else(|_| ctx.cwd.clone());
    // What the tab's nvim last wrote (`app.lua` `write_state`); an nvim that
    // hasn't written yet is asked, once.
    let state: Value = std::fs::read_to_string(&watch)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .filter(|state: &Value| state.get("cwd").is_some_and(Value::is_string))
        .or_else(|| {
            let config = config::load();
            let socket = daemon::registry::socket_path(&ctx.tab);
            let out = daemon::remote_expr_within(
                &socket,
                "json_encode({'file': expand('%:p'), 'cwd': getcwd()})",
                &config.sidebar,
                Duration::from_secs(2),
            )?;
            serde_json::from_str(&out).ok()
        })
        .unwrap_or_default();
    let real = |key: &str| {
        state
            .get(key)
            .and_then(Value::as_str)
            .filter(|path| !path.is_empty())
            .and_then(|path| std::fs::canonicalize(path).ok())
    };
    let file = real("file").filter(|path| path.is_file());
    // The tab's folder for the tree: where the tab's nvim works (`:pwd`), which
    // doesn't move when another pane of the tab is focused or `cd`s.
    let tab_root = real("cwd").unwrap_or_else(|| real_root.clone());
    let mut out =
        json!({"file": file, "watch": watch, "real_root": real_root, "tab_root": tab_root});
    if args.iter().any(|a| a == "--across") {
        out["across"] = json!(sidebar::swap_direction());
    }
    println!("{out}");
    Ok(())
}

/// `herdr-nvim app-reveal [--toggle]`, run by the tab's nvim (it knows its
/// tab from `HERDR_TAB_ID`): the controller mod opens the Files panel if
/// needed and gives the tree the keyboard on the current file. With
/// `--toggle`, a tree already showing is hidden instead and nvim stays.
pub fn reveal_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let tab = env::var("HERDR_TAB_ID").context("app-reveal runs inside a herdr-nvim nvim")?;
    cli::app_do(&reveal_command(&args, raw_tab(&tab)), &json!({}))?;
    Ok(())
}

/// Pure: the controller mod's command for `app-reveal <args>` in a tab.
pub(crate) fn reveal_command(args: &[String], tab: &str) -> String {
    let verb = if args.iter().any(|a| a == "--toggle") {
        "toggle"
    } else {
        "reveal"
    };
    format!("herdr-nvim.{verb}:{tab}")
}
