//! Talking to the Herdr app window (`herdr-app do`/`status`), finding our own
//! wrapper, and reading the invocation context a mod passes.

use std::{
    env,
    path::PathBuf,
    process::{Command, Stdio},
};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

/// The `herdr-app` binary: `HERDR_APP_BIN` (the install script bakes the app's
/// path into the wrapper), else `herdr-app` on PATH.
fn app_bin() -> String {
    env::var("HERDR_APP_BIN").unwrap_or_else(|_| "herdr-app".to_owned())
}

/// Runs an app command and returns what it made (`{pane, terminal}`, ...).
/// `HERDR_APP_HOME` (inherited) picks the window, as for `herdr-app` itself.
pub(crate) fn app_do(command: &str, args: &Value) -> Result<Value> {
    let output = Command::new(app_bin())
        .arg("do")
        .arg(command)
        .arg(args.to_string())
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("cannot run {} (set HERDR_APP_BIN)", app_bin()))?;
    let value: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
    if let Some(error) = value.get("error").and_then(Value::as_str) {
        bail!("herdr-app do {command}: {error}");
    }
    if !output.status.success() {
        bail!(
            "herdr-app do {command} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(value)
}

/// What the window shows now (`herdr-app status`): focused space, tab, pane.
pub(crate) fn app_status() -> Result<Value> {
    let output = Command::new(app_bin())
        .arg("status")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("cannot run {}", app_bin()))?;
    if !output.status.success() {
        bail!("herdr-app status failed: is the Herdr app open?");
    }
    serde_json::from_slice(&output.stdout).context("herdr-app status: bad JSON")
}

/// All machine-qualified spaces/tabs/panes as the window sees them.
pub(crate) fn app_listing() -> Result<Value> {
    let output = Command::new(app_bin())
        .arg("ls")
        .stdin(Stdio::null())
        .output()?;
    if !output.status.success() {
        bail!("cannot read the Herdr app's spaces");
    }
    serde_json::from_slice(&output.stdout).context("herdr-app ls: bad JSON")
}

/// The command that runs this binary with the app host's environment: the
/// `herdr-app/bin/herdr-nvim` wrapper exports `HERDR_NVIM_SELF`. Terminals the
/// app starts get the core's environment, not ours, so they must go through it.
pub(crate) fn self_command() -> Result<PathBuf> {
    match env::var_os("HERDR_NVIM_SELF") {
        Some(path) => Ok(PathBuf::from(path)),
        None => env::current_exe().context("cannot resolve the herdr-nvim binary"),
    }
}

/// Where a command was invoked: the focused space, tab and terminal, as the
/// mod read them from `focused()` (`--ctx '<json>'`).
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct AppCtx {
    pub space: String,
    pub tab: String,
    #[serde(default)]
    pub terminal: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Pure: the value after `name` (`--tab`) in `args`: `None` without the
/// flag, an error when nothing follows it.
pub(crate) fn flag<'a>(args: &'a [String], name: &str) -> Result<Option<&'a str>> {
    match args.iter().position(|arg| arg == name) {
        None => Ok(None),
        Some(at) => args
            .get(at + 1)
            .map(|value| Some(value.as_str()))
            .with_context(|| format!("{name} needs a value")),
    }
}

/// Pure: the first argument that is neither a flag nor the value of one of
/// `flags_with_values`.
pub(crate) fn positional<'a>(args: &'a [String], flags_with_values: &[&str]) -> Option<&'a str> {
    args.iter()
        .enumerate()
        .find(|(i, arg)| {
            !arg.starts_with("--")
                && (*i == 0 || !flags_with_values.contains(&args[i - 1].as_str()))
        })
        .map(|(_, arg)| arg.as_str())
}

/// Pure: the value after `--ctx` in `args`, parsed.
pub(crate) fn ctx_from_args(args: &[String]) -> Result<Option<AppCtx>> {
    let Some(raw) = flag(args, "--ctx")? else {
        return Ok(None);
    };
    let ctx: AppCtx = serde_json::from_str(raw).context("--ctx: invalid JSON")?;
    if ctx.space.is_empty() || ctx.tab.is_empty() {
        bail!("--ctx needs space and tab");
    }
    Ok(Some(ctx))
}

/// The context from `--ctx`, else from the window's focus.
pub(crate) fn read_ctx(args: &[String]) -> Result<AppCtx> {
    if let Some(ctx) = ctx_from_args(args)? {
        return Ok(ctx);
    }
    let status = app_status()?;
    let text = |key: &str| status.get(key).and_then(Value::as_str).map(str::to_owned);
    let (Some(space), Some(tab)) = (text("space"), text("tab")) else {
        bail!("the Herdr app shows no tab");
    };
    // The focused terminal: the agent an agent block shows, else the
    // terminal of the focused pane (from the layout document).
    let pane = text("focused_pane");
    let terminal = if super::remote::machine(&tab).is_some() {
        let listing = app_listing()?;
        listing["spaces"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|s| s["tabs"].as_array().into_iter().flatten())
            .filter(|t| t["id"] == tab)
            .flat_map(|t| t["terminals"].as_array().into_iter().flatten())
            .find(|t| t["pane"].as_str() == pane.as_deref())
            .and_then(|t| t["terminal"].as_str())
            .map(str::to_owned)
    } else {
        text("agent").or_else(|| {
            let doc = super::doc::read().ok().flatten()?;
            let pane = pane?;
            super::doc::terminal_of_pane(&doc, &pane)
        })
    };
    Ok(AppCtx {
        space,
        tab,
        terminal,
        cwd: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn flags_and_the_positional_argument() {
        let list = args(&["--tab", "tb1", "src/a.rs:3", "--space", "spA"]);
        assert_eq!(flag(&list, "--space").unwrap(), Some("spA"));
        assert_eq!(flag(&list, "--ctx").unwrap(), None);
        assert!(flag(&args(&["--tab"]), "--tab").is_err());
        assert_eq!(positional(&list, &["--tab", "--space"]), Some("src/a.rs:3"));
        assert_eq!(positional(&args(&["--tab", "tb1"]), &["--tab"]), None);
    }

    #[test]
    fn ctx_comes_from_the_mods_focus_json() {
        let ctx = ctx_from_args(&args(&[
            "toggle",
            "--ctx",
            r#"{"space":"spA","tab":"tbB","terminal":"t5","cwd":"/repo"}"#,
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(ctx.space, "spA");
        assert_eq!(ctx.terminal.as_deref(), Some("t5"));
        assert_eq!(ctx_from_args(&args(&["toggle"])).unwrap(), None);
        assert!(ctx_from_args(&args(&["toggle", "--ctx", r#"{"space":"","tab":"x"}"#])).is_err());
    }
}
