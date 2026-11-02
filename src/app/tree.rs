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
//! - `app-reveal`: from inside the tab's nvim (`<leader>e`): focus the tree on
//!   the current file.

use std::{env, time::Duration};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::{config, daemon, state};

use super::{cli, raw_tab, sidebar};

const NVIM_WAIT: Duration = Duration::from_secs(10);

fn value_of<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|at| args.get(at + 1))
        .map(String::as_str)
}

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
    let root = value_of(&args, "--root").context("--root is required")?;
    let query = value_of(&args, "--query").unwrap_or_default();
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
    // nvim names files by their real path (/private/tmp on macOS); the tree
    // maps them onto its root through this.
    let real_root = std::fs::canonicalize(&ctx.cwd).unwrap_or_else(|_| ctx.cwd.clone());
    let config = config::load();
    let socket = daemon::registry::socket_path(&ctx.tab);
    let file = daemon::remote_expr_within(
        &socket,
        "expand('%:p')",
        &config.sidebar,
        Duration::from_secs(2),
    )
    .filter(|f| !f.is_empty());
    println!(
        "{}",
        json!({"file": file, "watch": watch, "real_root": real_root})
    );
    Ok(())
}

/// `herdr-nvim app-reveal`, run by the tab's nvim (it knows its tab from
/// `HERDR_TAB_ID`): the controller mod opens the Files panel if needed and
/// gives the tree the keyboard on the current file.
pub fn reveal_cmd() -> Result<()> {
    let tab = env::var("HERDR_TAB_ID").context("app-reveal runs inside a herdr-nvim nvim")?;
    cli::app_do(&format!("herdr-nvim.reveal:{}", raw_tab(&tab)), &json!({}))?;
    Ok(())
}
