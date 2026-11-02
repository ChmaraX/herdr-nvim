// The controller that makes the Files panel and the tab's nvim pane one
// feature. It lives in an empty floating block (always on screen, no size),
// so it sees every change: a hidden block's mod is not rendered at all.
//
// With `tree = "native"` (herdr-nvim's config), one rule, applied to whatever
// changed last:
// - the shown tab changed (a tab or space switch, or the first look): the
//   panel follows the tab, open where the tab has its nvim pane, closed where
//   it has none. nvim panes are per tab, the panel is per window.
// - the panel opened or closed on the same tab (Files button, cmd-e): the
//   tab's nvim pane follows the panel. Switching the panel to Changes leaves
//   nvim open (Changes opens files in it).
// - the tab's nvim pane appeared or went (picker, `open`, gitview, `:q`): the
//   panel follows the pane.
// With `tree = "nvim"` the panel never stays: the Files button's show is
// turned into "open (or close) the tab's nvim, with nvim's own explorer".
//
// It also takes `herdr-nvim.reveal` (cmd-shift-e, and `<leader>e` in nvim):
// open the pair if needed, then hand the tree the keyboard (`pane.go` right
// from the tab's nvim pane, once the tree has been laid out).
//
// Each action makes the other side agree, so the next look finds nothing to
// do: no loops. While an action runs nothing else is decided, and until the
// app's terminal list shows its result, the result is assumed (the list lags
// the command by a moment).

import { focused, layout, nextCommand, run, shared, spaces, terminals } from "herdr";
import { nvim } from "./lib.js";
import { treeMode } from "./ops.js";
import { hidePanel, panelState, showFiles } from "./panel.js";

// The live herdr-nvim pane terminals of a tab.
function nvimPanes(tab) {
  return terminals().filter((t) => t.state === "live" && t.labels?.["herdr-nvim.sidebar"] === tab).map((t) => t.address);
}

export class Sync {
  constructor(cx) {
    cx.spawn(async (cx) => {
      await treeMode();
      for (;;) {
        const command = await nextCommand();
        if (command.startsWith("reveal")) await this.reveal(cx);
      }
    });
  }

  // Whether the tab's nvim pane is open. Right after our own open or close the
  // app's terminal list may not show it yet: until it shows that terminal
  // there (or gone), the result of the action is assumed.
  nvimOf(tab) {
    const seen = nvimPanes(tab), e = this.expect;
    if (e?.tab === tab) {
      if (e.opened && !seen.includes(e.opened)) return true;
      if (e.closed && seen.includes(e.closed)) return seen.length > 1;
    }
    this.expect = null;
    return seen.length > 0;
  }

  look(cx) {
    this.focusTree();
    if (this.busy) return;
    const p = panelState(layout()), f = focused();
    if (!p || !f?.tab) {
      this.prev = null;
      return;
    }
    const cur = { tab: f.tab, files: p.files, other: p.other, nvim: this.nvimOf(f.tab) };
    const prev = this.prev;
    this.prev = cur;
    const native = shared("herdr-nvim.tree") !== "nvim";
    const followTab = () => {
      if (native && cur.nvim && !cur.files && !cur.other) showFiles(p);
      else if (cur.files && (!cur.nvim || !native)) hidePanel(p);
    };
    if (!prev || prev.tab !== cur.tab) followTab();
    else if (prev.files !== cur.files) {
      const local = !spaces().find((s) => s.id === f.space)?.machine;
      if (cur.files && local) this.act(cur, p, cx);
      else if (!cur.files && !cur.other && cur.nvim && native) this.act(cur, p, cx);
    } else if (prev.nvim !== cur.nvim) followTab();
  }

  // The panel opened or closed: open or close the tab's nvim to match. With
  // `tree = "nvim"` the panel is put away and the press toggles nvim alone.
  act(cur, p, cx) {
    this.busy = true;
    cx.spawn(async (cx) => {
      const mode = await treeMode();
      let state = cur.files ? "open" : "close";
      if (mode === "nvim" && cur.files) {
        hidePanel(p);
        cur.files = false;
        state = cur.nvim ? "close" : "open";
      }
      if (state === "open" ? !cur.nvim : cur.nvim) await this.toggle(cur.tab, state);
      cur.nvim = state === "open";
      this.busy = false;
      cx.notify();
    });
  }

  async toggle(tab, state) {
    const out = await nvim("toggle", ["--state", state]);
    if (out === null) return null;
    const made = JSON.parse(out || "{}");
    this.expect = { tab, opened: made.terminal ?? null, closed: made.closed ?? null };
    return made;
  }

  // cmd-shift-e / `<leader>e`: the tree, on the file nvim shows.
  async reveal(cx) {
    const f = focused(), p = panelState(layout());
    if (!f?.tab || !p || this.busy) return;
    this.busy = true;
    try {
      const mode = await treeMode();
      let terminal = nvimPanes(f.tab)[0];
      if (!terminal) terminal = (await this.toggle(f.tab, "open"))?.terminal;
      if (!terminal) return;
      if (mode === "nvim") return run("pane.focus", { terminal });
      if (!p.files) showFiles(p);
      this.prev = { tab: f.tab, files: true, other: false, nvim: true };
      this.pendingFocus = { terminal, since: shared("herdr-nvim.tree-laid-out") ?? 0, shown: p.files };
    } finally {
      this.busy = false;
      cx.notify();
    }
  }

  // The tree is reached with `pane.go` from the tab's nvim pane, so it must
  // have been laid out first: right away if the panel was already shown, else
  // once the tree reports its first size.
  focusTree() {
    const want = this.pendingFocus;
    if (!want) return;
    if (!want.shown && (shared("herdr-nvim.tree-laid-out") ?? 0) === want.since) return;
    this.pendingFocus = null;
    run("pane.focus", { terminal: want.terminal });
    run("pane.go", { direction: "right" });
  }
}
