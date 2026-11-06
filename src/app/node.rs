//! The app's layout tree, one shape everywhere (a tab's tree in the layout
//! document, a layout file): nodes `{id, type, children, content, ...}`.
//! The walks over it, so the callers are iterator chains.

use serde_json::Value;

pub(crate) fn id(node: &Value) -> Option<&str> {
    node.get("id").and_then(Value::as_str)
}

pub(crate) fn children(node: &Value) -> impl Iterator<Item = &Value> {
    node.get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// `node` and every node under it, depth first, in the tree's order.
pub(crate) fn walk(node: &Value) -> impl Iterator<Item = &Value> {
    let mut stack = vec![node];
    std::iter::from_fn(move || {
        let next = stack.pop()?;
        let at = stack.len();
        stack.extend(children(next));
        stack[at..].reverse();
        Some(next)
    })
}

/// The terminal a block shows, if it shows one.
pub(crate) fn terminal(node: &Value) -> Option<&str> {
    node.pointer("/content/terminal").and_then(Value::as_str)
}

pub(crate) fn find<'a>(node: &'a Value, wanted: &str) -> Option<&'a Value> {
    walk(node).find(|n| id(n) == Some(wanted))
}

pub(crate) fn find_mut<'a>(node: &'a mut Value, wanted: &str) -> Option<&'a mut Value> {
    if id(node) == Some(wanted) {
        return Some(node);
    }
    node.get_mut("children")?
        .as_array_mut()?
        .iter_mut()
        .find_map(|child| find_mut(child, wanted))
}

/// The chain of nodes from `node` down to the first node `is` picks.
pub(crate) fn path_to<'a>(node: &'a Value, is: &dyn Fn(&Value) -> bool) -> Option<Vec<&'a Value>> {
    if is(node) {
        return Some(vec![node]);
    }
    children(node).find_map(|child| {
        let mut trail = path_to(child, is)?;
        trail.insert(0, node);
        Some(trail)
    })
}

/// The child indices from `node` down to the first node `is` picks.
pub(crate) fn index_path(node: &Value, is: &dyn Fn(&Value) -> bool) -> Option<Vec<usize>> {
    if is(node) {
        return Some(Vec::new());
    }
    children(node).enumerate().find_map(|(at, child)| {
        let mut trail = index_path(child, is)?;
        trail.insert(0, at);
        Some(trail)
    })
}

/// Remove the node `wanted` from wherever it is under `node`.
pub(crate) fn remove(node: &mut Value, wanted: &str) -> Option<Value> {
    let list = node.get_mut("children")?.as_array_mut()?;
    if let Some(at) = list.iter().position(|c| id(c) == Some(wanted)) {
        return Some(list.remove(at));
    }
    list.iter_mut().find_map(|child| remove(child, wanted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tree() -> Value {
        json!({"id": "r", "children": [
            {"id": "a", "children": [{"id": "a1", "content": {"terminal": "t1"}}]},
            {"id": "b", "content": {"terminal": "t2"}}]})
    }

    #[test]
    fn walks_depth_first_in_order() {
        let tree = tree();
        let ids: Vec<&str> = walk(&tree).filter_map(id).collect();
        assert_eq!(ids, ["r", "a", "a1", "b"]);
        let terminals: Vec<&str> = walk(&tree).filter_map(terminal).collect();
        assert_eq!(terminals, ["t1", "t2"]);
    }

    #[test]
    fn finds_paths_and_removes() {
        let mut tree = tree();
        let trail = path_to(&tree, &|n| id(n) == Some("a1")).unwrap();
        assert_eq!(
            trail.iter().filter_map(|n| id(n)).collect::<Vec<_>>(),
            ["r", "a", "a1"]
        );
        assert_eq!(
            index_path(&tree, &|n| terminal(n) == Some("t2")),
            Some(vec![1])
        );
        assert_eq!(find(&tree, "b").and_then(terminal), Some("t2"));
        find_mut(&mut tree, "a1").unwrap()["content"] = json!({"mod": "x"});
        assert!(remove(&mut tree, "a1").is_some());
        assert!(find(&tree, "a1").is_none());
    }
}
