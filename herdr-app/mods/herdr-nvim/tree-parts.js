// Two jobs the tree (tree-view.js) hands off: which root each tab shows
// (TabRoots), and making sure it holds the keyboard after the app hands it
// over (KeyboardHandover).

import { focused } from "herdr";

// After the app hands the tree the keyboard (`"focus"`), how long the tree
// makes sure it holds it: checks every ARRIVAL_STEP_MS, at most ARRIVAL_STEPS.
const ARRIVAL_STEP_MS = 100;
const ARRIVAL_STEPS = 20;

// Where the app's focus is, as far as a mod can see it.
function focusKey() {
  const f = focused();
  return f ? `${f.space}|${f.tab}|${f.pane}|${f.terminal}` : "";
}

// The root the person chose per tab (`.` / ⌫ / a crumb), kept while the
// tab's folder stays the one it was chosen in.
export class TabRoots {
  constructor() {
    this.roots = new Map(); // tab -> {tabRoot, root}
  }

  remember(tab, tabRoot, root) {
    this.roots.set(tab, { tabRoot, root });
  }

  // The root to show for `tab` whose folder is `tabRoot`.
  rootFor(tab, tabRoot) {
    const saved = this.roots.get(tab);
    return saved && saved.tabRoot === tabRoot ? saved.root : tabRoot;
  }
}

export class KeyboardHandover {
  // `handle`: the tree's focus handle; `nvimStamp()`: the tab's nvim's last
  // FocusGained stamp (it changes when nvim takes the keyboard).
  constructor(handle, nvimStamp) {
    this.handle = handle;
    this.nvimStamp = nvimStamp;
    this.arrival = null;
  }

  // The person left (q / Esc / the tree key): stop making sure.
  cancel() {
    this.arrival = null;
  }

  // The app gave this tree the keyboard (`pane.go`, cmd-shift-e, the tree
  // key): take it, and make sure it took. The app draws the tree as focused
  // from that moment, but the keys reach the tree only while its own handle
  // has GPUI's focus, and a handover can be lost (reported from the person's app:
  // highlighted, no keys until a click). So, for this arrival only, the tree
  // takes the keyboard again while it doesn't hold it, and stops as soon as it
  // has held it twice in a row, or the person has gone elsewhere: another
  // pane, tab or space (`focused()` changed), the tab's nvim got the keyboard
  // (its FocusGained stamp changed), they left with q / Esc / the tree key, or
  // another "focus" arrived. Never on render, never after it settled.
  //
  // The cause is the app's: it draws a pane's focus ring before the view
  // that should hold the keyboard does. Fixed upstream after 0.1.12
  // (herdrdev/herdr-app 558cf56, "hide pane focus ring when another view
  // holds keyboard"; herdrdev/herdr-app#362). Remove this retry once a
  // release has that fix.
  arrive(cx) {
    const arrival = (this.arrival = { where: focusKey() });
    try { this.handle.focus(); } catch {}
    cx.spawn(async (cx) => {
      arrival.nvim = await this.nvimStamp();
      let held = 0;
      for (let step = 0; step < ARRIVAL_STEPS && this.arrival === arrival; step++) {
        await cx.sleep(ARRIVAL_STEP_MS);
        if (this.arrival !== arrival || focusKey() !== arrival.where) break;
        if ((await this.nvimStamp()) !== arrival.nvim) break;
        if (this.handle.is_focused()) {
          if (++held >= 2) break;
          continue;
        }
        held = 0;
        try { this.handle.focus(); } catch {}
        cx.notify();
      }
      if (this.arrival === arrival) this.arrival = null;
    });
  }
}
