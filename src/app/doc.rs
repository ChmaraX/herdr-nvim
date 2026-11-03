//! The Herdr app's layout document (`doc.get herdr-app` on herdr-core): every
//! space, its tabs and each tab's split tree. It lives in the core, so it can
//! be read while the window is closed; the app owns its shape (an internal
//! format), so parsing is defensive: anything unexpected reads as "unknown",
//! never as "no tabs" (which would reap every daemon).

use serde::Deserialize;
use serde_json::{json, Value};

use crate::{herdr::TabInfo, state::TabId};

use super::{node, rpc, tab_id};

/// The document: only what herdr-nvim reads, every other field ignored. A
/// space without a string `id` or a `tabs` list, or a tab without an `id`,
/// fails the whole parse (unknown, not fewer tabs).
#[derive(Debug, Deserialize)]
pub(crate) struct Doc {
    pub spaces: Vec<Space>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Space {
    pub id: String,
    #[serde(default, deserialize_with = "lenient")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub folder: Option<String>,
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Tab {
    pub id: String,
    #[serde(default, deserialize_with = "lenient")]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    pub folder: Option<String>,
    /// The tab's split tree (`node`).
    #[serde(default)]
    pub root: Value,
}

/// A name or folder: a string, else nothing (never a failed parse).
fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Ok(Value::deserialize(d)?.as_str().map(str::to_owned))
}

/// The document if the core has one in a shape herdr-nvim knows.
pub(crate) fn read() -> anyhow::Result<Option<Doc>> {
    let result = rpc::call("doc.get", json!({"name": "herdr-app"}))?;
    Ok(parse(&result))
}

/// Pure: `doc.get`'s result to the document, or `None` when it was never
/// written (version 0) or isn't in the shape above.
pub(crate) fn parse(result: &Value) -> Option<Doc> {
    if result.get("version").and_then(Value::as_u64).unwrap_or(0) == 0 {
        return None;
    }
    Doc::deserialize(result.get("value")?).ok()
}

fn non_empty(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The tab `tab` (raw app id) and its space.
pub(crate) fn tab<'a>(doc: &'a Doc, tab: &str) -> Option<(&'a Space, &'a Tab)> {
    doc.spaces
        .iter()
        .find_map(|space| Some((space, space.tabs.iter().find(|t| t.id == tab)?)))
}

/// Every tab, named as the app shows it: a space nobody named is called after
/// its folder, a tab nobody named by its position.
pub(crate) fn tab_infos(doc: &Doc) -> Vec<TabInfo> {
    let mut out = Vec::new();
    for space in &doc.spaces {
        let space_label = non_empty(&space.name).or_else(|| {
            non_empty(&space.folder).map(|folder| {
                folder
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .unwrap_or(&folder)
                    .to_owned()
            })
        });
        for (index, t) in space.tabs.iter().enumerate() {
            out.push(TabInfo {
                tab_id: TabId::new(tab_id(&space.id, &t.id)),
                workspace_label: space_label.clone(),
                tab_label: non_empty(&t.name),
                tab_number: Some(index as u64 + 1),
            });
        }
    }
    out
}

/// The id of the root node of tab `tab` (its raw app id). It starts out as
/// the tab's id but changes as splits come and go.
pub(crate) fn root_of(doc: &Doc, raw: &str) -> Option<String> {
    node::id(&tab(doc, raw)?.1.root).map(str::to_owned)
}

/// The space a tab (raw app id) is in.
pub(crate) fn space_of(doc: &Doc, raw: &str) -> Option<String> {
    Some(tab(doc, raw)?.0.id.clone())
}

/// The folder of tab `tab` (raw app id): its own `folder` if the app gives
/// it one, else its space's.
pub(crate) fn folder_of(doc: &Doc, raw: &str) -> Option<String> {
    let (space, t) = tab(doc, raw)?;
    non_empty(&t.folder).or_else(|| non_empty(&space.folder))
}

/// The terminals a tab shows, in its tree's order.
pub(crate) fn terminals_of(doc: &Doc, raw: &str) -> Vec<String> {
    tab(doc, raw)
        .map(|(_, t)| {
            node::walk(&t.root)
                .filter_map(node::terminal)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The pane (block id) showing `terminal`, and its tab's composite id.
pub(crate) fn pane_of(doc: &Doc, terminal: &str) -> Option<(String, String)> {
    doc.spaces.iter().find_map(|space| {
        space.tabs.iter().find_map(|t| {
            let pane = node::walk(&t.root).find(|n| node::terminal(n) == Some(terminal))?;
            Some((node::id(pane)?.to_owned(), tab_id(&space.id, &t.id)))
        })
    })
}

/// The preview pane of tab `raw` (a `herdr-nvim` mod pane with
/// `props.preview`, put in nvim's place by `app-preview`): its block id and
/// the file it shows.
pub(crate) fn preview_of(doc: &Doc, raw: &str) -> Option<(String, String)> {
    let (_, t) = tab(doc, raw)?;
    node::walk(&t.root).find_map(|n| {
        if n.pointer("/content/mod").and_then(Value::as_str) != Some(super::layout_patch::MOD) {
            return None;
        }
        let file = n
            .pointer("/content/props/preview")
            .and_then(Value::as_str)?;
        Some((node::id(n)?.to_owned(), file.to_owned()))
    })
}

/// The terminal the pane (block id) `pane` shows, in any tab.
pub(crate) fn terminal_of_pane(doc: &Doc, pane: &str) -> Option<String> {
    doc.spaces
        .iter()
        .flat_map(|space| &space.tabs)
        .find_map(|t| node::find(&t.root, pane))
        .and_then(node::terminal)
        .map(str::to_owned)
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
        // A space or tab the parse can't read hides no tabs: the whole
        // document is unknown, so nothing is reaped.
        for bad in [
            json!({"spaces": [{"id": "spA"}]}),
            json!({"spaces": [{"id": "spA", "tab_list": []}]}),
            json!({"spaces": [{"tabs": []}]}),
            json!({"spaces": [{"id": 7, "tabs": []}]}),
            json!({"spaces": [{"id": "spA", "tabs": [{"name": "no id"}]}]}),
            json!({"spaces": [{"id": "spA", "tabs": {}}]}),
        ] {
            assert!(
                parse(&json!({"version": 4, "value": bad})).is_none(),
                "{bad}"
            );
        }
        // Names and folders of another type are only missing.
        let odd = parse(&json!({"version": 4, "value": {"spaces": [
            {"id": "spA", "name": 3, "folder": null, "tabs": [{"id": "tbA", "name": {}}]}]}}))
        .unwrap();
        assert_eq!(tab_infos(&odd)[0].tab_label, None);
        let empty = parse(&json!({"version": 9, "value": {"spaces": []}})).unwrap();
        assert!(
            tab_infos(&empty).is_empty(),
            "every space closed is a real answer"
        );
    }

    #[test]
    fn the_root_of_a_tab_is_whatever_node_is_on_top_now() {
        let doc = parse(&fixture()).unwrap();
        assert_eq!(root_of(&doc, "tbOne").as_deref(), Some("tbOne"));
        // After a sidebar closes, the split collapses to the remaining child.
        let mut raw = fixture();
        raw["value"]["spaces"][0]["tabs"][1]["root"] =
            json!({"id": "pAgent", "type": "block", "content": {"terminal": "t5"}});
        let doc = parse(&raw).unwrap();
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
        // Every space is searched (no early return on one).
        let mut raw = fixture();
        raw["value"]["spaces"]
            .as_array_mut()
            .unwrap()
            .insert(0, json!({"id": "spEmpty", "tabs": [{"id": "tbE"}]}));
        let doc = parse(&raw).unwrap();
        assert_eq!(
            pane_of(&doc, "t5"),
            Some(("pAgent".to_owned(), "spA:tbTwo".to_owned()))
        );
        assert_eq!(space_of(&doc, "tbTwo").as_deref(), Some("spA"));
        assert_eq!(terminals_of(&doc, "tbOne"), ["t1", "t4"]);
        assert_eq!(terminal_of_pane(&doc, "pRight").as_deref(), Some("t4"));
        assert_eq!(terminal_of_pane(&doc, "sX"), None);
    }

    #[test]
    fn a_tab_without_its_own_folder_has_its_space_folder() {
        let mut raw = fixture();
        assert_eq!(
            folder_of(&parse(&raw).unwrap(), "tbOne").as_deref(),
            Some("/tmp/herdr-app-nvim/work")
        );
        raw["value"]["spaces"][0]["tabs"][1]["folder"] = json!("/tmp/elsewhere");
        let doc = parse(&raw).unwrap();
        assert_eq!(folder_of(&doc, "tbTwo").as_deref(), Some("/tmp/elsewhere"));
        assert_eq!(folder_of(&doc, "tbNope"), None);
    }
}
