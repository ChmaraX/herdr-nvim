//! The app host's own subcommands: what the Lua plugin and the mods call
//! instead of the TUI's `herdr agent list` / `pane send-text` / plugin panes.

use std::{env, path::Path, process::Stdio};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::{bridge, extract, herdr::Herdr, host::Pick, openlink, pathkey};

use super::{cli, host, rpc, sidebar, AppHost};

/// `herdr-nvim app-agents`: every agent the app shows, in the shape of the
/// TUI's `herdr agent list`, so `agents.lua` parses both the same way.
pub fn agents_cmd() -> Result<()> {
    let agents: Vec<Value> = host::agents_in(&host::list_terminals()?)
        .into_iter()
        .map(|(agent, facts)| {
            let mut entry = json!({
                "pane_id": agent.pane_id,
                "workspace_id": crate::state::TabId::new(agent.tab_id.clone()).workspace(),
                "tab_id": agent.tab_id,
                "agent": facts.pointer("/status/app").and_then(Value::as_str).unwrap_or("agent"),
                "agent_status": facts.pointer("/status/state").and_then(Value::as_str).unwrap_or("unknown"),
                "cwd": host::cwd_of(&facts).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            });
            if let Some(title) = facts.get("title").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                entry["terminal_title"] = json!(title);
            }
            entry
        })
        .collect();
    println!("{}", json!({"result": {"agents": agents}}));
    Ok(())
}

/// `herdr-nvim app-send <terminal> <text> [--submit]`: submit through
/// `hc send` (paste + Enter once the person pauses typing), or paste only.
pub fn send_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let submit = args.iter().any(|arg| arg == "--submit");
    let mut rest = args.iter().filter(|arg| *arg != "--submit");
    let (Some(terminal), Some(text)) = (rest.next(), rest.next()) else {
        bail!("usage: herdr-nvim app-send <terminal> <text> [--submit]");
    };
    if !submit {
        return rpc::paste(terminal, text);
    }
    let output = rpc::hc()
        .args(["send", terminal, text])
        .stdin(Stdio::null())
        .output()
        .context("failed to run hc send")?;
    if !output.status.success() {
        bail!(
            "hc send {terminal} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// The app's last step of `pick-file` (`bridge::start_pick`): start the
/// picker in a hidden core terminal and print `{"terminal": "t9"}` for the mod
/// to show in a dialog, or `{"empty": "<why>"}`.
pub(crate) fn show_pick(pick: Pick) -> Result<()> {
    let (handoff, cwd) = match pick {
        Pick::NoAgent(_) => {
            println!("{}", json!({"empty": "No agent in this space"}));
            return Ok(());
        }
        Pick::Nothing => {
            println!(
                "{}",
                json!({"empty": "Nothing to pick yet: the agent has no session log here, the folder is not a git repo, and no file paths are on its screen"})
            );
            return Ok(());
        }
        Pick::Ready { handoff, cwd } => (handoff, cwd),
    };
    let started = rpc::call(
        "terminal.run",
        json!({
            "argv": [cli::self_command()?.to_string_lossy(), "picker"],
            "cwd": cwd,
            "env": {"HERDR_NVIM_HANDOFF": handoff, "TERM": "xterm-256color", "COLORTERM": "truecolor"},
            "no_parent": true,
            "rm": true,
        }),
    )?;
    let terminal = started
        .get("address")
        .and_then(Value::as_str)
        .context("terminal.run returned no address")?;
    println!("{}", json!({"terminal": terminal}));
    Ok(())
}

/// Pure: `path[:line[:col]]` split into its parts. Only a numeric tail is a
/// line or column, so a path that merely contains `:` stays whole.
pub(crate) fn split_location(text: &str) -> (String, Option<u32>, Option<u32>) {
    let tail = |s: &str| {
        s.rsplit_once(':')
            .and_then(|(head, n)| n.parse::<u32>().ok().map(|n| (head.to_owned(), n)))
    };
    match tail(text) {
        Some((head, last)) => match tail(&head) {
            Some((path, line)) => (path, Some(line), Some(last)),
            None => (head, Some(last), None),
        },
        None => (text.to_owned(), None, None),
    }
}

/// `herdr-nvim open [--tab <tab> [--space <space>] | --ctx JSON] <file>[:line[:col]]`:
/// open a file in a tab's nvim sidebar, opening the sidebar if it isn't, and
/// focus it. The contract for other mods (gitview's Changes, the file tree):
/// it needs no mod copy to be shown, only the tab. Relative paths resolve
/// against the tab's folder. Prints `{"terminal": "<sidebar>"}`.
pub(crate) fn open_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let target = cli::positional(&args, &["--tab", "--space", "--ctx"]).context(
        "usage: herdr-nvim open [--tab <tab> [--space <space>]] [--no-focus] <file>[:line[:col]]",
    )?;
    // The tree focuses nvim itself (`pane.focus` from the mod, no app
    // round trips).
    let focus = !args.iter().any(|a| a == "--no-focus");
    let app_ctx = match cli::flag(&args, "--tab")?.map(str::to_owned) {
        Some(tab) => {
            let space = match cli::flag(&args, "--space")?.map(str::to_owned) {
                Some(space) => space,
                None => super::doc::read()?
                    .and_then(|doc| super::doc::space_of(&doc, &tab))
                    .with_context(|| format!("no tab {tab} in the Herdr app"))?,
            };
            cli::AppCtx {
                space,
                tab,
                terminal: None,
                cwd: None,
            }
        }
        None => cli::read_ctx(&args)?,
    };
    let ctx = sidebar::ctx(&app_ctx)?;
    let (path, line, col) = split_location(target);
    let path = crate::pathkey::absolute(&path, &ctx.cwd);
    if !path.is_file() {
        bail!("no such file: {}", path.display());
    }
    let opened = bridge::open_in_sidebar_at(&ctx, &path.to_string_lossy(), line, col, focus)?;
    // An image went to the preview (`HostOps::preview`): no terminal.
    if opened.pane.is_empty() {
        println!("{}", json!({"preview": path}));
        return Ok(());
    }
    let mut made = json!({"terminal": opened.pane});
    if let Some(notice) = opened.notice {
        made["notice"] = json!(notice);
    }
    println!("{made}");
    Ok(())
}

/// Pure: a `herdr-nvim:` URL (`herdr-nvim:///abs/path:12` or
/// `herdr-nvim://open?path=/abs&line=12`) as the text a click would carry.
pub(crate) fn from_scheme(url: &str) -> Option<String> {
    let rest = url.strip_prefix("herdr-nvim:")?;
    let rest = rest.trim_start_matches('/');
    if let Some(query) = rest.strip_prefix("open?") {
        let mut path = None;
        let mut line = None;
        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("path", value)) => path = Some(value.to_owned()),
                Some(("line", value)) => line = Some(value.to_owned()),
                _ => {}
            }
        }
        let path = format!("file://{}", path?);
        return Some(match line {
            Some(line) => format!("{path}:{line}"),
            None => path,
        });
    }
    Some(format!("/{rest}"))
}

/// Pure: the line the agent named for `file` in its output (`file:12`, in any
/// spelling that resolves to it), newest mention first.
pub(crate) fn mentioned_line(text: &str, cwd: &Path, file: &Path) -> Option<u32> {
    let want = pathkey::canonical(file);
    extract::extract(text, cwd, &|p| p.is_file())
        .into_iter()
        .find(|mention| pathkey::canonical(Path::new(&mention.path)) == want)
        .and_then(|mention| mention.line)
}

/// `herdr-nvim open-link <path[:line[:col]] | file://… | herdr-nvim:…>` in the
/// app: open it in the focused tab's sidebar. The app hands clicked paths to
/// the system opener without their `:line`, so when none came with it, the
/// line is read back from the focused terminal's own text.
pub(crate) fn open_link() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let Some(clicked) = args
        .iter()
        .find(|arg| !arg.starts_with("--") && !arg.starts_with('{'))
    else {
        bail!("usage: herdr-nvim open-link <path[:line]> [--ctx JSON]");
    };
    let clicked = from_scheme(clicked).unwrap_or_else(|| clicked.clone());
    let Some((raw_path, line)) = openlink::parse_clicked(&clicked).or_else(|| {
        Path::new(&clicked)
            .is_absolute()
            .then(|| (clicked.clone(), None))
    }) else {
        return Ok(());
    };
    let ctx = sidebar::ctx(&cli::read_ctx(&args)?)?;
    let Some(resolved) = openlink::resolve_click(
        &raw_path,
        &ctx.cwd,
        &|p| p.is_file(),
        &crate::gitscan::toplevel,
    ) else {
        bail!("no such file: {raw_path}");
    };
    let mut herdr = AppHost;
    // The click came from the focused terminal, or (focus already moved, e.g.
    // to the sidebar) from the agent of this tab.
    let line = line.or_else(|| {
        let agent =
            bridge::target_agent_pane(&mut herdr, &ctx.workspace, &ctx.tab, &ctx.focused_pane).ok();
        [Some(ctx.focused_pane.clone()), agent]
            .into_iter()
            .flatten()
            .filter(|terminal| !terminal.is_empty())
            .find_map(|terminal| {
                let text = herdr.read_pane(&terminal, 400).ok()?;
                mentioned_line(&text, &ctx.cwd, &resolved)
            })
    });
    bridge::open_in_sidebar(&ctx, &resolved.to_string_lossy(), line).map(drop)
}

/// `herdr-nvim app-install-nvim [--pause]`: the "Neovim is needed" card's
/// Install button, in a terminal tab of its own so the person sees what runs
/// (and any password prompt): the runtime's `install-nvim.sh`, which installs
/// Neovim and points `[sidebar] nvim_bin` at it. macOS and Linux (a shell
/// script, like the app install itself).
#[cfg(unix)]
pub fn install_nvim_cmd() -> Result<()> {
    use std::os::unix::process::CommandExt;
    let root = crate::daemon::plugin_root()?;
    let script = ["bin/install-nvim.sh", "herdr-app/install-nvim.sh"]
        .iter()
        .map(|rel| root.join(rel))
        .find(|path| path.is_file())
        .with_context(|| format!("no install-nvim.sh in {}", root.display()))?;
    let error = std::process::Command::new("sh")
        .arg(&script)
        .arg("--herdr-nvim")
        .arg(cli::self_command()?)
        .args(env::args().skip(2))
        .exec();
    Err(error).with_context(|| format!("cannot run {}", script.display()))
}

#[cfg(not(unix))]
pub fn install_nvim_cmd() -> Result<()> {
    bail!("install Neovim 0.10+ yourself (winget install Neovim.Neovim), then press cmd-e again")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_split_into_path_line_and_column() {
        assert_eq!(
            split_location("src/a.rs:12:7"),
            ("src/a.rs".into(), Some(12), Some(7))
        );
        assert_eq!(
            split_location("src/a.rs:12"),
            ("src/a.rs".into(), Some(12), None)
        );
        assert_eq!(
            split_location("/repo/a.rs"),
            ("/repo/a.rs".into(), None, None)
        );
        assert_eq!(
            split_location("C:/x/a.rs:3"),
            ("C:/x/a.rs".into(), Some(3), None)
        );
    }

    #[test]
    fn scheme_urls_become_click_text() {
        assert_eq!(
            from_scheme("herdr-nvim:///repo/src/a.rs:12").as_deref(),
            Some("/repo/src/a.rs:12")
        );
        assert_eq!(
            from_scheme("herdr-nvim://open?path=/repo/a.rs&line=7").as_deref(),
            Some("file:///repo/a.rs:7")
        );
        assert_eq!(from_scheme("/repo/a.rs"), None);
    }

    #[test]
    fn recovers_the_line_from_the_agents_mention() {
        let dir = std::env::temp_dir().join(format!("hn-line-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let file = dir.join("src/greet.js");
        std::fs::write(&file, "x\n").unwrap();
        let text = "I read src/greet.js:3 first\nthen fixed src/greet.js:5 for real\nunrelated src/other.js:9";
        assert_eq!(
            mentioned_line(text, &dir, &file),
            Some(5),
            "the newest mention wins"
        );
        assert_eq!(mentioned_line("no mention here", &dir, &file), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
