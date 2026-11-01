// The controller that makes the Files panel and the tab's nvim pane one
// feature. It lives in an empty floating block (always on screen, no size),
// so it sees every change: a hidden block's mod is not rendered at all.
//
// One rule, applied to whatever changed last:
// - the shown tab changed (a tab or space switch, or the first look): the
//   panel follows the tab, open where the tab has its nvim pane, closed where
//   it has none. nvim panes are per tab, the panel is per window.
// - the panel opened or closed on the same tab (Files button, cmd-e): the
//   tab's nvim pane follows the panel. Switching the panel to Changes leaves
//   nvim open (Changes opens files in it).
// - the tab's nvim pane appeared or went (picker, `open`, gitview, `:q`): the
//   panel follows the pane.
// Each action makes the other side agree, so the next look finds nothing to
// do: no loops. While an nvim pane opens or closes nothing else is decided,
// and until the app's terminal list shows the result, the result is assumed
// (the list lags the command by a moment).

import { focused, layout, spaces, terminals } from "herdr";
import { nvim } from "./lib.js";
import { hidePanel, panelState, showFiles } from "./panel.js";

// The live herdr-nvim pane terminals of a tab.
function nvimPanes(tab) {
  return terminals().filter((t) => t.state === "live" && t.labels?.["herdr-nvim.sidebar"] === tab).map((t) => t.address);
}

export class Sync {
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
    if (this.busy) return;
    const p = panelState(layout()), f = focused();
    if (!p || !f?.tab) {
      this.prev = null;
      return;
    }
    const cur = { tab: f.tab, files: p.files, other: p.other, nvim: this.nvimOf(f.tab) };
    const prev = this.prev;
    this.prev = cur;
    const followTab = () => {
      if (cur.nvim && !cur.files && !cur.other) showFiles(p);
      else if (!cur.nvim && cur.files) hidePanel(p);
    };
    if (!prev || prev.tab !== cur.tab) followTab();
    else if (prev.files !== cur.files) {
      const local = !spaces().find((s) => s.id === f.space)?.machine;
      if (cur.files && !cur.nvim && local) this.act(cur, "open", cx);
      else if (!cur.files && !cur.other && cur.nvim) this.act(cur, "close", cx);
    } else if (prev.nvim !== cur.nvim) followTab();
  }

  act(cur, state, cx) {
    this.busy = true;
    cx.spawn(async (cx) => {
      const out = await nvim("toggle", ["--state", state]);
      if (out !== null) {
        const made = JSON.parse(out || "{}");
        this.expect = { tab: cur.tab, opened: made.terminal ?? null, closed: made.closed ?? null };
        cur.nvim = state === "open";
      }
      this.busy = false;
      cx.notify();
    });
  }
}
