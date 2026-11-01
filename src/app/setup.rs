//! `herdr-nvim app-setup install|uninstall --keys <keys.json> [--panel-width N]`: the two edits
//! the Herdr app install makes outside the mod folders, and undoes.
//!
//! * **Layout** (`layout_patch`): the shown layout's `files` slot gets the
//!   `herdr-nvim` tree, a floating `herdr-nvim-sync` controller is added and
//!   the panel narrowed to `--panel-width` (default 300). Herdr's own layouts
//!   can't be edited, so a shipped one is cloned first (`<name>-mods`, shared
//!   with other mods that patch other slots the same way); a personal one is
//!   patched in place. Every write re-reads the file and replaces it whole only
//!   if nobody changed it meanwhile. Uninstall takes it all out and drops a
//!   clone that no longer differs from the layout it was cloned from.
//! * **Keys**: the mod's `keys.json` entries are added where the key is free;
//!   a key already bound to something else is left alone and reported.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

use super::{
    cli,
    layout_patch::{self, DEFAULT_PANEL_PX},
};

fn home() -> PathBuf {
    env::var_os("HERDR_APP_HOME")
        .map(PathBuf::from)
        .or_else(|| crate::home::home_dir().map(|home| home.join(".config/herdr-app")))
        .unwrap_or_else(|| PathBuf::from(".config/herdr-app"))
}

/// Herdr's own layouts (`installed.json` lists them as `layout:<name>`).
fn shipped(home: &Path, name: &str) -> bool {
    fs::read_to_string(home.join("installed.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|installed| installed.get(format!("layout:{name}")).is_some())
}

/// Read, change, and replace `path` whole, unless it changed meanwhile (then
/// read again). `edit` returns whether it changed anything.
fn edit_json(path: &Path, edit: &mut dyn FnMut(&mut Value) -> Result<bool>) -> Result<bool> {
    for _ in 0..5 {
        let before = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        let mut value: Value = serde_json::from_slice(&before)
            .with_context(|| format!("{} is not valid JSON", path.display()))?;
        if !edit(&mut value)? {
            return Ok(false);
        }
        let name = path.file_name().context("no file name")?.to_string_lossy();
        let tmp = path.with_file_name(format!(".{name}.herdr-nvim.tmp"));
        fs::write(&tmp, format!("{}\n", serde_json::to_string_pretty(&value)?))?;
        if fs::read(path)? != before {
            let _ = fs::remove_file(&tmp);
            continue;
        }
        fs::rename(&tmp, path)?;
        return Ok(true);
    }
    bail!("{} keeps changing; try again", path.display())
}

/// Pure: add `ours` to `keys` where free. Returns (added, conflicts).
pub(crate) fn merge_keys(keys: &mut Value, ours: &Value) -> (Vec<String>, Vec<String>) {
    let (mut added, mut conflicts) = (Vec::new(), Vec::new());
    for (section, bindings) in ours.as_object().into_iter().flatten() {
        let Some(bindings) = bindings.as_object() else {
            continue;
        };
        if !keys.get(section).is_some_and(Value::is_object) {
            keys[section] = Value::Object(Map::new());
        }
        let target = keys[section].as_object_mut().expect("just made an object");
        for (key, action) in bindings {
            match target.get(key) {
                None => {
                    target.insert(key.clone(), action.clone());
                    added.push(format!("{section} {key} → {action}"));
                }
                Some(existing) if existing == action => {}
                Some(existing) => conflicts.push(format!(
                    "{section} {key} is {existing}; bind {action} yourself if you want it"
                )),
            }
        }
    }
    (added, conflicts)
}

/// Pure: remove the bindings of `ours` that still have our action.
pub(crate) fn unmerge_keys(keys: &mut Value, ours: &Value) -> Vec<String> {
    let mut removed = Vec::new();
    for (section, bindings) in ours.as_object().into_iter().flatten() {
        let Some(target) = keys.get_mut(section).and_then(Value::as_object_mut) else {
            continue;
        };
        for (key, action) in bindings.as_object().into_iter().flatten() {
            if target.get(key) == Some(action) {
                target.remove(key);
                removed.push(format!("{section} {key}"));
            }
        }
    }
    removed
}

fn layout_path(home: &Path, name: &str) -> PathBuf {
    home.join("layouts").join(format!("{name}.json"))
}

fn shown_layout() -> Result<String> {
    cli::app_status()?
        .get("layout")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .context("herdr-app status names no layout")
}

fn install_layout(home: &Path, panel_width: u32) -> Result<()> {
    let shown = shown_layout()?;
    let target = if shipped(home, &shown) {
        let clone = env::var("HERDR_NVIM_LAYOUT_AS").unwrap_or_else(|_| format!("{shown}-mods"));
        if layout_path(home, &clone).exists() {
            cli::app_do("layout.switch", &json!({"name": clone}))?;
            println!("layout: switched to {clone} (already a personal copy of {shown})");
        } else {
            cli::app_do("layout.clone", &json!({"name": shown, "as": clone}))?;
            println!("layout: cloned {shown} as {clone} (Herdr's own layouts can't be edited)");
        }
        clone
    } else {
        shown
    };
    if edit_json(&layout_path(home, &target), &mut |layout| {
        layout_patch::patch(layout, panel_width)
    })? {
        println!("layout: {target}: Files now opens herdr-nvim's tree and the tab's nvim together");
    } else {
        println!("layout: {target}: already uses herdr-nvim for Files");
    }
    Ok(())
}

fn uninstall_layouts(home: &Path) -> Result<()> {
    let shown = shown_layout().ok();
    let Ok(entries) = fs::read_dir(home.join("layouts")) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        if path.extension().and_then(|e| e.to_str()) != Some("json") || shipped(home, &name) {
            continue;
        }
        if edit_json(&path, &mut |layout| Ok(layout_patch::restore(layout)))? {
            println!("layout: {name}: Files is the shipped Files browser again");
        }
        let layout: Value = serde_json::from_slice(&fs::read(&path)?)?;
        let Some(origin) = layout.pointer("/cloned_from/name").and_then(Value::as_str) else {
            continue;
        };
        let origin_layout = fs::read(layout_path(home, origin))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        if !name.ends_with("-mods")
            || !origin_layout.is_some_and(|o| layout_patch::same_as_origin(&layout, &o))
        {
            continue;
        }
        if shown.as_deref() == Some(name.as_str()) {
            cli::app_do("layout.switch", &json!({"name": origin}))?;
        }
        fs::remove_file(&path)?;
        let _ = fs::remove_file(home.join("layouts/.state").join(format!("{name}.json")));
        println!("layout: removed {name}, back to {origin}");
    }
    Ok(())
}

fn keys_file(args: &[String]) -> Result<Value> {
    let path = args
        .iter()
        .position(|arg| arg == "--keys")
        .and_then(|at| args.get(at + 1))
        .context("--keys <keys.json> is required")?;
    serde_json::from_str(&fs::read_to_string(path)?)
        .with_context(|| format!("{path}: invalid JSON"))
}

/// `--panel-width N` (logical px), else herdr-nvim's default.
fn panel_width(args: &[String]) -> Result<u32> {
    match args.iter().position(|arg| arg == "--panel-width") {
        None => Ok(DEFAULT_PANEL_PX),
        Some(at) => args
            .get(at + 1)
            .and_then(|w| w.parse::<u32>().ok())
            .filter(|w| (120..=1200).contains(w))
            .context("--panel-width takes a width in px between 120 and 1200"),
    }
}

pub fn setup_cmd() -> Result<()> {
    let args: Vec<String> = env::args().skip(2).collect();
    let home = home();
    let ours = keys_file(&args)?;
    let keys = home.join("keys.json");
    if !keys.exists() {
        fs::write(&keys, "{}\n")?;
    }
    match args.first().map(String::as_str) {
        Some("install") => {
            let mut report = (Vec::new(), Vec::new());
            edit_json(&keys, &mut |value| {
                report = merge_keys(value, &ours);
                Ok(!report.0.is_empty())
            })?;
            report
                .0
                .iter()
                .for_each(|line| println!("keys: added {line}"));
            report
                .1
                .iter()
                .for_each(|line| println!("keys: kept {line}"));
            install_layout(&home, panel_width(&args)?)
        }
        Some("uninstall") => {
            let mut removed = Vec::new();
            edit_json(&keys, &mut |value| {
                removed = unmerge_keys(value, &ours);
                Ok(!removed.is_empty())
            })?;
            removed
                .iter()
                .for_each(|line| println!("keys: removed {line}"));
            uninstall_layouts(&home)
        }
        _ => bail!(
            "usage: herdr-nvim app-setup install|uninstall --keys <keys.json> [--panel-width N]"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_merge_only_into_free_keys_and_unmerge_only_ours() {
        let mut keys = json!({"global": {"cmd-j o": "mod.open:open-opencode", "cmd-t": "tab.new"}});
        let ours = json!({"global": {"cmd-j e": "mod.open:herdr-nvim", "cmd-j o": "mod.open:herdr-nvim-pick"}});
        let (added, conflicts) = merge_keys(&mut keys, &ours);
        assert_eq!(added.len(), 1);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(keys["global"]["cmd-j o"], "mod.open:open-opencode");
        assert_eq!(merge_keys(&mut keys, &ours).0.len(), 0, "idempotent");
        assert_eq!(unmerge_keys(&mut keys, &ours), ["global cmd-j e"]);
        assert_eq!(
            keys,
            json!({"global": {"cmd-j o": "mod.open:open-opencode", "cmd-t": "tab.new"}})
        );
    }
}
