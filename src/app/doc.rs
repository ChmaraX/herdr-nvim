//! The Herdr app's layout document (`doc.get herdr-app` on herdr-core): every
//! space, its tabs and each tab's split tree. It lives in the core, so it can
//! be read while the window is closed; the app owns its shape (an internal
//! format), so parsing is defensive: anything unexpected reads as "unknown",
//! never as "no tabs" (which would reap every daemon).

use serde_json::{json, Value};

use crate::{herdr::TabInfo, state::TabId};

use super::{rpc, tab_id};

/// The document if the core has one with a `spaces` list.
pub(crate) fn read() -> anyhow::Result<Option<Value>> {
    let result = rpc::call("doc.get", json!({"name": "herdr-app"}))?;
    Ok(parse(&result))
}

/// Pure: `doc.get`'s result to the document, or `None` when it was never
/// written (version 0) or has no `spaces` list.
pub(crate) fn parse(result: &Value) -> Option<Value> {
    if result.get("version").and_then(Value::as_u64).unwrap_or(0) == 0 {
        return None;
    }
    let value = result.get("value")?;
    value.get("spaces")?.as_array()?;
    Some(value.clone())
}

fn spaces(doc: &Value) -> &[Value] {
    doc.get("spaces")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn tabs(space: &Value) -> &[Value] {
    space
        .get("tabs")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn non_empty(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Every tab, named as the app shows it: a space nobody named is called after
/// its folder, a tab nobody named by its position.
pub(crate) fn tab_infos(doc: &Value) -> Vec<TabInfo> {
    let mut out = Vec::new();
    for space in spaces(doc) {
        let Some(space_id) = space.get("id").and_then(Value::as_str) else {
            continue;
        };
        let space_label = non_empty(space.get("name")).or_else(|| {
            non_empty(space.get("folder")).map(|folder| {
                folder
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(&folder)
                    .to_owned()
            })
        });
        for (index, tab) in tabs(space).iter().enumerate() {
            let Some(id) = tab.get("id").and_then(Value::as_str) else {
                continue;
            };
            out.push(TabInfo {
                tab_id: TabId::new(tab_id(space_id, id)),
                workspace_label: space_label.clone(),
                tab_label: non_empty(tab.get("name")),
                tab_number: Some(index as u64 + 1),
            });
        }
    }
    out
}

/// The id of the root node of tab `tab` (its raw app id). It starts out as
/// the tab's id but changes as splits come and go.
pub(crate) fn root_of(doc: &Value, tab: &str) -> Option<String> {
    spaces(doc)
        .iter()
        .flat_map(|space| tabs(space))
        .find(|t| t.get("id").and_then(Value::as_str) == Some(tab))?
        .pointer("/root/id")?
        .as_str()
        .map(str::to_owned)
}

/// The space a tab (raw app id) is in.
pub(crate) fn space_of(doc: &Value, tab: &str) -> Option<String> {
    spaces(doc)
        .iter()
        .find(|space| {
            tabs(space)
                .iter()
                .any(|t| t.get("id").and_then(Value::as_str) == Some(tab))
        })?
        .get("id")?
        .as_str()
        .map(str::to_owned)
}

/// The terminals a tab shows, in its tree's order.
pub(crate) fn terminals_of(doc: &Value, tab: &str) -> Vec<String> {
    fn collect(node: &Value, out: &mut Vec<String>) {
        if let Some(t) = node.pointer("/content/terminal").and_then(Value::as_str) {
            out.push(t.to_owned());
        }
        for child in node
            .get("children")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            collect(child, out);
        }
    }
    let mut out = Vec::new();
    if let Some(root) = spaces(doc)
        .iter()
        .flat_map(|space| tabs(space))
        .find(|t| t.get("id").and_then(Value::as_str) == Some(tab))
        .and_then(|t| t.get("root"))
    {
        collect(root, &mut out);
    }
    out
}

/// The folder of tab `tab` (raw app id): its own `folder` if the app gives
/// it one, else its space's.
pub(crate) fn folder_of(doc: &Value, tab: &str) -> Option<String> {
    spaces(doc).iter().find_map(|space| {
        let found = tabs(space)
            .iter()
            .find(|t| t.get("id").and_then(Value::as_str) == Some(tab))?;
        non_empty(found.get("folder")).or_else(|| non_empty(space.get("folder")))
    })
}

/// The pane (block id) showing `terminal`, and its tab's composite id.
pub(crate) fn pane_of(doc: &Value, terminal: &str) -> Option<(String, String)> {
    for space in spaces(doc) {
        let space_id = space.get("id").and_then(Value::as_str)?;
        for tab in tabs(space) {
            let tab_raw = tab.get("id").and_then(Value::as_str)?;
            if let Some(pane) = tab
                .get("root")
                .and_then(|root| find_terminal(root, terminal))
            {
                return Some((pane, tab_id(space_id, tab_raw)));
            }
        }
    }
    None
}

fn find_terminal(node: &Value, terminal: &str) -> Option<String> {
    if node.pointer("/content/terminal").and_then(Value::as_str) == Some(terminal) {
        return node.get("id").and_then(Value::as_str).map(str::to_owned);
    }
    node.get("children")?
        .as_array()?
        .iter()
        .find_map(|child| find_terminal(child, terminal))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What herdr-core 0.1.16 returned for a private app with one space, a
    /// named tab split in two and an unnamed tab.
    fn fixture() -> Value {
        json!({"version": 7, "value": {"spaces": [{
        "id": "spA", "name": "", "folder": "/tmp/herdr-app-nvim/work",
        "tabs": [
            {"id": "tbOne", "name": "", "root": {"id": "tbOne", "type": "row", "children": [
                {"id": "pLeft", "type": "block", "content": {"terminal": "t1"}},
                {"id": "sX", "type": "column", "children": [
                    {"id": "pRight", "type": "block", "content": {"terminal": "t4"}}]}]}},
            {"id": "tbTwo", "name": "agent", "root": {"id": "tbTwo", "type": "row", "children": [
                {"id": "pAgent", "type": "block", "content": {"terminal": "t5"}}]}}
        ]}]}})
    }

    #[test]
    fn tabs_are_named_like_the_app_names_them() {
        let doc = parse(&fixture()).unwrap();
        let tabs = tab_infos(&doc);
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].tab_id.as_str(), "spA:tbOne");
        assert_eq!(tabs[0].workspace_label.as_deref(), Some("work"));
        assert_eq!(tabs[0].tab_label, None);
        assert_eq!(tabs[0].tab_number, Some(1));
        assert_eq!(tabs[1].tab_label.as_deref(), Some("agent"));
    }

    #[test]
    fn a_missing_or_foreign_document_is_unknown_not_empty() {
        assert!(parse(&json!({"version": 0, "value": null})).is_none());
        assert!(parse(&json!({"version": 3, "value": {"something": []}})).is_none());
        let empty = parse(&json!({"version": 9, "value": {"spaces": []}})).unwrap();
        assert!(
            tab_infos(&empty).is_empty(),
            "every space closed is a real answer"
        );
    }

    #[test]
    fn the_root_of_a_tab_is_whatever_node_is_on_top_now() {
        let mut doc = parse(&fixture()).unwrap();
        assert_eq!(root_of(&doc, "tbOne").as_deref(), Some("tbOne"));
        // After a sidebar closes, the split collapses to the remaining child.
        doc["spaces"][0]["tabs"][1]["root"] =
            json!({"id": "pAgent", "type": "block", "content": {"terminal": "t5"}});
        assert_eq!(root_of(&doc, "tbTwo").as_deref(), Some("pAgent"));
        assert_eq!(root_of(&doc, "tbNope"), None);
    }

    #[test]
    fn finds_the_pane_of_a_nested_terminal() {
        let doc = parse(&fixture()).unwrap();
        assert_eq!(
            pane_of(&doc, "t4"),
            Some(("pRight".to_owned(), "spA:tbOne".to_owned()))
        );
        assert_eq!(pane_of(&doc, "t9"), None);
        assert_eq!(space_of(&doc, "tbTwo").as_deref(), Some("spA"));
        assert_eq!(terminals_of(&doc, "tbOne"), ["t1", "t4"]);
    }

    #[test]
    fn a_tab_without_its_own_folder_has_its_space_folder() {
        let mut doc = parse(&fixture()).unwrap();
        assert_eq!(
            folder_of(&doc, "tbOne").as_deref(),
            Some("/tmp/herdr-app-nvim/work")
        );
        doc["spaces"][0]["tabs"][1]["folder"] = json!("/tmp/elsewhere");
        assert_eq!(folder_of(&doc, "tbTwo").as_deref(), Some("/tmp/elsewhere"));
        assert_eq!(folder_of(&doc, "tbNope"), None);
    }
}
