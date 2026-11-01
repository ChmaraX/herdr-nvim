//! The two nodes the Herdr app install puts into a layout, and takes out:
//!
//! * the `files` slot (the side panel's Files browser, shown by the tab bar's
//!   Files button) shows the `herdr-nvim` tree; its old content is kept in
//!   the block's `props.replaces`;
//! * an empty floating block, `herdr-nvim-sync`, the controller that keeps the
//!   Files panel and the tab's nvim pane together. A hidden block's mod is not
//!   rendered, so the slot can't see the panel close; a floating block that
//!   draws nothing is always on screen, at no size.
//!
//! It also narrows the panel (the `files` stack's parent) to `panel_width`
//! unless someone set a width of their own in the file: it changes only the
//! shipped width or the width a previous install set, and keeps the width it
//! replaced in the slot's props for the uninstall.
//!
//! Everything else in the layout, including another mod's patch of the
//! `changes` slot, is left as it is.

use anyhow::{Context, Result};
use serde_json::{json, Value};

pub(crate) const MOD: &str = "herdr-nvim";
pub(crate) const SLOT: &str = "files";
pub(crate) const SYNC: &str = "herdr-nvim-sync";
/// The panel width Herdr 0.1.11 ships in `main` (logical px).
pub(crate) const SHIPPED_PANEL_PX: f64 = 368.0;
/// herdr-nvim's default: ~15% of a 2021px window, as the person sized it.
pub(crate) const DEFAULT_PANEL_PX: u32 = 300;

/// The chain of nodes from `node` to the node `id`.
fn path_to<'a>(node: &'a Value, id: &str, trail: &mut Vec<&'a Value>) -> bool {
    trail.push(node);
    if node.get("id").and_then(Value::as_str) == Some(id) {
        return true;
    }
    for child in node
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if path_to(child, id, trail) {
            return true;
        }
    }
    trail.pop();
    false
}

/// The id of the panel: the parent of the stack holding the slot.
fn panel_id(layout: &Value) -> Option<String> {
    let mut trail = Vec::new();
    if !path_to(layout, SLOT, &mut trail) {
        return None;
    }
    let stack = trail
        .iter()
        .rposition(|n| n.get("type").and_then(Value::as_str) == Some("stack"))?;
    trail
        .get(stack.checked_sub(1)?)?
        .get("id")?
        .as_str()
        .map(str::to_owned)
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

fn px(node: &Value) -> Option<f64> {
    node.pointer("/size/px").and_then(Value::as_f64)
}

/// Pure: install into `layout`. Returns whether anything changed.
pub(crate) fn patch(layout: &mut Value, panel_width: u32) -> Result<bool> {
    let before = layout.clone();
    let panel = panel_id(layout);
    let slot = find_mut(layout, SLOT)
        .context("this layout has no `files` slot (the side panel's Files browser) to replace")?;
    if slot.pointer("/content/mod").and_then(Value::as_str) != Some(MOD) {
        let old = slot.get("content").cloned().unwrap_or(Value::Null);
        slot["content"] = json!({"mod": MOD, "props": {"replaces": old}});
    }
    let ours = slot
        .pointer("/content/props/panel_width")
        .and_then(Value::as_f64);
    let mut was = None;
    if let Some(node) = panel.as_deref().and_then(|id| find_mut(layout, id)) {
        let now = px(node);
        let free = now.is_none() || now == Some(SHIPPED_PANEL_PX) || (now.is_some() && now == ours);
        if free && now != Some(f64::from(panel_width)) {
            was = Some(node.pointer("/size/px").cloned().unwrap_or(Value::Null));
            node["size"] = json!({"px": panel_width});
        }
    }
    if let Some(was) = was {
        let props = &mut find_mut(layout, SLOT).expect("found above")["content"]["props"];
        if props.get("panel_width_was").is_none() {
            props["panel_width_was"] = was;
        }
        props["panel_width"] = json!(panel_width);
    }
    let floating = layout
        .as_object_mut()
        .context("the layout is not an object")?
        .entry("floating")
        .or_insert_with(|| json!([]));
    if let Some(list) = floating.as_array_mut() {
        if !list
            .iter()
            .any(|f| f.get("id").and_then(Value::as_str) == Some(SYNC))
        {
            list.push(
                json!({"id": SYNC, "content": {"mod": MOD, "props": {"role": "sync"}},
                "edge": "bottom", "align": "end"}),
            );
        }
    }
    Ok(*layout != before)
}

/// Pure: take everything `patch` added out again. Returns whether anything
/// changed.
pub(crate) fn restore(layout: &mut Value) -> bool {
    let before = layout.clone();
    let panel = panel_id(layout);
    if let Some(slot) = find_mut(layout, SLOT)
        .filter(|s| s.pointer("/content/mod").and_then(Value::as_str) == Some(MOD))
    {
        let props = slot
            .pointer("/content/props")
            .cloned()
            .unwrap_or(Value::Null);
        slot["content"] = props
            .get("replaces")
            .filter(|old| !old.is_null())
            .cloned()
            .unwrap_or_else(|| json!({"mod": SLOT}));
        if let (Some(ours), Some(node)) = (
            props.get("panel_width").and_then(Value::as_f64),
            panel.as_deref().and_then(|id| find_mut(layout, id)),
        ) {
            if px(node) == Some(ours) {
                match props.get("panel_width_was").filter(|was| was.is_number()) {
                    Some(was) => node["size"] = json!({"px": was}),
                    None => {
                        node.as_object_mut().map(|n| n.remove("size"));
                    }
                }
            }
        }
    }
    if let Some(root) = layout.as_object_mut() {
        if let Some(list) = root.get_mut("floating").and_then(Value::as_array_mut) {
            list.retain(|f| f.get("id").and_then(Value::as_str) != Some(SYNC));
            if list.is_empty() {
                root.remove("floating");
            }
        }
    }
    *layout != before
}

/// Pure: whether a clone is back to being its origin (nothing but the clone
/// record differs).
pub(crate) fn same_as_origin(clone: &Value, origin: &Value) -> bool {
    let mut clone = clone.clone();
    if let Some(root) = clone.as_object_mut() {
        root.remove("cloned_from");
    }
    numbers_as_floats(clone) == numbers_as_floats(origin.clone())
}

/// `368` and `368.0` are the same width: compare every number as a float.
fn numbers_as_floats(value: Value) -> Value {
    match value {
        Value::Number(n) => n.as_f64().map_or(Value::Number(n), |f| json!(f)),
        Value::Array(list) => Value::Array(list.into_iter().map(numbers_as_floats).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, numbers_as_floats(v)))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The side panel of Herdr 0.1.11's `main` layout (368px, hidden), with
    /// another mod's patch already applied to the Changes slot.
    fn layout() -> Value {
        json!({"id": "root", "type": "row", "cloned_from": {"name": "main", "version": "0.1.11"},
            "children": [{"id": "panel", "type": "column", "hidden": true, "size": {"px": 368.0}, "children": [
                {"id": "panel-body", "type": "stack", "active": "files", "children": [
                    {"id": "files", "type": "block", "content": {"mod": "files"}},
                    {"id": "changes", "type": "block", "content": {"mod": "gitview", "props": {"replaces": {"mod": "changes"}}}}]}]}]})
    }

    #[test]
    fn patch_adds_the_tree_the_controller_and_the_width_once() {
        let mut value = layout();
        assert!(patch(&mut value, 300).unwrap());
        assert_eq!(
            value
                .pointer("/children/0/children/0/children/0/content")
                .unwrap(),
            &json!({"mod": "herdr-nvim", "props": {"replaces": {"mod": "files"}, "panel_width_was": 368.0, "panel_width": 300}})
        );
        assert_eq!(value.pointer("/children/0/size/px").unwrap(), 300);
        assert_eq!(value["floating"][0]["id"], SYNC);
        assert_eq!(value["floating"][0]["content"]["props"]["role"], "sync");
        assert_eq!(
            value
                .pointer("/children/0/children/0/children/1/content/mod")
                .unwrap(),
            "gitview",
            "the other slot is someone else's"
        );
        let once = value.clone();
        assert!(!patch(&mut value, 300).unwrap(), "idempotent");
        assert_eq!(value, once);
        assert!(patch(&mut value, 280).unwrap(), "our own width can change");
        assert_eq!(value.pointer("/children/0/size/px").unwrap(), 280);
    }

    #[test]
    fn a_width_someone_else_set_in_the_file_is_kept() {
        let mut value = layout();
        value["children"][0]["size"] = json!({"px": 420.0});
        patch(&mut value, 300).unwrap();
        assert_eq!(value.pointer("/children/0/size/px").unwrap(), 420.0);
        assert!(value
            .pointer("/children/0/children/0/children/0/content/props/panel_width")
            .is_none());
        assert!(restore(&mut value));
        assert_eq!(value.pointer("/children/0/size/px").unwrap(), 420.0);
    }

    #[test]
    fn restore_puts_back_what_was_there_and_clone_detection_sees_others() {
        let mut value = layout();
        patch(&mut value, 300).unwrap();
        assert!(restore(&mut value));
        assert_eq!(value, layout());
        assert!(!restore(&mut value));
        // gitview's slot still differs from main: the clone must stay.
        let mut origin = layout();
        origin.as_object_mut().unwrap().remove("cloned_from");
        origin["children"][0]["size"] = json!({"px": 368}); // as main.json writes it
        assert!(!same_as_origin(&value, &{
            let mut o = origin.clone();
            o["children"][0]["children"][0]["children"][1]["content"] = json!({"mod": "changes"});
            o
        }));
        assert!(same_as_origin(&value, &origin));
    }

    #[test]
    fn an_older_install_upgrades_in_place() {
        // What df33436 wrote: the tree in the slot, no controller, no width.
        let mut value = layout();
        value["children"][0]["children"][0]["children"][0]["content"] =
            json!({"mod": "herdr-nvim", "props": {"replaces": {"mod": "files"}}});
        assert!(patch(&mut value, 300).unwrap());
        assert_eq!(value["floating"][0]["id"], SYNC);
        assert_eq!(value.pointer("/children/0/size/px").unwrap(), 300);
        assert!(restore(&mut value));
        assert_eq!(value, layout());
    }

    #[test]
    fn a_layout_without_a_files_slot_is_refused() {
        let mut value = json!({"id": "root", "type": "row", "children": []});
        assert!(patch(&mut value, 300).is_err());
    }
}
