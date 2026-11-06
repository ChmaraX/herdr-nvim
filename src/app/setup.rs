//! `herdr-nvim app-setup install|uninstall --keys <keys.json> [--panel-width N]`: the two edits
//! the Herdr app install makes outside the mod folders, and undoes.
//!
//! * **Layout** (`layout_patch`, RECIPE.md "Built from › Layout footprint"): the person's one
//!   personal layout, the one on screen, gets herdr-nvim's tree in its `files`
//!   slot (or the standard side panel when it has none), the floating
//!   `herdr-nvim-sync` controller and the panel width. Herdr's own layouts
//!   can't be edited: on one, the person's existing personal copy of it is
//!   used (`<name>-copy`, else the only one cloned from it), else it is cloned
//!   once with the app's default (`layout.clone {name}` → `<name>-copy`).
//!   Every write re-reads the file and replaces it whole only if nobody changed
//!   it meanwhile. Uninstall takes herdr-nvim's own edits out of every
//!   personal layout and never deletes or switches a layout.
//! * **Keys**: the mod's `keys.json` entries are added where the key is free;
//!   a key the person already uses for something else is left alone and
//!   reported (AGENTS.md: ask before replacing one).

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

/// Actions earlier herdr-nvim installs bound that no longer exist (the
/// separate picker mod, now `herdr-nvim.pick`; the profiler): an install or
/// uninstall removes them, on whatever key they are.
const RETIRED: &[&str] = &[
    "mod.open:herdr-nvim-pick",
    "herdr-nvim.profile",
    "herdr-nvim.profile-mark",
];

fn retired(action: &Value) -> bool {
    action.as_str().is_some_and(|a| RETIRED.contains(&a))
}

/// Pure: remove every binding to a retired action. Returns what went.
pub(crate) fn drop_retired(keys: &mut Value) -> Vec<String> {
    let mut removed = Vec::new();
    for (section, bindings) in keys.as_object_mut().into_iter().flatten() {
        let Some(bindings) = bindings.as_object_mut() else {
            continue;
        };
        bindings.retain(|key, action| {
            let keep = !retired(action);
            if !keep {
                removed.push(format!("{section} {key} ({action}, retired)"));
            }
            keep
        });
    }
    removed
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
                Some(existing) if existing == action => {}
                Some(existing) if !retired(existing) => conflicts.push(format!(
                    "{section} {key} is {existing}; herdr-nvim's {action} is not bound (bind it to a key yourself if you want it)"
                )),
                _ => {
                    target.insert(key.clone(), action.clone());
                    added.push(format!("{section} {key} → {action}"));
                }
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
            if target.get(key) == Some(action) || target.get(key).is_some_and(retired) {
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

/// The person's own layouts: every `layouts/*.json` Herdr doesn't ship.
fn personal_layouts(home: &Path) -> Vec<(String, Value)> {
    let mut found: Vec<(String, Value)> = fs::read_dir(home.join("layouts"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let layout = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
            Some((name, layout))
        })
        .filter(|(name, _)| !shipped(home, name))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

/// Pure: which personal layout to install into while Herdr's own `shown`
/// is on screen. `Ok(Some(name))`: the person's copy of it (`<shown>-copy`,
/// else the only one cloned from it, e.g. an older install's `<shown>-mods`);
/// `Ok(None)`: none yet, clone it.
pub(crate) fn copy_of(shown: &str, personal: &[(String, Value)]) -> Result<Option<String>> {
    let default = format!("{shown}-copy");
    let copies: Vec<&str> = personal
        .iter()
        .filter(|(_, l)| l.pointer("/cloned_from/name").and_then(Value::as_str) == Some(shown))
        .map(|(name, _)| name.as_str())
        .collect();
    if copies.contains(&default.as_str()) {
        return Ok(Some(default));
    }
    match copies.as_slice() {
        [one] => Ok(Some((*one).to_owned())),
        [] if personal.iter().any(|(name, _)| *name == default) => bail!(
            "{default} exists but is not a copy of {shown}: switch to the personal layout you \
             want herdr-nvim in (ctrl-shift-l) and run the install again"
        ),
        [] => Ok(None),
        several => bail!(
            "you have several personal copies of {shown} ({}): switch to the one you want \
             herdr-nvim in (ctrl-shift-l) and run the install again",
            several.join(", ")
        ),
    }
}

/// Everything `install` will need, checked without changing anything: the
/// layout it would edit (the one on screen, else the person's copy of it, or
/// the shipped one a clone would copy) takes herdr-nvim's patch, in memory.
fn check_layout(home: &Path, panel_width: u32) -> Result<()> {
    let shown = shown_layout()?;
    let name = if shipped(home, &shown) {
        copy_of(&shown, &personal_layouts(home))?.unwrap_or_else(|| shown.clone())
    } else {
        shown
    };
    let path = layout_path(home, &name);
    let mut layout: Value = serde_json::from_str(
        &fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?,
    )
    .with_context(|| format!("{} is not JSON", path.display()))?;
    layout_patch::patch(&mut layout, panel_width)
        .with_context(|| format!("herdr-nvim can't go into layout {name}"))?;
    Ok(())
}

fn install_layout(home: &Path, panel_width: u32) -> Result<()> {
    let shown = shown_layout()?;
    let target = if shipped(home, &shown) {
        match copy_of(&shown, &personal_layouts(home))? {
            Some(copy) => {
                cli::app_do("layout.switch", &json!({"name": copy}))?;
                println!("layout: switched to {copy}, your personal copy of {shown} (Herdr's own layouts can't be edited)");
                copy
            }
            None => {
                cli::app_do("layout.clone", &json!({"name": shown}))?;
                let copy = format!("{shown}-copy");
                if !layout_path(home, &copy).exists() {
                    bail!("layout.clone {shown} made no {copy}");
                }
                println!("layout: cloned {shown} as {copy}, your personal layout from now on (Herdr's own layouts can't be edited)");
                copy
            }
        }
    } else {
        shown
    };
    if edit_json(&layout_path(home, &target), &mut |layout| {
        layout_patch::patch(layout, panel_width)
    })? {
        println!("layout: {target}: Files opens herdr-nvim's tree and the tab's nvim together; pane headers carry nvim's Preview / Source button");
    } else {
        println!("layout: {target}: already uses herdr-nvim for Files");
    }
    Ok(())
}

/// Take herdr-nvim's edits out of every personal layout. Layouts themselves
/// stay, and so does the one on screen.
fn uninstall_layouts(home: &Path) -> Result<()> {
    for (name, _) in personal_layouts(home) {
        if edit_json(&layout_path(home, &name), &mut |layout| {
            Ok(layout_patch::restore(layout))
        })? {
            println!("layout: {name}: herdr-nvim's edits taken out (Files is what it was before)");
        }
    }
    Ok(())
}

fn keys_file(args: &[String]) -> Result<Value> {
    let path = cli::flag(args, "--keys")?.context("--keys <keys.json> is required")?;
    serde_json::from_str(&fs::read_to_string(path)?)
        .with_context(|| format!("{path}: invalid JSON"))
}

/// `--panel-width N` (logical px), else herdr-nvim's default.
fn panel_width(args: &[String]) -> Result<u32> {
    match cli::flag(args, "--panel-width")? {
        None => Ok(DEFAULT_PANEL_PX),
        Some(width) => Some(width)
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
        // `install.sh` runs this before it changes anything.
        Some("check") => check_layout(&home, panel_width(&args)?),
        Some("install") => {
            // Checked first: nothing is written when the install can't finish.
            let width = panel_width(&args)?;
            check_layout(&home, width)?;
            let (mut report, mut dropped) = ((Vec::new(), Vec::new()), Vec::new());
            edit_json(&keys, &mut |value| {
                dropped = drop_retired(value);
                report = merge_keys(value, &ours);
                Ok(!report.0.is_empty() || !dropped.is_empty())
            })?;
            dropped
                .iter()
                .for_each(|line| println!("keys: removed {line}"));
            report
                .0
                .iter()
                .for_each(|line| println!("keys: added {line}"));
            report
                .1
                .iter()
                .for_each(|line| println!("keys: kept yours: {line}"));
            install_layout(&home, width)?;
            super::stale::report_after_install();
            match super::sidebar::end_parked(None) {
                Ok(0) => {}
                Ok(n) => println!("nvim: {n} parked nvim pane(s) ended; the next Files shows a new one"),
                Err(error) => eprintln!("herdr-nvim: could not end parked nvim panes: {error:#}"),
            }
            Ok(())
        }
        Some("uninstall") => {
            let mut removed = Vec::new();
            edit_json(&keys, &mut |value| {
                removed = drop_retired(value);
                removed.extend(unmerge_keys(value, &ours));
                Ok(!removed.is_empty())
            })?;
            removed
                .iter()
                .for_each(|line| println!("keys: removed {line}"));
            uninstall_layouts(&home)
        }
        _ => bail!(
            "usage: herdr-nvim app-setup check|install|uninstall --keys <keys.json> [--panel-width N]"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_merge_only_into_free_keys_and_unmerge_only_ours() {
        let mut keys = json!({"global": {"cmd-j o": "mod.open:open-opencode", "cmd-t": "tab.new"}});
        let ours =
            json!({"global": {"cmd-j e": "mod.open:herdr-nvim", "cmd-j o": "herdr-nvim.pick"}});
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

    #[test]
    fn retired_actions_go_on_any_key_and_the_persons_own_keys_stay() {
        let mut keys = json!({
            "global": {"cmd-alt-shift-p": "herdr-nvim.profile", "cmd-l": "herdr-nvim.profile-mark", "cmd-t": "tab.new"},
            "terminal": {"cmd-k": "herdr-nvim.profile"}
        });
        let mut removed = drop_retired(&mut keys);
        removed.sort();
        assert_eq!(removed.len(), 3, "{removed:?}");
        assert_eq!(
            keys,
            json!({"global": {"cmd-t": "tab.new"}, "terminal": {}})
        );
    }

    #[test]
    fn the_old_picker_mod_key_is_ours_to_replace_and_remove() {
        let mut keys = json!({"global": {"cmd-o": "mod.open:herdr-nvim-pick"}});
        let ours = json!({"global": {"cmd-o": "herdr-nvim.pick"}});
        let (added, conflicts) = merge_keys(&mut keys, &ours);
        assert_eq!((added.len(), conflicts.len()), (1, 0));
        assert_eq!(keys["global"]["cmd-o"], "herdr-nvim.pick");
        keys["global"]["cmd-o"] = json!("mod.open:herdr-nvim-pick");
        assert_eq!(unmerge_keys(&mut keys, &ours), ["global cmd-o"]);
    }

    fn copy(name: &str, of: &str) -> (String, Value) {
        (
            name.to_owned(),
            json!({"cloned_from": {"name": of, "version": "0.1.12"}}),
        )
    }

    #[test]
    fn on_a_shipped_layout_the_persons_own_copy_is_used_or_one_is_made() {
        assert_eq!(copy_of("main", &[]).unwrap(), None, "none yet: clone");
        let mine = ("mine".to_owned(), json!({"id": "root"}));
        assert_eq!(copy_of("main", std::slice::from_ref(&mine)).unwrap(), None);
        // An older install's clone is the person's copy now.
        let old = copy("main-mods", "main");
        assert_eq!(
            copy_of("main", &[old.clone(), mine.clone()])
                .unwrap()
                .as_deref(),
            Some("main-mods")
        );
        assert_eq!(
            copy_of("main", &[old.clone(), copy("main-copy", "main")])
                .unwrap()
                .as_deref(),
            Some("main-copy"),
            "the app's default name wins"
        );
        assert_eq!(
            copy_of("terminals", std::slice::from_ref(&old)).unwrap(),
            None
        );
        assert!(
            copy_of("main", &[old, copy("main-2", "main")]).is_err(),
            "several: ask"
        );
        assert!(
            copy_of("main", &[("main-copy".to_owned(), json!({}))]).is_err(),
            "the default name is taken by something else"
        );
    }
}
