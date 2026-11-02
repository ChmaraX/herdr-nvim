//! herdr-nvim's whole layout footprint (RECIPE.md, "Built from › Layout footprint"), put
//! into any personal layout and taken out again:
//!
//! * the `files` slot (the side panel's Files browser, shown by the tab bar's
//!   Files button) shows the `herdr-nvim` tree; its old content is kept in
//!   the block's `props.replaces`. A layout without a `files` slot gets the
//!   standard side panel (`panel` column → `panel-body` stack → `files`
//!   block) right of its tab block, recorded in `props.added`;
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
//! `changes` slot, is left as it is, in its order.

use std::collections::HashSet;

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use super::node;

pub(crate) const MOD: &str = "herdr-nvim";
pub(crate) const SLOT: &str = "files";
pub(crate) const SYNC: &str = "herdr-nvim-sync";
/// The ids of the standard side panel (Herdr's `main`; the tab bar's Files
/// button names them), used when a layout has no `files` slot of its own.
const PANEL: &str = "panel";
const PANEL_BODY: &str = "panel-body";
/// The row the tab block is wrapped in when its parent is not a row.
const ROW: &str = "herdr-nvim-row";
/// The panel width Herdr 0.1.11 ships in `main` (logical px).
pub(crate) const SHIPPED_PANEL_PX: f64 = 368.0;
/// herdr-nvim's default: ~15% of a 2021px window, as the person sized it.
pub(crate) const DEFAULT_PANEL_PX: u32 = 300;

/// The id of the panel: the parent of the stack holding the slot.
fn panel_id(layout: &Value) -> Option<String> {
    let trail = node::path_to(layout, &|n| node::id(n) == Some(SLOT))?;
    let stack = trail
        .iter()
        .rposition(|n| n.get("type").and_then(Value::as_str) == Some("stack"))?;
    node::id(trail.get(stack.checked_sub(1)?)?).map(str::to_owned)
}

/// Every node id in the layout, floating blocks included.
fn ids(node: &Value, out: &mut HashSet<String>) {
    if let Some(id) = node::id(node) {
        out.insert(id.to_owned());
    }
    for floating in node
        .get("floating")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        ids(floating, out);
    }
    for child in node::children(node) {
        ids(child, out);
    }
}

fn at_path<'a>(mut node: &'a mut Value, path: &[usize]) -> &'a mut Value {
    for &at in path {
        node = &mut node["children"][at];
    }
    node
}

/// Where a node sits among its siblings: what moves to a wrapping row.
const PLACE: [&str; 3] = ["size", "min", "max"];

/// A layout without a `files` slot: add the standard side panel right of its
/// tab block, hidden until Files or cmd-e shows it. The tab block's parent
/// row takes it; a tab block in a column is wrapped in a row first.
fn add_panel(layout: &mut Value, panel_width: u32) -> Result<()> {
    let mut taken = HashSet::new();
    ids(layout, &mut taken);
    let free = |id: &str| {
        if taken.contains(id) {
            format!("herdr-nvim-{id}")
        } else {
            id.to_owned()
        }
    };
    let (panel, stack) = (free(PANEL), free(PANEL_BODY));
    if taken.contains(&panel) || taken.contains(&stack) || taken.contains(ROW) {
        bail!("this layout already uses the ids herdr-nvim's side panel needs ({panel}, {stack}, {ROW})");
    }
    let is_tab = |n: &Value| n.pointer("/content/tab").and_then(Value::as_str) == Some("active");
    let Some(trail) = node::index_path(layout, &is_tab).filter(|trail| !trail.is_empty()) else {
        bail!("this layout has no `files` slot and no tab block (`{{\"tab\": \"active\"}}`) to put the side panel beside");
    };
    let width = f64::from(panel_width);
    let parent_is_row = at_path(layout, &trail[..trail.len() - 1])
        .get("type")
        .and_then(Value::as_str)
        == Some("row");
    let added = if parent_is_row {
        json!({"panel": panel})
    } else {
        json!({"panel": panel, "row": ROW})
    };
    let node = json!({"id": panel, "type": "column", "hidden": true, "size": {"px": panel_width},
        "min": width.min(280.0), "max": width.max(900.0),
        "children": [{"id": stack, "type": "stack", "active": SLOT, "children": [
            {"id": SLOT, "type": "block",
             "content": {"mod": MOD, "props": {"added": added, "panel_width": panel_width}}}]}]});
    let at = *trail.last().expect("not empty");
    let parent = at_path(layout, &trail[..trail.len() - 1]);
    let list = parent["children"].as_array_mut().context("children")?;
    if parent_is_row {
        list.insert(at + 1, node);
    } else {
        let mut tab = list[at].take();
        let mut row = json!({"id": ROW, "type": "row", "gap": 8});
        for key in PLACE {
            if let Some(value) = tab.as_object_mut().and_then(|t| t.shift_remove(key)) {
                row[key] = value;
            }
        }
        row["children"] = json!([tab, node]);
        list[at] = row;
    }
    Ok(())
}

/// Undo `add_panel`: remove the panel, and unwrap the tab block if it was
/// wrapped (and nothing else was put in that row since).
fn remove_panel(layout: &mut Value, added: &Value) {
    if let Some(panel) = added.get("panel").and_then(Value::as_str) {
        node::remove(layout, panel);
    }
    let Some(row_id) = added.get("row").and_then(Value::as_str) else {
        return;
    };
    let Some(row) = node::find_mut(layout, row_id) else {
        return;
    };
    let Some([only]) = row
        .get("children")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    else {
        return;
    };
    let mut tab = only.clone();
    for key in PLACE {
        if let Some(value) = row.get(key) {
            tab[key] = value.clone();
        }
    }
    *row = tab;
}

fn px(node: &Value) -> Option<f64> {
    node.pointer("/size/px").and_then(Value::as_f64)
}

/// Pure: install into `layout`. Returns whether anything changed.
pub(crate) fn patch(layout: &mut Value, panel_width: u32) -> Result<bool> {
    if !layout.is_object() {
        bail!("the layout is not an object");
    }
    let before = layout.clone();
    if node::find_mut(layout, SLOT).is_none() {
        add_panel(layout, panel_width)?;
    }
    let panel = panel_id(layout);
    let slot = node::find_mut(layout, SLOT).context("the `files` slot")?;
    if slot.pointer("/content/mod").and_then(Value::as_str) != Some(MOD) {
        let old = slot.get("content").cloned().unwrap_or(Value::Null);
        slot["content"] = json!({"mod": MOD, "props": {"replaces": old}});
    }
    let ours = slot
        .pointer("/content/props/panel_width")
        .and_then(Value::as_f64);
    let mut was = None;
    if let Some(node) = panel.as_deref().and_then(|id| node::find_mut(layout, id)) {
        let now = px(node);
        let free = now.is_none() || now == Some(SHIPPED_PANEL_PX) || (now.is_some() && now == ours);
        if free && now != Some(f64::from(panel_width)) {
            was = Some(node.pointer("/size/px").cloned().unwrap_or(Value::Null));
            node["size"] = json!({"px": panel_width});
        }
    }
    if let Some(was) = was {
        let props = &mut node::find_mut(layout, SLOT).expect("found above")["content"]["props"];
        if props.get("panel_width_was").is_none() && props.get("added").is_none() {
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
    let props = node::find_mut(layout, SLOT)
        .filter(|s| s.pointer("/content/mod").and_then(Value::as_str) == Some(MOD))
        .map(|slot| {
            slot.pointer("/content/props")
                .cloned()
                .unwrap_or(Value::Null)
        });
    if let Some(props) = props {
        if let Some(added) = props.get("added").filter(|a| a.is_object()) {
            remove_panel(layout, added);
        } else {
            let slot = node::find_mut(layout, SLOT).expect("found above");
            slot["content"] = props
                .get("replaces")
                .filter(|old| !old.is_null())
                .cloned()
                .unwrap_or_else(|| json!({"mod": SLOT}));
            if let (Some(ours), Some(node)) = (
                props.get("panel_width").and_then(Value::as_f64),
                panel.as_deref().and_then(|id| node::find_mut(layout, id)),
            ) {
                if px(node) == Some(ours) {
                    match props.get("panel_width_was").filter(|was| was.is_number()) {
                        Some(was) => node["size"] = json!({"px": was}),
                        None => {
                            node.as_object_mut().map(|n| n.shift_remove("size"));
                        }
                    }
                }
            }
        }
    }
    if let Some(root) = layout.as_object_mut() {
        if let Some(list) = root.get_mut("floating").and_then(Value::as_array_mut) {
            list.retain(|f| f.get("id").and_then(Value::as_str) != Some(SYNC));
            if list.is_empty() {
                root.shift_remove("floating");
            }
        }
    }
    *layout != before
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
    fn restore_puts_back_what_was_there_in_the_same_order() {
        let mut value = layout();
        let text = serde_json::to_string_pretty(&value).unwrap();
        patch(&mut value, 300).unwrap();
        assert!(restore(&mut value));
        assert_eq!(serde_json::to_string_pretty(&value).unwrap(), text);
        assert!(!restore(&mut value));
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

    /// The tab block of a layout, in a row or a column.
    fn tab_block() -> Value {
        json!({"id": "main", "type": "block", "content": {"tab": "active"}})
    }

    #[test]
    fn a_layout_without_a_files_slot_gets_the_side_panel_beside_its_tab() {
        let original = json!({"id": "root", "type": "row", "children": [
            {"id": "list", "type": "block", "content": {"mod": "agents"}}, tab_block(),
            {"id": "right", "type": "block", "content": {"mod": "clock"}}]});
        let mut value = original.clone();
        assert!(patch(&mut value, 300).unwrap());
        let panel = &value["children"][2];
        assert_eq!(panel["id"], "panel");
        assert_eq!(panel["hidden"], true);
        assert_eq!(panel["size"], json!({"px": 300}));
        assert_eq!(panel["children"][0]["id"], "panel-body");
        assert_eq!(panel["children"][0]["active"], "files");
        assert_eq!(
            panel["children"][0]["children"][0]["content"],
            json!({"mod": "herdr-nvim", "props": {"added": {"panel": "panel"}, "panel_width": 300}})
        );
        assert_eq!(panel_id(&value).as_deref(), Some("panel"));
        assert_eq!(value["floating"][0]["id"], SYNC);
        assert!(!patch(&mut value, 300).unwrap(), "idempotent");
        assert!(patch(&mut value, 320).unwrap(), "its width can change");
        assert_eq!(value["children"][2]["size"], json!({"px": 320}));
        assert!(restore(&mut value));
        assert_eq!(
            serde_json::to_string(&value).unwrap(),
            serde_json::to_string(&original).unwrap()
        );
    }

    #[test]
    fn a_tab_block_in_a_column_is_wrapped_in_a_row_and_unwrapped_again() {
        let mut tab = tab_block();
        tab["size"] = json!({"weight": 2});
        let original = json!({"id": "root", "type": "column", "gap": 8, "children": [
            {"id": "tabs", "type": "block", "content": {"mod": "terminals-bar"}}, tab]});
        let mut value = original.clone();
        assert!(patch(&mut value, 300).unwrap());
        let row = &value["children"][1];
        assert_eq!(row["id"], ROW);
        assert_eq!(row["type"], "row");
        assert_eq!(
            row["size"],
            json!({"weight": 2}),
            "the row takes the tab's place"
        );
        assert_eq!(row["children"][0]["id"], "main");
        assert!(row["children"][0].get("size").is_none());
        assert_eq!(row["children"][1]["id"], "panel");
        assert!(restore(&mut value));
        assert_eq!(value, original);
    }

    #[test]
    fn panel_ids_someone_else_uses_are_not_reused() {
        let mut value = json!({"id": "root", "type": "row", "children": [
            tab_block(), {"id": "panel", "type": "block", "content": {"mod": "notes"}}]});
        patch(&mut value, 300).unwrap();
        assert_eq!(value["children"][1]["id"], "herdr-nvim-panel");
        assert_eq!(value["children"][1]["children"][0]["id"], "panel-body");
        assert!(restore(&mut value));
        assert_eq!(value["children"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_layout_without_a_files_slot_or_a_tab_block_is_refused() {
        let mut value = json!({"id": "root", "type": "row", "children": [
            {"id": "x", "type": "block", "content": {"mod": "win98"}}]});
        assert!(patch(&mut value, 300).is_err());
    }
}
