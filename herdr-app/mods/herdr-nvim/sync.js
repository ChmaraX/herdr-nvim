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
// The tree key (`<leader>e` in nvim, the same keys in the tree; both send
// `herdr-nvim.toggle`) is the one exception to "nvim follows the panel": it
// hides the tree and keeps nvim, and the tab remembers it: coming back to
// that tab leaves the tree hidden until the panel is shown there again
// (Files, cmd-e, the tree key, cmd-shift-e) or its nvim closes.
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
//
// Panes are made by herdr-nvim (Rust: `toggle`, `app-preview`); the
// controller only closes them, which needs no process. What it knows of each
// tab is one record (`tabState`).

import { focused, layout, nextCommand, notify, panes, run, setShared, shared, spaces } from "herdr";
import { nvim, place, trace } from "./lib.js";
import { treeMode } from "./ops.js";
import { pick } from "./pick.js";
import { welcome } from "./card.js";
import { wanted } from "./sync-rule.js";
import { ended, goToTree, hidePanel, nvimPanes, panelState, previewPane, showFiles } from "./panel.js";

// How long a preview that went may wait for nvim in its place.
const GRACE_MS = 1000;

// How long a Preview / Source click waits for another action to finish.
const WAIT_MS = 5000;

// How long our own open, close or panel change is assumed at most before
// the app shows it. The lists lag by a moment, and a pane closed meanwhile
// by something else shows only as ended; without an end its tab would look
// open for good. The tree key's hide is its own for as long.
const EXPECT_MS = 3000;

export class Sync {
  constructor(cx) {
    this.tabs = new Map(); // tab id -> tabState
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

  // What the controller knows of a tab:
  // - expect: what our own open, close or swap will show, until the app's
  //   lists show it (`nvimOf`);
  // - kept: the tree key hid its tree, nvim kept (a switch back leaves it so);
  // - keepAt: when the tree key's hide was sent (the hide that follows is it);
  // - memory: the preview Files closed, which the next Files opens again.
  tabState(tab) {
    let t = this.tabs.get(tab);
    if (!t) this.tabs.set(tab, (t = { expect: null, kept: false, keepAt: 0, memory: null }));
    return t;
  }

  // What our own action will show in `tab` (`toggle`'s or `app-preview`'s
  // answer: opened/closed terminal, preview made/closed).
  expect(tab, made) {
    this.tabState(tab).expect = {
      opened: made.terminal ?? null,
      closed: made.closed ?? null,
      preview: Boolean(made.preview),
      closedPreview: made.closed_preview ?? null,
      since: Date.now(),
    };
  }

  // Whether the tab's nvim pane is open (a preview in its place counts:
  // nvim keeps running behind it). Right after our own action the app's
  // lists may not show it yet: until they show its result (or EXPECT_MS),
  // that result is assumed, whatever tab is looked at meanwhile.
  nvimOf(tab) {
    const seen = nvimPanes(tab), t = this.tabState(tab), e = t.expect, preview = previewPane(tab);
    if (e && Date.now() - e.since > EXPECT_MS) t.expect = null;
    else if (e) {
      if (e.opened && !seen.includes(e.opened) && !ended(e.opened)) return true;
      if (e.preview && (!preview || preview === e.closedPreview)) return true;
      if (e.closed && seen.includes(e.closed)) return seen.length > 1 || Boolean(preview);
      if (e.closedPreview && preview === e.closedPreview) return seen.length > 0;
      t.expect = null;
    }
    return seen.length > 0 || Boolean(preview);
  }

  // Each render: what the tab looks like now, what that change asks for
  // (`wanted`), then make it so.
  look(cx) {
    this.focusTree();
    if (this.busy) return;
    const p = panelState(layout()), f = focused();
    // Nothing with a tab focused (an overlay such as cmd-e's, a dialog): no
    // decision, and the last look stays, so a change made meanwhile (cmd-e's
    // hide) is still read as one when the tab is back.
    if (!p || !f?.tab) return;
    // Our own show or hide, not on screen yet (the app applies it a moment
    // later): nothing to decide until it is, or it's EXPECT_MS old. Read
    // before then, it would look like the person's Files press.
    const to = this.panelWant;
    if (to && p.files !== to.files && Date.now() - to.since <= EXPECT_MS) return;
    this.panelWant = null;
    // `at`: the whole tab context, fixed now; actions on `cur` use it after
    // their awaits, whichever tab is focused by then.
    const cur = {
      tab: f.tab, at: place(f), files: p.files, other: p.other,
      nvim: this.nvimOf(f.tab), preview: Boolean(previewPane(f.tab)),
    };
    const t = this.tabState(f.tab);
    // nvim itself on screen (back by any way, cmd-o, open): no preview for
    // Files to bring back. Not the nvim pane we just closed, which the
    // app's list may still show.
    if (t.memory && nvimPanes(f.tab).some((x) => x !== t.expect?.closed)) t.memory = null;
    const prev = this.prev;
    if (prev && prev.tab !== cur.tab) this.forgetGoneTabs();
    // A preview gone with the tree shown, nvim not there: give nvim GRACE_MS
    // to show in its place (a file opened from the tree swaps it in, and the
    // app's list shows the new pane as nvim a moment after the preview
    // closes). Without the wait the panel hides and comes back: a flicker.
    if (prev?.tab === cur.tab && prev.preview && !cur.nvim && cur.files) {
      if (!this.grace) {
        this.grace = Date.now();
        cx.spawn(async (cx) => {
          await cx.sleep(GRACE_MS + 50);
          cx.notify();
        });
      }
      if (Date.now() - this.grace < GRACE_MS) return;
    }
    this.grace = null;
    this.prev = cur;
    const hidByTreeKey = prev?.tab === cur.tab && prev.files && !cur.files && Date.now() - t.keepAt < EXPECT_MS;
    if (prev?.files && !cur.files) t.keepAt = 0;
    const want = wanted(cur, prev, {
      native: shared("herdr-nvim.tree") !== "nvim",
      local: !spaces().find((s) => s.id === f.space)?.machine,
      kept: t.kept,
      hidByTreeKey,
    });
    if (Object.keys(want).length) {
      trace("look", { tab: cur.tab, prev: prev && { tab: prev.tab, files: prev.files, nvim: prev.nvim, preview: prev.preview }, cur: { files: cur.files, nvim: cur.nvim, preview: cur.preview }, want, seen: nvimPanes(cur.tab), expect: t.expect });
    }
    if (want.kept !== undefined) t.kept = want.kept;
    // Our own show or hide: nothing for the next look to follow.
    if (want.panel === "show") { this.panelTo(true, () => showFiles(p)); cur.files = true; }
    if (want.panel === "hide") { this.panelTo(false, () => hidePanel(p)); cur.files = false; }
    if (want.nvim) this.act(cur, p, cx);
  }

  // Shows (true) or hides the panel, and remembers that until the layout
  // shows it (`look`).
  panelTo(files, change) {
    this.panelWant = { files, since: Date.now() };
    change();
  }

  // The tree key's hide: the tree goes, nvim stays and gets the keyboard.
  hideTreeKeepNvim(p, tab) {
    this.tabState(tab).keepAt = Date.now();
    this.panelTo(false, () => hidePanel(p));
    const terminal = nvimPanes(tab)[0];
    if (terminal) run("pane.focus", { terminal });
  }

  // Tabs that are gone take their records with them.
  forgetGoneTabs() {
    const known = new Set(spaces().flatMap((s) => s.tabs.map((t) => t.id)));
    for (const tab of this.tabs.keys()) if (!known.has(tab)) this.tabs.delete(tab);
  }

  // `herdr-nvim.swap:<tab>:<file>` (the pane header's Preview, an image
  // opened any way): `file` rendered in the tab's nvim's place, or in the
  // preview's (`app-preview`). `herdr-nvim.swap:<tab>:` (Source): nvim back
  // in the preview's place (`toggle --state open`). Nothing else is decided
  // meanwhile, and the result is assumed until the lists show it, so the
  // moment with neither pane is not read as nvim closing.
  async startSwap(tab, file, cx) {
    // A click while another action runs waits its turn (a moment, usually).
    for (const until = Date.now() + WAIT_MS; this.busy && Date.now() < until; ) await cx.sleep(100);
    if (this.busy) {
      notify({ title: "herdr-nvim", body: "Still busy with the last action; try Preview / Source again.", level: "warning" });
      return;
    }
    // The tab the swap is for (it may not be the one on screen).
    const here = place();
    const at = here?.tab === tab ? here
      : { space: spaces().find((s) => s.tabs.some((t) => t.id === tab))?.id ?? null, tab, terminal: null, cwd: null };
    if (!at.space || (!file && !previewPane(tab))) return;
    this.busy = true;
    try {
      if (!file) await this.toggle(at, "open");
      else {
        const out = await nvim("app-preview", ["--file", file], at);
        let made = null;
        try { made = out && JSON.parse(out); } catch {}
        trace("previewed", { tab, file, made });
        if (made) this.expect(tab, made);
      }
    } finally {
      this.busy = false;
      cx.notify();
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
          this.panelTo(false, () => hidePanel(p));
          cur.files = false;
          state = cur.nvim ? "close" : "open";
        }
        trace("act", { tab: cur.tab, mode, state, nvim: cur.nvim });
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
  // closes too and is remembered for the tab (`memory`): the next open
  // shows it again (`toggle --preview`).
  async toggle(at, state) {
    const t = this.tabState(at.tab);
    if (state === "close") {
      const terminal = nvimPanes(at.tab)[0] ?? null;
      const pane = terminal && panes().find((p) => p.terminal === terminal)?.pane;
      const preview = previewPane(at.tab);
      if (pane || preview) {
        // What Files closes now is what it opens next: the preview, or nvim.
        t.memory = preview ? (shared("herdr-nvim.previews") ?? {})[at.tab] ?? null : null;
        for (const p of [pane, preview]) if (p) run("pane.close", { pane: p });
        this.expect(at.tab, { closed: terminal, closed_preview: preview });
        return { closed: terminal };
      }
    }
    const remembered = state === "open" ? t.memory : null;
    if (remembered) t.memory = null;
    const out = await nvim("toggle", ["--state", state, ...(remembered ? ["--preview", remembered] : [])], at);
    if (out === null) return null;
    let made = {};
    try { made = JSON.parse(out || "{}"); } catch {}
    trace("toggled", { tab: at.tab, state, made });
    this.expect(at.tab, made);
    // The first nvim pane ever: the welcome, once its pane has the keyboard
    // (`toggle` gave it), so Esc hands it back there.
    if (made.terminal) welcome();
    return made;
  }

  // The tree key in nvim: hide the tree when it shows (nvim stays and keeps
  // the keyboard), else reveal.
  async toggleTree(cx) {
    if ((await treeMode()) === "nvim") return;
    const f = focused(), p = panelState(layout());
    if (!f?.tab || !p || this.busy) return;
    if (!p.files) return this.reveal(cx);
    this.hideTreeKeepNvim(p, f.tab);
    cx.notify();
  }

  // cmd-shift-e / `<leader>E`: the tree, on the file nvim shows.
  async reveal(cx) {
    const f = focused(), p = panelState(layout());
    if (!f?.tab || !p || this.busy) return;
    const at = place(f);
    this.tabState(f.tab).kept = false;
    this.busy = true;
    try {
      const mode = await treeMode();
      let terminal = nvimPanes(f.tab)[0];
      // A preview in nvim's place: the tree beside it, the keyboard stays.
      if (!terminal && previewPane(f.tab)) {
        if (!p.files) this.panelTo(true, () => showFiles(p));
        this.prev = { tab: f.tab, files: true, other: false, nvim: true };
        return;
      }
      if (!terminal) terminal = (await this.toggle(at, "open"))?.terminal;
      if (!terminal) return;
      if (mode === "nvim") return run("pane.focus", { terminal });
      // On nvim's file, on purpose: the tree otherwise keeps the selection
      // where the person left it (an empty path: nvim's current file).
      setShared("herdr-nvim.reveal", { tab: f.tab, path: "", n: (shared("herdr-nvim.reveal")?.n ?? 0) + 1 });
      if (!p.files) this.panelTo(true, () => showFiles(p));
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
