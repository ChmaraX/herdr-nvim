//! `herdr-nvim app-setup install|uninstall --keys <keys.json>`: the two edits
//! the Herdr app install makes outside the mod folders, and undoes.
//!
//! * **Layout**: the shown layout's `files` slot (the side panel's Files
//!   browser, which the tab bar's Files button shows) gets the `herdr-nvim`
//!   mod; the slot's old content is kept in the block's props and put back on
//!   uninstall. Only that block changes. Herdr's own layouts can't be edited,
//!   so a shipped one is cloned first (`<name>-mods`, shared with other mods
//!   that patch other slots the same way); a personal one is patched in place.
//!   Every write re-reads the file and replaces it whole only if nobody
//!   changed it meanwhile. Uninstall drops a clone that no longer differs from
//!   the layout it was cloned from.
//! * **Keys**: the mod's `keys.json` entries are added where the key is free;
//!   a key already bound to something else is left alone and reported.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};

use super::cli;

const MOD: &str = "herdr-nvim";
const SLOT: &str = "files";

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

fn find_mut<'a>(node: &'a mut Value, id: &str) -> Option<&'a mut Value> {
    if node.get("id").and_then(Value::as_str) == Some(id) {
        return Some(node);
    }
    node.get_mut("children")?
        .as_array_mut()?
        .iter_mut()
        .find_map(|child| find_mut(child, id))
}

fn walk_mut(node: &mut Value, visit: &mut dyn FnMut(&mut Value)) {
    visit(node);
    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            walk_mut(child, visit);
        }
    }
}

/// Pure: put the mod in the `files` slot. `Ok(false)`: it already is there.
pub(crate) fn patch(layout: &mut Value) -> Result<bool> {
    let slot = find_mut(layout, SLOT)
        .context("this layout has no `files` slot (the side panel's Files browser) to replace")?;
    let old = slot.get("content").cloned().unwrap_or(Value::Null);
    if old.get("mod").and_then(Value::as_str) == Some(MOD) {
        return Ok(false);
    }
    slot["content"] = json!({"mod": MOD, "props": {"replaces": old}});
    Ok(true)
}

/// Pure: put back what the mod replaced. `false`: nothing of ours was there.
pub(crate) fn restore(layout: &mut Value) -> bool {
    let mut changed = false;
    walk_mut(layout, &mut |node| {
        if node.pointer("/content/mod").and_then(Value::as_str) != Some(MOD) {
            return;
        }
        let old = node
            .pointer("/content/props/replaces")
            .filter(|old| !old.is_null())
            .cloned()
            .unwrap_or_else(|| json!({"mod": SLOT}));
        node["content"] = old;
        changed = true;
    });
    changed
}

/// Pure: whether a clone is back to being its origin (nothing but the clone
/// record differs).
pub(crate) fn same_as_origin(clone: &Value, origin: &Value) -> bool {
    let mut clone = clone.clone();
    if let Some(root) = clone.as_object_mut() {
        root.remove("cloned_from");
    }
    clone == *origin
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

fn install_layout(home: &Path) -> Result<()> {
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
    if edit_json(&layout_path(home, &target), &mut patch)? {
        println!("layout: {target}: the Files button now shows herdr-nvim's file tree");
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
        if edit_json(&path, &mut |layout| Ok(restore(layout)))? {
            println!("layout: {name}: Files is the shipped Files browser again");
        }
        let layout: Value = serde_json::from_slice(&fs::read(&path)?)?;
        let Some(origin) = layout.pointer("/cloned_from/name").and_then(Value::as_str) else {
            continue;
        };
        let origin_layout = fs::read(layout_path(home, origin))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        if !name.ends_with("-mods") || !origin_layout.is_some_and(|o| same_as_origin(&layout, &o)) {
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
            install_layout(&home)
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
        _ => bail!("usage: herdr-nvim app-setup install|uninstall --keys <keys.json>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The side panel of Herdr 0.1.11's `main` layout, with another mod's
    /// patch already applied to the Changes slot.
    fn layout() -> Value {
        json!({"id": "root", "type": "row", "cloned_from": {"name": "main", "version": "0.1.11"},
            "children": [{"id": "panel", "type": "column", "hidden": true, "children": [
                {"id": "panel-body", "type": "stack", "active": "files", "children": [
                    {"id": "files", "type": "block", "content": {"mod": "files"}},
                    {"id": "changes", "type": "block", "content": {"mod": "gitview", "props": {"replaces": {"mod": "changes"}}}}]}]}]})
    }

    #[test]
    fn patch_touches_only_the_files_slot_and_is_idempotent() {
        let mut value = layout();
        assert!(patch(&mut value).unwrap());
        assert_eq!(
            value
                .pointer("/children/0/children/0/children/0/content")
                .unwrap(),
            &json!({"mod": "herdr-nvim", "props": {"replaces": {"mod": "files"}}})
        );
        assert_eq!(
            value
                .pointer("/children/0/children/0/children/1/content/mod")
                .unwrap(),
            "gitview",
            "the other slot is someone else's"
        );
        let once = value.clone();
        assert!(!patch(&mut value).unwrap());
        assert_eq!(value, once);
    }

    #[test]
    fn restore_puts_back_what_was_there_and_clone_detection_sees_others() {
        let mut value = layout();
        patch(&mut value).unwrap();
        assert!(restore(&mut value));
        assert_eq!(value, layout());
        assert!(!restore(&mut value));
        // gitview's slot still differs from main: the clone must stay.
        let mut origin = layout();
        origin.as_object_mut().unwrap().remove("cloned_from");
        assert!(!same_as_origin(&value, &{
            let mut o = origin.clone();
            o["children"][0]["children"][0]["children"][1]["content"] = json!({"mod": "changes"});
            o
        }));
        assert!(same_as_origin(&value, &origin));
    }

    #[test]
    fn a_layout_without_a_files_slot_is_refused() {
        let mut value = json!({"id": "root", "type": "row", "children": []});
        assert!(patch(&mut value).is_err());
    }

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
