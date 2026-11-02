//! The nvim sidebar in the Herdr app: a real pane of the tab, split off the
//! tab's whole tree so it runs full height beside every other pane. Its
//! terminal carries `herdr-nvim.sidebar=<tab>`, which is how a toggle finds it.

use std::{env, path::PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::{
    config::{self, SidebarPosition},
    maneuver::Ctx,
    state::TabId,
};

use super::{
    cli::{self, AppCtx},
    doc, host, raw_tab, rpc, tab_id, watch,
};

pub(crate) const LABEL: &str = "herdr-nvim.sidebar";

/// The app splits only right or down; left/top fall back to them.
pub(crate) fn direction(position: SidebarPosition) -> &'static str {
    match position {
        SidebarPosition::Right | SidebarPosition::Left => "right",
        SidebarPosition::Bottom | SidebarPosition::Top => "down",
    }
}

/// The live sidebar terminal of `tab` (`<space>:<tab>`), if any.
pub(crate) fn find(tab: &str) -> Result<Option<String>> {
    let list = rpc::call("terminal.list", json!({"labels": {LABEL: raw_tab(tab)}}))?;
    Ok(list
        .get("terminals")
        .and_then(Value::as_array)
        .and_then(|terminals| {
            terminals
                .iter()
                .filter(|t| t.get("state").and_then(Value::as_str) == Some("live"))
                .find_map(|t| t.get("address").and_then(Value::as_str))
        })
        .map(str::to_owned))
}

/// Close the pane showing `terminal` (its program ends with it).
pub(crate) fn close(terminal: &str) -> Result<()> {
    match doc::read()?.and_then(|doc| doc::pane_of(&doc, terminal)) {
        Some((pane, _)) => cli::app_do("pane.close", &json!({"pane": pane})).map(drop),
        None => cli::app_do("terminal.kill", &json!({"terminal": terminal})).map(drop),
    }
}

/// Give `terminal`'s pane the keyboard. A dialog still on screen (the picker,
/// closing as its program exits) hands the keyboard back to the pane it was
/// opened over when it goes, so wait for it to go first (at most 3 s).
pub(crate) fn focus(terminal: &str) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline
        && cli::app_status()
            .ok()
            .and_then(|status| status.get("overlays").cloned())
            .and_then(|overlays| overlays.as_array().cloned())
            .is_some_and(|overlays| overlays.iter().any(|o| o == "layout"))
    {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    cli::app_do("pane.focus", &json!({"terminal": terminal})).map(drop)
}

/// Split the whole tab and run the sidebar in the new pane: `pane.split`
/// accepts any node of the tab, so splitting its root node puts the sidebar
/// beside all of its panes, full height.
pub(crate) fn open(ctx: &Ctx) -> Result<String> {
    let config = config::load();
    let root = doc::read()?
        .and_then(|doc| doc::root_of(&doc, raw_tab(&ctx.tab)))
        .with_context(|| format!("tab {} is not in the app's layout", raw_tab(&ctx.tab)))?;
    let mut argv = vec![
        cli::self_command()?.to_string_lossy().into_owned(),
        "sidebar".to_owned(),
        "--cwd".to_owned(),
        ctx.cwd.to_string_lossy().into_owned(),
    ];
    // `tree = "nvim"`: nvim's own explorer is the tree, open it with nvim.
    if config.app.tree == crate::config::TreeMode::Nvim {
        argv.push("--explore".to_owned());
    }
    let made = cli::app_do(
        "pane.split",
        &json!({
            "direction": direction(config.sidebar.position),
            "pane": root,
            "argv": argv,
        }),
    )?;
    let terminal = made
        .get("terminal")
        .and_then(Value::as_str)
        .context("pane.split made no terminal")?
        .to_owned();
    // The sidebar labels itself too; doing it here as well means a toggle
    // right after this one already finds it.
    label(&terminal, raw_tab(&ctx.tab))?;
    Ok(terminal)
}

fn label(terminal: &str, tab: &str) -> Result<()> {
    rpc::call(
        "terminal.label",
        json!({"terminal": terminal, "labels": {LABEL: tab}}),
    )
    .map(drop)
}

/// The tab's sidebar, opened if it isn't.
pub(crate) fn ensure_open(ctx: &Ctx) -> Result<String> {
    match find(&ctx.tab)? {
        Some(terminal) => Ok(terminal),
        None => open(ctx),
    }
}

/// The TUI `Ctx` for an app invocation: the folder is the one the mod saw,
/// else the focused terminal's, else the space's.
pub(crate) fn ctx(app: &AppCtx) -> Result<Ctx> {
    let cwd = match &app.cwd {
        Some(cwd) if !cwd.is_empty() => PathBuf::from(cwd),
        _ => app
            .terminal
            .as_deref()
            .and_then(|t| host::terminal(t).ok())
            .and_then(|facts| host::cwd_of(&facts))
            .or_else(|| env::current_dir().ok())
            .context("no folder for the sidebar")?,
    };
    Ok(Ctx {
        workspace: app.space.clone(),
        tab: tab_id(&app.space, &app.tab),
        focused_pane: app.terminal.clone().unwrap_or_default(),
        cwd,
    })
}

/// The `Ctx` for a tab named by id alone (`open --tab`): its folder is its
/// first terminal's, which is where the app starts new terminals of the tab.
pub(crate) fn ctx_for_tab(app: &AppCtx) -> Result<Ctx> {
    if app.cwd.is_some() || app.terminal.is_some() {
        return ctx(app);
    }
    let first = doc::read()?
        .map(|doc| doc::terminals_of(&doc, &app.tab))
        .and_then(|terminals| terminals.into_iter().next());
    ctx(&AppCtx {
        terminal: first,
        ..app.clone()
    })
}

/// What `toggle --state <open|close>` asks for; no `--state` flips it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Want {
    Flip,
    Open,
    Close,
}

/// Pure: the `--state` argument.
pub(crate) fn want_from_args(args: &[String]) -> Result<Want> {
    match args
        .iter()
        .position(|arg| arg == "--state")
        .map(|at| args.get(at + 1).map(String::as_str))
    {
        None => Ok(Want::Flip),
        Some(Some("open")) => Ok(Want::Open),
        Some(Some("close")) => Ok(Want::Close),
        Some(other) => anyhow::bail!("--state is open or close, not {other:?}"),
    }
}

/// `herdr-nvim toggle [--state open|close] [--ctx JSON]`: open or close this
/// tab's nvim pane. With `--state` it is idempotent: the Files panel's
/// controller says what it wants, never "the other way round". Prints
/// `{"terminal": "tN"}` (open) or `{"closed": "tN"}`.
pub fn toggle_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let want = want_from_args(&args)?;
    let ctx = ctx(&cli::read_ctx(&args)?)?;
    // Best-effort: a reap failure must never block a toggle.
    let _ = watch::reconcile();
    // What changed, for the caller to wait on: the pane's terminal, opened
    // (`terminal`) or closed (`closed`).
    let made = match (find(&ctx.tab)?, want) {
        (Some(terminal), Want::Flip | Want::Close) => {
            close(&terminal)?;
            json!({"closed": terminal})
        }
        (None, Want::Flip | Want::Open) => json!({"terminal": open(&ctx)?}),
        (Some(terminal), Want::Open) => json!({"terminal": terminal}),
        (None, Want::Close) => json!({}),
    };
    println!("{made}");
    Ok(())
}

/// Inside the sidebar terminal: which tab it belongs to (the app labels its
/// terminals), its own address, and the folder to start nvim in.
pub(crate) fn identity() -> Result<(TabId, String, PathBuf)> {
    let me = env::var("HC_TERMINAL_ID")
        .context("herdr-nvim sidebar must run in a Herdr app terminal (HC_TERMINAL_ID)")?;
    let facts = host::terminal(&me)?;
    let tab = host::tab_of(&facts).context("this terminal is in no Herdr app tab")?;
    label(&me, raw_tab(&tab))?;
    let args: Vec<String> = env::args().collect();
    let cwd = args
        .iter()
        .position(|arg| arg == "--cwd")
        .and_then(|at| args.get(at + 1))
        .map(PathBuf::from)
        .or_else(|| host::cwd_of(&facts))
        .context("no folder for the sidebar")?;
    Ok((TabId::new(tab), me, cwd))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn state_argument_makes_the_toggle_idempotent() {
        assert_eq!(want_from_args(&args(&["--ctx", "{}"])).unwrap(), Want::Flip);
        assert_eq!(
            want_from_args(&args(&["--state", "open"])).unwrap(),
            Want::Open
        );
        assert_eq!(
            want_from_args(&args(&["--state", "close"])).unwrap(),
            Want::Close
        );
        assert!(want_from_args(&args(&["--state", "maybe"])).is_err());
        assert!(want_from_args(&args(&["--state"])).is_err());
    }
}
