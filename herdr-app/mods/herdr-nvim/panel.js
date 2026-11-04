// The side panel that holds herdr-nvim's tree, as `layout()` describes it, and
// the commands the tab bar's Files button runs on it (mods/tabs/main.js).

import { panes, run, setShared, shared, terminals } from "herdr";

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

// The live herdr-nvim pane terminals of a tab.
export function nvimPanes(tab) {
  return terminals().filter((t) => t.state === "live" && t.labels?.["herdr-nvim.sidebar"] === tab).map((t) => t.address);
}

// Whether the app lists a terminal as ended.
export function ended(address) {
  return terminals().some((t) => t.address === address && t.state !== "live");
}

// The tab's preview pane, if one shows in its nvim's place: a herdr-nvim mod
// pane of the tab. herdr-nvim opens a mod pane only as a preview (always
// with `props.preview`, which is how the Rust side, `doc::preview_of`, finds
// it: `panes()` has no props). For the pair it counts as nvim's pane.
export function previewPane(tab) {
  return panes().find((p) => p.tab === tab && p.terminal === null && p.mod === "herdr-nvim")?.pane ?? null;
}

// Make the tree the app's focused block, the one way the app has: `pane.go`
// right from the tab's nvim pane (the full-height pane next to the panel).
// Only then does the app (0.1.12) move its focus ring off the pane before.
// Always from nvim's pane: `focused()` names a pane even while a block (the
// tree itself) holds the keyboard, and `pane.go` starts from that block.
export function goToTree(terminal) {
  run("pane.focus", { terminal });
  run("pane.go", { direction: "right" });
}

// The tree key (nvim's `<leader>e`, or the same keys in the tree) hides the
// tree but keeps the tab's nvim, while every other way of closing the panel
// closes nvim with it. It says so first: this intent, for the controller
// (sync.js) to find on the very hide that follows, for that tab, and consume.
const KEEP_NVIM = "herdr-nvim.keep-nvim";
// shared() is persisted: an intent whose hide never came must not outlive it.
const KEEP_FOR_MS = 10000;

export function hideTreeKeepNvim(p, tab, terminal) {
  setShared(KEEP_NVIM, { tab, at: Date.now() });
  hidePanel(p);
  if (terminal) run("pane.focus", { terminal });
}

// On a hide in `tab`: whether it was the tree key's. Consumes the intent.
export function takeKeepNvim(tab) {
  const keep = shared(KEEP_NVIM);
  if (!keep) return false;
  setShared(KEEP_NVIM, null);
  return keep.tab === tab && Date.now() - keep.at < KEEP_FOR_MS;
}
