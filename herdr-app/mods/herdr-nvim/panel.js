// The side panel that holds herdr-nvim's tree, as `layout()` describes it, and
// the commands the tab bar's Files button runs on it (mods/tabs/main.js).

import { run } from "herdr";

// The nodes from the root to the tree's block (the `files` slot), or null.
function slotPath(node, trail = []) {
  if (!node) return null;
  const here = [...trail, node];
  if (node.content?.mod === "herdr-nvim" && node.content?.props?.role !== "sync") return here;
  for (const child of node.children ?? []) {
    const found = slotPath(child, here);
    if (found) return found;
  }
  return null;
}

// { stack, slot, panel, files, other }: `files` while the panel shows the
// tree, `other` while it shows another child of the stack (Changes). Null in
// a layout without herdr-nvim's slot.
export function panelState(root) {
  const path = slotPath(root);
  if (!path) return null;
  const at = path.findLastIndex((node) => node.type === "stack");
  const stack = path[at], slot = path[path.length - 1];
  const panel = at > 0 ? path[at - 1] : slot;
  const shown = path.slice(0, Math.max(at, 0) + 1).every((node) => !node.hidden);
  return {
    stack: stack?.id ?? null,
    slot: slot.id,
    panel: panel.id,
    files: shown && (!stack || stack.active === slot.id),
    other: shown && Boolean(stack) && stack.active !== slot.id,
  };
}

export function showFiles(p) {
  if (p.stack) run("layout.select", { stack: p.stack, child: p.slot });
  run("layout.show", { id: p.panel });
}

export function hidePanel(p) {
  run("layout.hide", { id: p.panel });
}
