//! The nvim sidebar in the Herdr app: a real pane of the tab, split off the
//! tab's whole tree so it runs full height beside every other pane. Its
//! terminal carries `herdr-nvim.sidebar=<tab>`, which is how a toggle finds it.

use std::{env, path::PathBuf};

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::{
    config::{self, SidebarPosition},
    host::Opened,
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
/// beside all of its panes, full height. A preview showing in nvim's place
/// (`app-preview`) is swapped back instead: nvim takes the preview's pane.
///
/// A stale daemon (an older runtime's) is restarted first, or kept with a
/// notice when it has unsaved work (`stale`).
pub(crate) fn open(ctx: &Ctx) -> Result<Opened> {
    let notice =
        super::stale::refresh_on_open(ctx.tab.as_str(), &ctx.cwd).unwrap_or_else(|error| {
            eprintln!("herdr-nvim: could not check this tab's nvim for updates: {error:#}");
            None
        });
    let config = config::load();
    let doc = doc::read()?;
    let preview = doc
        .as_ref()
        .and_then(|doc| doc::preview_of(doc, raw_tab(&ctx.tab)));
    let anchor = match &preview {
        Some((pane, _)) => pane.clone(),
        None => doc
            .as_ref()
            .and_then(|doc| doc::root_of(doc, raw_tab(&ctx.tab)))
            .with_context(|| format!("tab {} is not in the app's layout", raw_tab(&ctx.tab)))?,
    };
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
    // In a preview's place: across, so nvim takes the preview's exact slot
    // (`swap_direction`); else beside the whole tab.
    let split = match preview {
        Some(_) => swap_direction(),
        None => direction(config.sidebar.position),
    };
    let made = cli::app_do(
        "pane.split",
        &json!({
            "direction": split,
            "pane": anchor,
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
    if let Some((pane, _)) = preview {
        cli::app_do("pane.close", &json!({"pane": pane}))?;
    }
    forget_preview(&ctx.tab);
    Ok(Opened {
        pane: terminal,
        notice,
    })
}

/// Show `file` rendered (the `herdr-nvim` mod's preview role) in nvim's
/// place: split nvim's pane (else a preview's, else the whole tab) with the
/// preview, then close what it replaces. nvim's daemon keeps running, with
/// its buffers, cursor and undo; `open` brings it back. Returns the
/// preview's pane, if the app named it.
pub(crate) fn open_preview(ctx: &Ctx, file: &str) -> Result<Value> {
    let config = config::load();
    let doc = doc::read()?.context("the app's layout is unknown")?;
    let raw = raw_tab(&ctx.tab);
    let nvim = find(&ctx.tab)?;
    let nvim_pane = nvim
        .as_deref()
        .and_then(|terminal| doc::pane_of(&doc, terminal))
        .map(|(pane, _)| pane);
    let old_preview = doc::preview_of(&doc, raw).map(|(pane, _)| pane);
    let (anchor, split) = match nvim_pane.clone().or_else(|| old_preview.clone()) {
        Some(pane) => (pane, swap_direction()),
        None => (
            doc::root_of(&doc, raw)
                .with_context(|| format!("tab {raw} is not in the app's layout"))?,
            direction(config.sidebar.position),
        ),
    };
    let made = cli::app_do(
        "pane.split",
        &json!({
            "direction": split,
            "pane": anchor,
            "mod": super::layout_patch::MOD,
            "props": {"preview": file, "tab": raw, "cwd": ctx.cwd},
        }),
    )?;
    if let Some(pane) = old_preview {
        cli::app_do("pane.close", &json!({"pane": pane}))?;
    }
    if let Some(pane) = nvim_pane {
        cli::app_do("pane.close", &json!({"pane": pane}))?;
    }
    forget_preview(&ctx.tab);
    // The preview's pane (`true` when the app didn't name it).
    let pane = made.get("pane").cloned().filter(|p| !p.is_null());
    let mut out = json!({"preview": pane.unwrap_or(json!(true))});
    if let Some(terminal) = nvim {
        out["closed"] = json!(terminal);
    }
    Ok(out)
}

/// The direction to split nvim's pane (or its preview) in for a swap that
/// keeps its place: across the split that made the slot, so both nest in
/// the slot and the new pane keeps its size when the old one closes (along
/// it, the app shares the row out again). The slot
/// is always made by splitting the whole tab in `direction(position)`, so
/// across it is the other one. Not read from the layout: a header asks when
/// its pane is new, which is mid-swap, while it still shares a nested split
/// with the pane it replaces (and that answer would be the wrong way).
pub(crate) fn swap_direction() -> &'static str {
    match direction(config::load().sidebar.position) {
        "right" => "down",
        _ => "right",
    }
}

/// What Files closed in this tab was a preview of `file`: the next Files
/// opens that preview again.
fn remember_preview(tab: &str, file: &str) {
    let path = crate::state::preview_memory_path(tab);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, file);
}

fn forget_preview(tab: &str) {
    let _ = crate::state::remove_file_if_exists(&crate::state::preview_memory_path(tab));
}

/// The preview Files closed in this tab, if its file is still there.
fn remembered_preview(tab: &str) -> Option<String> {
    std::fs::read_to_string(crate::state::preview_memory_path(tab))
        .ok()
        .filter(|file| std::path::Path::new(file).is_file())
}

fn label(terminal: &str, tab: &str) -> Result<()> {
    rpc::call(
        "terminal.label",
        json!({"terminal": terminal, "labels": {LABEL: tab}}),
    )
    .map(drop)
}

/// The tab's sidebar, opened if it isn't.
pub(crate) fn ensure_open(ctx: &Ctx) -> Result<Opened> {
    match find(&ctx.tab)? {
        Some(terminal) => Ok(Opened {
            pane: terminal,
            notice: None,
        }),
        None => open(ctx),
    }
}

/// The TUI `Ctx` for an app invocation: the folder is the one the mod saw,
/// else the focused terminal's, else the tab's (or its space's) in the app's
/// layout document. Never this process's own folder: the app's is often `/`.
pub(crate) fn ctx(app: &AppCtx) -> Result<Ctx> {
    let cwd = match &app.cwd {
        Some(cwd) if !cwd.is_empty() => PathBuf::from(cwd),
        _ => match app
            .terminal
            .as_deref()
            .and_then(|t| host::terminal(t).ok())
            .and_then(|facts| host::cwd_of(&facts))
        {
            Some(cwd) => cwd,
            None => doc::read()?
                .and_then(|doc| doc::folder_of(&doc, &app.tab))
                .map(PathBuf::from)
                .with_context(|| {
                    format!(
                        "no folder for tab {}: no focused terminal, and neither the tab nor its space has a folder",
                        app.tab
                    )
                })?,
        },
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
    match cli::flag(args, "--state")? {
        None => Ok(Want::Flip),
        Some("open") => Ok(Want::Open),
        Some("close") => Ok(Want::Close),
        Some(other) => anyhow::bail!("--state is open or close, not {other:?}"),
    }
}

/// `herdr-nvim toggle [--state open|close] [--ctx JSON]`: open or close this
/// tab's nvim pane. With `--state` it is idempotent: the Files panel's
/// controller says what it wants, never "the other way round". A preview in
/// nvim's place (`app-preview`) counts as the open pane: closing closes it
/// and remembers it, the next open shows it again. Prints
/// `{"terminal": "tN"}` (nvim opened), `{"preview": pane}` (preview opened)
/// or `{"closed": "tN"}`.
pub fn toggle_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let want = want_from_args(&args)?;
    let ctx = ctx(&cli::read_ctx(&args)?)?;
    // Best-effort: a reap failure must never block a toggle.
    let _ = watch::reconcile();
    let terminal = find(&ctx.tab)?;
    let preview = doc::read()?.and_then(|doc| doc::preview_of(&doc, raw_tab(&ctx.tab)));
    let shown = terminal.is_some() || preview.is_some();
    // What changed, for the caller to wait on: the pane's terminal, opened
    // (`terminal`) or closed (`closed`).
    let made = match (shown, want) {
        (true, Want::Flip | Want::Close) => {
            let mut made = json!({});
            if let Some((pane, file)) = preview {
                cli::app_do("pane.close", &json!({"pane": pane}))?;
                remember_preview(&ctx.tab, &file);
                made["closed_preview"] = json!(pane);
            }
            if let Some(terminal) = terminal {
                close(&terminal)?;
                made["closed"] = json!(terminal);
            }
            made
        }
        (false, Want::Flip | Want::Open) => match remembered_preview(&ctx.tab) {
            Some(file) => open_preview(&ctx, &file)?,
            None => {
                let opened = open(&ctx)?;
                match opened.notice {
                    Some(notice) => json!({"terminal": opened.pane, "notice": notice}),
                    None => json!({"terminal": opened.pane}),
                }
            }
        },
        (true, Want::Open) => match terminal {
            Some(terminal) => json!({"terminal": terminal}),
            None => json!({"preview": preview.map_or(json!(true), |(pane, _)| json!(pane))}),
        },
        (false, Want::Close) => json!({}),
    };
    println!("{made}");
    Ok(())
}

/// `herdr-nvim app-preview --file <path> --ctx JSON` (the header's Preview
/// button, Enter on an image in the tree): the file rendered in nvim's place.
pub fn preview_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let file = cli::flag(&args, "--file")?
        .context("usage: herdr-nvim app-preview --file <path> --ctx JSON")?;
    let ctx = ctx(&cli::read_ctx(&args)?)?;
    let file = crate::pathkey::absolute(file, &ctx.cwd);
    if !file.is_file() {
        anyhow::bail!("no such file: {}", file.display());
    }
    println!("{}", open_preview(&ctx, &file.to_string_lossy())?);
    Ok(())
}

/// `herdr-nvim app-source --ctx JSON` (the preview's Source button): nvim
/// back in the preview's place, as it was, with the keyboard.
pub fn source_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let ctx = ctx(&cli::read_ctx(&args)?)?;
    let opened = ensure_open(&ctx)?;
    focus(&opened.pane)?;
    let mut made = json!({"terminal": opened.pane});
    if let Some(notice) = opened.notice {
        made["notice"] = json!(notice);
    }
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
    let cwd = cli::flag(&args, "--cwd")?
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
