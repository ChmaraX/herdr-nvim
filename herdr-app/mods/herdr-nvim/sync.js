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
//   tab's nvim pane follows the panel. An opening counts even when the
//   look sees it only after a tab switch (the switch itself never opens it). Switching the panel to Changes leaves
//   nvim open (Changes opens files in it).
// - the tab's nvim pane appeared or went (picker, `open`, gitview, `:q`): the
//   panel follows the pane.
// With `tree = "nvim"` the panel never stays: the Files button's show is
// turned into "open (or close) the tab's nvim, with nvim's own explorer".
//
// The tree key (`<leader>e` in nvim, the same keys in the tree) is the one
// exception to "nvim follows the panel": it hides the tree and keeps nvim
// (an intent the hide carries, panel.js), and the tab remembers it: coming
// back to that tab leaves the tree hidden until the panel is shown there
// again (Files, cmd-e, the tree key, cmd-shift-e) or its nvim closes.
//
// It also takes `herdr-nvim.pick` (cmd-o: a file from the agent's output,
// pick.js), `herdr-nvim.reveal` (cmd-shift-e, `<leader>E` in nvim):
// open the pair if needed, then hand the tree the keyboard (`pane.go` right
// from the tab's nvim pane, once the tree has been laid out); and
// `herdr-nvim.toggle` (the tree key in nvim): that, or hide a tree that shows.
//
// Each action makes the other side agree, so the next look finds nothing to
// do: no loops. While an action runs nothing else is decided, and until the
// app's terminal list shows its result, the result is assumed (the list lags
// the command by a moment).

import { focused, layout, nextCommand, notify, panes, run, setShared, shared, spaces } from "herdr";
import { nvim, place } from "./lib.js";
import { appConfig, treeMode } from "./ops.js";
import { pick } from "./pick.js";
import { wanted } from "./sync-rule.js";
import { ended, goToTree, hidePanel, hideTreeKeepNvim, nvimPanes, panelState, previewPane, showFiles, takeKeepNvim } from "./panel.js";

// How long a Preview swap waits for its pane.
const SWAP_GIVE_UP_MS = 5000;

// How long our own open or close is assumed at most before the app's
// terminal list shows it. The list lags by a moment, and a pane closed
// meanwhile by something else (cmd-e with `tree = "nvim"` runs `toggle`
// itself) shows only as ended; without an end its tab would look open for
// good.
const EXPECT_MS = 3000;

// The preview each tab's Files closed: {[tab]: file}.
const MEMORY = "herdr-nvim.preview-memory";

// Sets (or, with null, clears) a tab's entry; tabs that are gone drop out.
function remember(tab, file) {
  const tabs = new Set(spaces().flatMap((s) => s.tabs.map((t) => t.id)));
  const kept = Object.entries(shared(MEMORY) ?? {}).filter(([t]) => t !== tab && tabs.has(t));
  if (file) kept.push([tab, file]);
  setShared(MEMORY, Object.fromEntries(kept));
}

export class Sync {
  constructor(cx) {
    this.kept = new Set(); // tabs whose tree the tree key hid, nvim kept
    this.expects = new Map(); // tab -> what our own open/close will show (nvimOf)
    cx.spawn(async (cx) => {
      await treeMode();
      for (;;) {
        const command = await nextCommand();
        // `reveal:<tab>` / `toggle:<tab>` come from that tab's nvim
        // (`app-reveal`): only for the tab on screen (the panel is the
        // window's), never for one the person has left meanwhile. A swap
        // names its tab and may be for any (`open --tab` of an image).
        const [verb, tab] = command.split(":");
        if (tab && verb !== "swap" && focused()?.tab !== tab) continue;
        if (verb === "pick") pick(place());
        else if (verb === "swap") await this.startSwap(tab, decodeURIComponent(command.split(":").slice(2).join(":")), cx);
        else if (verb === "reveal") await this.reveal(cx);
        else if (verb === "toggle") await this.toggleTree(cx);
      }
    });
  }

  // Whether the tab's nvim pane is open (a preview in its place counts:
  // the controller swaps them, nvim keeps running). Right after our own open
  // or close the app's terminal list may not show it yet: until it shows that
  // terminal there (or gone), the result of the action is assumed.
  // Each tab's own expectation, kept until that tab's lists show it (or
  // EXPECT_MS): a look at another tab meanwhile doesn't drop it.
  nvimOf(tab) {
    const seen = nvimPanes(tab), e = this.expects.get(tab), preview = previewPane(tab);
    if (e && Date.now() - e.since > EXPECT_MS) this.expects.delete(tab);
    else if (e) {
      if (e.opened && !seen.includes(e.opened) && !ended(e.opened)) return true;
      if (e.preview && !preview) return true;
      if (e.closed && seen.includes(e.closed)) return seen.length > 1 || Boolean(preview);
      if (e.closedPreview && preview === e.closedPreview) return seen.length > 0;
      this.expects.delete(tab);
    }
    return seen.length > 0 || Boolean(preview);
  }

  // Each render: what the tab looks like now, what that change asks for
  // (`wanted`), then make it so.
  look(cx) {
    this.focusTree();
    if (this.busy) return;
    if (this.swapping) return this.finishSwap();
    const p = panelState(layout()), f = focused();
    // Nothing with a tab focused (an overlay such as cmd-e's, a dialog): no
    // decision, and the last look stays, so a change made meanwhile (cmd-e's
    // hide) is still read as one when the tab is back.
    if (!p || !f?.tab) return;
    // `at`: the whole tab context, fixed now; actions on `cur` use it after
    // their awaits, whichever tab is focused by then.
    const cur = { tab: f.tab, at: place(f), files: p.files, other: p.other, nvim: this.nvimOf(f.tab) };
    // nvim itself on screen (back by any way, cmd-o, open): no preview for
    // Files to bring back. Not the nvim pane we just closed, which the
    // app's list may still show.
    const closing = this.expects.get(f.tab)?.closed;
    if ((shared(MEMORY) ?? {})[f.tab] && nvimPanes(f.tab).some((t) => t !== closing)) remember(f.tab, null);
    const prev = this.prev;
    this.prev = cur;
    const hidByTreeKey = prev?.tab === cur.tab && prev.files && !cur.files && takeKeepNvim(cur.tab);
    const want = wanted(cur, prev, {
      native: shared("herdr-nvim.tree") !== "nvim",
      local: !spaces().find((s) => s.id === f.space)?.machine,
      kept: this.kept.has(cur.tab),
      hidByTreeKey,
    });
    if (want.kept === true) this.kept.add(cur.tab);
    if (want.kept === false) this.kept.delete(cur.tab);
    // Our own show or hide: nothing for the next look to follow.
    if (want.panel === "show") { showFiles(p); cur.files = true; }
    if (want.panel === "hide") { hidePanel(p); cur.files = false; }
    if (want.nvim) this.act(cur, p, cx);
  }

  // `herdr-nvim.swap:<tab>:<file>` (the pane header's Preview, an image
  // opened any way): `file` rendered in the tab's nvim's place (or in the
  // preview's: another file). The old pane is split across the slot's split
  // (`across`), so both nest in its slot, and closed once the new one is in
  // the tab: the new pane keeps the old one's exact size. nvim keeps running
  // meanwhile (its pane only attaches to the tab's daemon). Until the swap
  // is done, nothing else is decided (`finishSwap`), so the moment with
  // neither pane is not read as nvim closing.
  // `herdr-nvim.swap:<tab>:` (Source): nvim back in the preview's place,
  // which herdr-nvim does (`toggle --state open` swaps it in).
  async startSwap(tab, file, cx) {
    if (this.busy || this.swapping) return;
    // The tab the swap is for (it may not be the one on screen).
    const here = place();
    const at = here?.tab === tab ? here
      : { space: spaces().find((s) => s.tabs.some((t) => t.id === tab))?.id ?? null, tab, terminal: null, cwd: null };
    if (!at.space) return;
    if (!file) {
      if (!previewPane(tab)) return;
      this.busy = true;
      try {
        await this.toggle(at, "open");
      } finally {
        this.busy = false;
        cx.notify();
      }
      return;
    }
    const { across } = await appConfig().catch(() => ({ across: "down" }));
    const terminal = nvimPanes(tab)[0];
    const old = panes().find((p) => p.terminal === terminal)?.pane ?? previewPane(tab);
    // Neither on screen: herdr-nvim puts the preview where nvim's pane goes.
    if (!old) return void (await nvim("app-preview", ["--file", file], at));
    this.swapping = { tab, old, before: this.panesOf(tab), at: Date.now() };
    run("pane.split", { direction: across, pane: old, mod: "herdr-nvim", props: { preview: file, tab, cwd: at.cwd ?? null } });
    // The deadlines below hold even if nothing else redraws meanwhile.
    cx.spawn(async (cx) => {
      await cx.sleep(SWAP_GIVE_UP_MS + 100);
      cx.notify();
    });
    cx.notify();
  }

  panesOf(tab) {
    return new Set(panes().filter((p) => p.tab === tab).map((p) => p.pane));
  }

  // Each render mid-swap: close the old pane once the new preview pane is in
  // the tab (whenever it comes); done once the old one is gone and the
  // preview is listed. A preview that hasn't come in SWAP_GIVE_UP_MS closes
  // nothing.
  finishSwap() {
    const s = this.swapping, waited = Date.now() - s.at;
    const made = panes().find((p) => p.tab === s.tab && !s.before.has(p.pane) && p.terminal === null && p.mod === "herdr-nvim");
    if (!s.closed && made) {
      run("pane.close", { pane: s.old });
      s.closed = true;
    }
    const settled = s.closed && !panes().some((p) => p.pane === s.old);
    if (settled || waited >= SWAP_GIVE_UP_MS) {
      if (!s.closed) notify({ title: "herdr-nvim", body: "The preview did not show up; nothing was closed.", level: "error" });
      this.swapping = null;
      if (this.prev?.tab === s.tab) this.prev = { ...this.prev, nvim: true };
    }
  }

  // The panel opened or closed: open or close the tab's nvim to match. With
  // `tree = "nvim"` the panel is put away and the press toggles nvim alone.
  act(cur, p, cx) {
    this.busy = true;
    cx.spawn(async (cx) => {
      // Whatever fails, the controller goes on: a stuck `busy` would stop
      // every later look (the Files button would do nothing).
      try {
        const mode = await treeMode();
        let state = cur.files ? "open" : "close";
        if (mode === "nvim" && cur.files) {
          hidePanel(p);
          cur.files = false;
          state = cur.nvim ? "close" : "open";
        }
        if (state === "open" ? !cur.nvim : cur.nvim) await this.toggle(cur.at, state);
        cur.nvim = state === "open";
      } catch (e) {
        notify({ title: "herdr-nvim", body: `Could not open or close the tab's nvim: ${e?.message ?? e}`, level: "error" });
      } finally {
        this.busy = false;
        cx.notify();
      }
    });
  }

  // Opens or closes the nvim of the tab `at` (a `place()`), not whichever
  // tab is focused when the command runs. Closing is done here: the mod
  // knows the panes, so no process runs for it. A preview in nvim's place
  // closes too and is remembered for the tab (shared, `MEMORY`): the next
  // open shows it again (`toggle --preview`).
  async toggle(at, state) {
    const memory = shared(MEMORY) ?? {};
    if (state === "close") {
      const terminal = nvimPanes(at.tab)[0] ?? null;
      const pane = terminal && panes().find((p) => p.terminal === terminal)?.pane;
      const preview = previewPane(at.tab);
      if (pane || preview) {
        // What Files closes now is what it opens next: the preview, or nvim.
        remember(at.tab, preview ? (shared("herdr-nvim.previews") ?? {})[at.tab] ?? null : null);
        for (const p of [pane, preview]) if (p) run("pane.close", { pane: p });
        this.expects.set(at.tab, { opened: null, closed: terminal, preview: false, closedPreview: preview, since: Date.now() });
        return { closed: terminal };
      }
    }
    const remembered = state === "open" ? memory[at.tab] : null;
    if (remembered) remember(at.tab, null);
    const out = await nvim("toggle", ["--state", state, ...(remembered ? ["--preview", remembered] : [])], at);
    if (out === null) return null;
    let made = {};
    try { made = JSON.parse(out || "{}"); } catch {}
    this.expects.set(at.tab, {
      opened: made.terminal ?? null,
      closed: made.closed ?? null,
      preview: Boolean(made.preview),
      closedPreview: made.closed_preview ?? null,
      since: Date.now(),
    });
    return made;
  }

  // The tree key in nvim: hide the tree when it shows (nvim stays and keeps
  // the keyboard), else reveal.
  async toggleTree(cx) {
    if ((await treeMode()) === "nvim") return;
    const f = focused(), p = panelState(layout());
    if (!f?.tab || !p || this.busy) return;
    if (!p.files) return this.reveal(cx);
    hideTreeKeepNvim(p, f.tab, nvimPanes(f.tab)[0]);
    cx.notify();
  }

  // cmd-shift-e / `<leader>E`: the tree, on the file nvim shows.
  async reveal(cx) {
    const f = focused(), p = panelState(layout());
    if (!f?.tab || !p || this.busy) return;
    const at = place(f);
    this.kept.delete(f.tab);
    this.busy = true;
    try {
      const mode = await treeMode();
      let terminal = nvimPanes(f.tab)[0];
      // A preview in nvim's place: the tree beside it, the keyboard stays.
      if (!terminal && previewPane(f.tab)) {
        if (!p.files) showFiles(p);
        this.prev = { tab: f.tab, files: true, other: false, nvim: true };
        return;
      }
      if (!terminal) terminal = (await this.toggle(at, "open"))?.terminal;
      if (!terminal) return;
      if (mode === "nvim") return run("pane.focus", { terminal });
      // On nvim's file, on purpose: the tree otherwise keeps the selection
      // where the person left it (an empty path: nvim's current file).
      setShared("herdr-nvim.reveal", { tab: f.tab, path: "", n: (shared("herdr-nvim.reveal")?.n ?? 0) + 1 });
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
    goToTree(want.terminal);
  }
}
