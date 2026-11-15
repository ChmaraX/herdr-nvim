// herdr-nvim's file tree in the side panel's `files` slot: the tab's folder
// with git marks, and LazyVim's explorer keys (Snacks defaults). It draws;
// the tab's nvim does every file operation (ops.js), and opening a file shows
// it in the tab's nvim pane. It takes the keyboard from `pane.go` and
// cmd-shift-e (`"focus"`), follows the file nvim shows, and gives the
// keyboard back with q / Esc.
import * as fs from "fs/promises";
import { div, svg } from "gpui-kit";
import { v_flex, h_flex, Scrollbar, VirtualListScrollHandle, v_virtual_list } from "gpui-base";
import { focused, home, layout, nextCommand, notify, panes, readTextFile, run, setShared, shared, spaces, theme, watchFolder } from "herdr";
import { previewKind } from "./preview.js";
import { nvim } from "./lib.js";
import { current, findFiles, fsOp, inside, request, systemOpen, tabOf } from "./ops.js";
import { openDialog } from "./dialog.js";
import { openKeyHelp } from "./keys-help.js";
import { ACTIONS } from "./tree-keys.js";
import { goToTree, nvimPanes, panelState } from "./panel.js";

// The tree key when nvim hasn't said (an nvim from before tree_key): <leader>e
// with LazyVim's leader.
const TREE_KEY = ["space", "e"];
import { GIT_EVERY_MS, TreeModel, dirname } from "./tree-model.js";
import { markOf, normalizePath } from "./tree.js";
import { KeyboardHandover, TabRoots } from "./tree-parts.js";

const HINT = "? keys";

// `/` finds once typing pauses this long, one search at a time.
const FIND_PAUSE_MS = 120;

// The focused tab and its folder. While this tree has the keyboard there is
// no focused pane: the tab's folder stays what it was.
function place(previous) {
  const f = focused();
  const space = spaces().find((s) => s.id === f?.space) ?? spaces().find((s) => s.active);
  const pane = f?.pane ? panes().find((p) => p.pane === f.pane) : null;
  const tabPanes = f?.tab ? panes().filter((p) => p.tab === f.tab) : [];
  const tabCwd = tabPanes.find((p) => p.cwd)?.cwd;
  const rawCwd = pane?.cwd || tabCwd || (f?.tab === previous?.tab ? previous?.cwd : null) || f?.cwd || space?.folder || home();
  const cwd = normalizePath(rawCwd);
  return { space: space?.id ?? null, tab: f?.tab ?? null, cwd, machine: space?.machine ?? null };
}

export class Tree {
  constructor(cx) {
    this.handle = cx.focus_handle();
    this.scroll = VirtualListScrollHandle.new();
    this.scrolledTo = null; // the selected row last scrolled into view (`scrollKey`)
    this.model = new TreeModel();
    this.place = null;
    this.selected = 0;
    this.picked = new Set(); // Tab-selected paths
    this.filter = null; // {query, results, seq}
    this.roots = new TabRoots();
    this.handover = new KeyboardHandover(this.handle, () => this.nvimFocusStamp());
    this.revealed = shared("herdr-nvim.reveal")?.n ?? 0;
    cx.spawn(async (cx) => {
      for (;;) {
        const command = await nextCommand();
        if (command === "focus") {
          // The selection stays where the person left it: nvim's file changes
          // already move it (watchCurrent), and cmd-shift-e / <leader>E reveal
          // on purpose (shared "herdr-nvim.reveal"). Only the first arrival,
          // before the tree knows nvim's file, picks it (else the first row).
          const keep = this.overlayOpen || this.pressing || this.current != null;
          this.overlayOpen = this.pressing = false;
          this.handover.arrive(cx);
          cx.notify();
          this.readTreeKey();
          // Not awaited: the loop stays free for the next "focus".
          if (!keep) cx.spawn(async (cx) => this.revealCurrent(cx, true));
        } else if (command === "click") {
          // e2e: a press, as GPUI makes it (a press on a tracked element
          // focuses it), then the tree's own mouse-down handler.
          this.handle.focus();
          this.pressed(cx);
        }
      }
    });
  }

  // Follow the focused tab. The root is the tab's folder (where the tab's
  // nvim works, as a real path), or one the person chose with `.` / ⌫ for
  // this tab; focusing another pane of the tab never moves it.
  follow(cx) {
    const next = place(this.place);
    const moved = next.tab !== this.place?.tab || next.machine !== this.place?.machine || (!this.rootTab && next.cwd);
    this.place = next;
    if (!moved) return;
    // Until the new tab's state file is known, none is read (the old tab's
    // would put its file and folder in this one).
    this.watching = null;
    if (next.machine || !next.cwd) {
      this.rootTab = next.tab;
      this.tabRoot = null;
      return this.setRoot(null, cx);
    }
    this.rootTab = next.tab;
    cx.spawn(async (cx) => {
      const now = await current(next);
      if (this.place?.tab !== next.tab) return;
      const h = normalizePath(home());
      let candidateRoot = normalizePath(now.tab_root || next.cwd || h);
      if ((!now.file || candidateRoot === h) && next.space) {
        const sp = spaces().find((s) => s.id === next.space) ?? spaces().find((s) => s.active);
        if (sp?.name && sp.name !== "~") {
          const checks = [
            `${h}/Projetos/${sp.name}`,
            `${h}/Projetos/${sp.name.replace(/-/g, "")}`,
            `${h}/${sp.name}`,
            `${h}/${sp.name.replace(/-/g, "")}`,
          ];
          for (const c of checks) {
            if (await fs.exists(c).catch(() => false)) {
              candidateRoot = c;
              break;
            }
          }
        }
      }
      this.adopt(next.tab, candidateRoot, cx);
      this.watchCurrent(next, cx, now);
    });
  }

  // The tab's folder is `tabRoot`: show the root chosen for this tab, unless
  // the tab's folder changed since (then the folder itself).
  adopt(tab, tabRoot, cx) {
    const norm = normalizePath(tabRoot);
    if (this.rootTab !== tab || (this.shownTab === tab && this.tabRoot === norm)) return;
    this.shownTab = tab;
    this.tabRoot = norm;
    this.setRoot(this.roots.rootFor(tab, norm), cx);
    cx.notify();
  }

  // The view is gone (main.js `close`): its watch loops end.
  close() {
    this.rootGen = (this.rootGen ?? 0) + 1;
    this.currentGen = (this.currentGen ?? 0) + 1;
  }

  // The tab, with its folder as the tree knows it: what operations get.
  where() {
    return { space: this.place?.space, tab: this.place?.tab, cwd: this.tabRoot ?? this.model.root };
  }

  // Shows `root` and watches it. Each root gets one watch loop, ended by the
  // next setRoot (`rootGen`); the same root again (two tabs on one repo, or
  // back to a tab) keeps the loop, the listing and the selection.
  setRoot(root, cx) {
    const norm = normalizePath(root);
    if (this.rootTab && norm) this.roots.remember(this.rootTab, this.tabRoot, norm);
    if (norm && norm === this.model.root && !this.error) return;
    const gen = (this.rootGen = (this.rootGen ?? 0) + 1);
    const keep = { hidden: this.model.showHidden, ignored: this.model.showIgnored };
    this.model.reset(norm);
    Object.assign(this.model, { showHidden: keep.hidden, showIgnored: keep.ignored });
    this.selected = 0;
    this.picked.clear();
    this.filter = null;
    this.error = null;
    if (!norm) return;
    cx.spawn(async (cx) => {
      try {
        await this.model.reload();
        cx.notify();
        while (this.rootGen === gen) {
          const changed = await watchFolder(norm);
          if (this.rootGen !== gen) return;
          // Hidden (Files closed): no listing or git status for every change
          // (a build makes many); it catches up once when shown (`render`).
          if (!panelState(layout())?.files) {
            this.stale = true;
            continue;
          }
          const { marks } = await this.model.reloadChanged(changed);
          if (marks === "none") continue; // nothing the tree shows changed
          cx.notify();
          if (marks === "later" && !this.marksDue) {
            this.marksDue = true;
            cx.spawn(async (cx) => {
              await cx.sleep(GIT_EVERY_MS);
              this.marksDue = false;
              if (this.rootGen !== gen) return;
              await this.model.refreshMarks();
              cx.notify();
            });
          }
        }
      } catch (e) {
        if (this.rootGen === gen) { this.error = String(e); cx.notify(); }
      }
    });
    if (!this.defaults) {
      this.defaults = true;
      cx.spawn(async (cx) => {
        // Quiet: the tree only showed; with no Neovim, the card waits for a
        // press that wants nvim.
        const d = await fsOp(this.where(), request.defaults(), { quiet: true });
        if (d) Object.assign(this.model, { showHidden: d.hidden, showIgnored: d.ignored });
        cx.notify();
      });
    }
  }

  // The file the tab's nvim shows: selected (and its folders opened) whenever
  // it changes, without taking the keyboard. One loop at a time: the next
  // call ends this one (`currentGen`).
  watchCurrent(where, cx, known = null) {
    const gen = (this.currentGen = (this.currentGen ?? 0) + 1);
    cx.spawn(async (cx) => {
      // `known`: what `follow` just asked for this tab (no second process).
      const { file, watch } = known ?? (await current(where));
      if (this.currentGen !== gen) return;
      this.watching = watch;
      this.watchingTab = where.tab;
      this.readTreeKey();
      if (file) await this.reveal(file, cx);
      if (!watch) return;
      // The folder holds every tab's file: only this tab's wakes the tree
      // (a batch naming the folder itself may be anything), and it redraws
      // only for a new file or a new FocusGained.
      let seen = await this.nvimState();
      while (this.currentGen === gen) {
        const changed = await watchFolder(dirname(watch));
        if (this.currentGen !== gen) return;
        // Its own file, or the temp file nvim renames over it.
        if (changed.length && !changed.some((p) => p.startsWith(watch) || p === dirname(watch))) continue;
        const now = await this.nvimState();
        if (now.file && now.file !== seen.file) await this.reveal(now.file, cx);
        // nvim took the keyboard, maybe: draw the focus again (a file
        // outside the root reveals nothing, so this is its own check).
        if (now.focus !== seen.focus) cx.notify();
        seen = now;
      }
    });
  }

  // What the tab's nvim tells the tree, one JSON file (lua/herdr-nvim/app.lua
  // `write_state`): {file, cwd, tree_key, leader, focus}; {} before it wrote.
  async nvimState() {
    // Only the shown tab's file: right after a switch the old one is no
    // answer for this tab.
    const ours = this.watching && this.watchingTab === this.place?.tab;
    const text = ours ? await readTextFile(this.watching).catch(() => null) : null;
    try { return (text && JSON.parse(text)) || {}; } catch { return {}; }
  }

  // What the tab's nvim wrote on its last FocusGained.
  async nvimFocusStamp() {
    return (await this.nvimState()).focus ?? null;
  }

  // nvim's file; arriving with the keyboard and no file of nvim's to show,
  // the first row.
  async revealCurrent(cx, arriving = false) {
    if (!this.place?.cwd) return;
    const seq = (this.revealSeq = (this.revealSeq ?? 0) + 1);
    // Once the tree watches the tab's state file, it holds both (real paths,
    // app.lua `write_state`): no process. Before, herdr-nvim answers.
    const state = this.watching && this.watchingTab === this.place.tab ? await this.nvimState() : null;
    const { file, tab_root } = state?.cwd ? { file: state.file ?? null, tab_root: state.cwd } : await current(this.place);
    if (seq !== this.revealSeq) return;
    if (tab_root && tab_root !== this.tabRoot) this.adopt(this.rootTab, tab_root, cx);
    if (file && inside(file, this.model.root ?? "")) await this.reveal(file, cx);
    else if (arriving) {
      this.selected = 0;
      this.revealing = null;
      cx.notify();
    }
  }

  // Selects `path`, opening its folders (after listing everything again,
  // with `reload`); if it isn't listed yet (just created), render selects it
  // once it is. The latest reveal wins, whichever finishes first.
  async reveal(path, cx, reload = false) {
    if (!path || !inside(path, this.model.root ?? "")) return;
    const seq = (this.revealSeq = (this.revealSeq ?? 0) + 1);
    if (reload) await this.model.reload().catch(() => {});
    await this.model.expandTo(path);
    if (seq !== this.revealSeq) return;
    this.current = path;
    this.revealing = path;
    cx.notify();
  }

  rows() {
    if (this.filter) return this.filter.results.map((rel) => ({ name: rel, abs: `${this.filter.root}/${rel}`, dir: false, depth: 0 }));
    return this.model.rows();
  }

  // The folder an action works in: the row's own folder, or its parent.
  dirOf(row) {
    return row ? (row.dir ? row.abs : dirname(row.abs)) : this.model.root;
  }

  targets(row) {
    return this.picked.size ? [...this.picked] : row ? [row.abs] : [];
  }

  dialog(spec) {
    this.overlayOpen = true;
    openDialog({ place: this.where(), ...spec });
  }

  async openRow(row, cx) {
    if (!row) return;
    if (row.dir) {
      await this.model.toggle(row.abs).catch((e) => notify({ title: "Cannot open folder", body: String(e), level: "error" }));
      return cx.notify();
    }
    this.busy = true;
    cx.notify();
    // nvim can't show an image: the preview takes nvim's place instead (the
    // controller swaps them, sync.js `startSwap`).
    const where = tabOf(this.where());
    if (previewKind(row.abs) === "image") run(`herdr-nvim.swap:${where.tab}:${encodeURIComponent(row.abs)}`);
    else {
      // The mod gives nvim the keyboard: no app round trips in herdr-nvim.
      const out = await nvim("open", ["--no-focus", row.abs], where);
      let terminal = null;
      try { terminal = out && JSON.parse(out).terminal; } catch {}
      if (terminal) run("pane.focus", { terminal });
    }
    this.busy = false;
    cx.notify();
  }

  // Snacks' `h`: close the open folder, else the one the row is in, and stand on it.
  closeRow(row, cx) {
    if (!row) return;
    const folder = row.dir && this.model.open.has(row.abs) ? row.abs : dirname(row.abs);
    if (folder === this.model.root) return;
    this.model.close(folder);
    const at = this.model.rows().findIndex((r) => r.abs === folder);
    if (at >= 0) this.selected = at;
    cx.notify();
  }

  action(name, row, cx) {
    const rel = (path) => path.slice(this.model.root.length + 1);
    switch (name) {
      case "add":
        return this.dialog({ kind: "input", title: "New file or folder", hint: `In ${rel(this.dirOf(row)) || "."}/ — end with / for a folder`,
          action: { type: "create", dir: this.dirOf(row) } });
      case "rename":
        if (row) this.dialog({ kind: "input", title: "Rename", value: rel(row.abs), hint: "A path in this folder; the tab's nvim updates its buffers and language servers",
          action: { type: "rename", from: row.abs, base: this.model.root } });
        return;
      case "move": {
        if (!this.picked.size) return this.action("rename", row, cx);
        const dir = this.dirOf(row), paths = [...this.picked];
        this.picked.clear();
        return this.dialog({ kind: "confirm", title: `Move ${paths.length} item${paths.length > 1 ? "s" : ""} to ${rel(dir) || "."}/?`,
          detail: paths.map(rel).join(", "), action: { type: "request", request: request.move(paths, dir) } });
      }
      case "copy":
        if (this.picked.size) {
          const paths = [...this.picked];
          this.picked.clear();
          return this.dialog({ kind: "confirm", title: `Copy ${paths.length} item${paths.length > 1 ? "s" : ""} to ${rel(this.dirOf(row)) || "."}/?`,
            detail: paths.map(rel).join(", "), action: { type: "request", request: request.copyInto(paths, this.dirOf(row)) } });
        }
        if (row) this.dialog({ kind: "input", title: "Copy to", value: row.name, hint: `In ${rel(dirname(row.abs)) || "."}/`,
          action: { type: "copy", from: row.abs, base: dirname(row.abs) } });
        return;
      case "delete": {
        const paths = this.targets(row);
        this.picked.clear();
        if (paths.length) this.dialog({ kind: "confirm", title: `Delete ${paths.length === 1 ? rel(paths[0]) : `${paths.length} items`}?`,
          detail: "Their buffers in the tab's nvim close too.", action: { type: "request", request: request.delete(paths) } });
        return;
      }
      case "yank": {
        const paths = this.targets(row);
        this.picked.clear();
        return cx.spawn(async (cx) => {
          const r = await fsOp(this.where(), request.yank(paths));
          if (r) notify({ title: `Yanked ${r.count} item${r.count === 1 ? "" : "s"}`, level: "info", timeout: 2500 });
          cx.notify();
        });
      }
      case "paste": {
        const dir = this.dirOf(row);
        if (!inside(dir, this.where().cwd)) {
          return this.dialog({ kind: "confirm", title: "Paste outside this tab's folder?", detail: dir,
            action: { type: "request", request: { ...request.paste(dir), outside: true } } });
        }
        return cx.spawn(async () => { await fsOp(this.where(), request.paste(dir)); });
      }
    }
  }

  // A mouse press anywhere in the tree (the e2e sends `herdr-nvim.click`).
  // GPUI has already given the tree the keyboard, but the app still counts
  // the pane before as focused and keeps its focus ring on it: it moves the
  // ring only when the app itself gives a mod the keyboard (`pane.go`). So a
  // press that brings the keyboard here asks the app for it that way.
  pressed(cx) {
    const terminal = this.drawnFocused ? null : nvimPanes(this.place?.tab)[0];
    if (!terminal) return;
    this.pressing = true; // the app's "focus" that follows keeps the selection
    goToTree(terminal);
    cx.notify();
  }

  // The tree key's presses, as the tab's nvim wrote them (with its leader).
  async readTreeKey() {
    const keys = (await this.nvimState()).tree_key;
    this.treeKey = Array.isArray(keys) ? keys : TREE_KEY;
  }

  // Whether `k` is part of the tree key (a sequence, like space then e);
  // the whole of it hides the tree and gives nvim the keyboard, nvim staying.
  treeKeyPress(k) {
    const want = this.treeKey ?? TREE_KEY;
    const prefix = (seq) => seq.length <= want.length && seq.every((x, i) => x === want[i]);
    let seq = [...(this.pending ?? []), k];
    if (!prefix(seq)) seq = [k];
    this.pending = null;
    if (!want.length || !prefix(seq)) return false;
    if (seq.length < want.length) this.pending = seq;
    else this.hideToNvim();
    return true;
  }

  hideToNvim() {
    this.handover.cancel();
    const tab = this.place?.tab, p = panelState(layout());
    // The controller hides it, keeping nvim (sync.js `toggleTree`).
    if (tab && p?.files) run(`herdr-nvim.toggle:${tab}`);
  }

  key(e, rows, cx) {
    if (!Number.isInteger(this.selected)) this.selected = 0;
    if (this.filter) return this.filterKey(e, rows, cx);
    const plain = !e.modifiers.platform && !e.modifiers.control && !e.modifiers.alt;
    const k = plain && e.key_char && e.key_char.trim() ? e.key_char : e.keystroke;
    if (this.treeKeyPress(k)) {
      cx.stop_propagation();
      return cx.notify();
    }
    const action = ACTIONS.get(k);
    if (!action) return;
    this.actions[action](rows[this.selected], rows, cx);
    cx.stop_propagation();
    cx.notify();
  }

  // What each key does (tree-keys.js names them).
  get actions() {
    const move = (rows, to) => {
      this.revealSeq = (this.revealSeq ?? 0) + 1; // a reveal still on its way doesn't undo the move
      this.selected = Math.max(0, Math.min(rows.length - 1, to));
    };
    return {
      down: (_row, rows) => { this.revealing = null; move(rows, this.selected + 1); },
      up: (_row, rows) => { this.revealing = null; move(rows, this.selected - 1); },
      first: (_row, rows) => move(rows, 0),
      last: (_row, rows) => move(rows, rows.length - 1),
      open: (row, _rows, cx) => cx.spawn(async (cx) => this.openRow(row, cx)),
      close: (row, _rows, cx) => this.closeRow(row, cx),
      pick: (row, rows) => {
        if (row) { this.picked.has(row.abs) ? this.picked.delete(row.abs) : this.picked.add(row.abs); move(rows, this.selected + 1); }
      },
      add: (row, _rows, cx) => this.action("add", row, cx),
      rename: (row, _rows, cx) => this.action("rename", row, cx),
      move: (row, _rows, cx) => this.action("move", row, cx),
      copy: (row, _rows, cx) => this.action("copy", row, cx),
      delete: (row, _rows, cx) => this.action("delete", row, cx),
      yank: (row, _rows, cx) => this.action("yank", row, cx),
      paste: (row, _rows, cx) => this.action("paste", row, cx),
      systemOpen: (row) => { if (row) systemOpen(row.abs); },
      hidden: () => { this.model.showHidden = !this.model.showHidden; },
      ignored: () => { this.model.showIgnored = !this.model.showIgnored; },
      collapse: () => this.model.collapseAll(),
      reload: (_row, _rows, cx) => cx.spawn(async (cx) => { await this.model.reload(); cx.notify(); }),
      rootHere: (row, _rows, cx) => { if (row) this.setRoot(this.dirOf(row), cx); },
      rootUp: (_row, _rows, cx) => { if (this.model.root !== "/") this.setRoot(dirname(this.model.root), cx); },
      rootHome: (_row, _rows, cx) => { if (this.tabRoot) this.setRoot(this.tabRoot, cx); },
      help: () => { this.overlayOpen = true; openKeyHelp(this.treeKey ?? TREE_KEY); },
      find: () => { this.filter = { query: "", results: [], seq: 0, root: this.findRoot() }; this.selected = 0; },
      leave: () => { this.handover.cancel(); run("pane.go", { direction: "left" }); },
    };
  }

  // Typing finds files anywhere under the root (fff); ⏎ opens, Esc returns to the tree.
  filterKey(e, rows, cx) {
    const f = this.filter;
    const k = e.keystroke;
    if (k === "escape") this.filter = null;
    else if (k === "enter") {
      const row = rows[this.selected];
      this.filter = null;
      if (row) cx.spawn(async (cx) => { await this.reveal(row.abs, cx); await this.openRow(row, cx); });
    } else if (k === "down" || k === "ctrl-n") this.selected = Math.min(rows.length - 1, this.selected + 1);
    else if (k === "up" || k === "ctrl-p") this.selected = Math.max(0, this.selected - 1);
    else {
      if (k === "backspace") f.query = f.query.slice(0, -1);
      else if (e.key_char && !e.modifiers.platform && !e.modifiers.control) f.query += e.key_char;
      else return;
      const seq = ++f.seq;
      cx.spawn(async (cx) => {
        await cx.sleep(FIND_PAUSE_MS);
        if (this.filter === f && f.seq === seq) await this.runFind(f, cx);
      });
    }
    cx.stop_propagation();
    cx.notify();
  }

  // Where `/` searches: the root, but never above the tab's folder (⌫ may
  // have climbed to $HOME or /, a whole-disk index).
  findRoot() {
    const root = this.model.root, home = this.tabRoot;
    return home && !inside(root, home) && inside(home, root) ? home : root;
  }

  // One search at a time: keys typed while one runs make one more, for the
  // latest query, when it ends; only the latest query's results show.
  async runFind(f, cx) {
    if (f.busy) { f.again = true; return; }
    f.busy = true;
    do {
      f.again = false;
      const query = f.query;
      const results = query ? await findFiles(f.root, query) : [];
      if (this.filter !== f) return;
      if (!f.again) { f.results = results; this.selected = 0; cx.notify(); }
    } while (f.again);
    f.busy = false;
  }

  // "Files", the tab's folder, and while the root is elsewhere the way back:
  // each step a click (or ~ / ⌫).
  header(t) {
    const badge = (on, label) => div().px(4).rounded(4).text_size(10).text_color(on ? t.foreground : t.dim).bg(on ? t.selected : t.background).child(label);
    const root = this.model.root, home = this.tabRoot;
    const crumb = (id, label, path, current) => div().id(id).flex_none().max_w(140).truncate().text_size(12)
      .text_color(current ? t.foreground : t.accent).when(!current, (el) => el.cursor_pointer().hover((s) => s.text_color(t.foreground))
        .on_click((_e, cx) => this.setRoot(path, cx)))
      .child(label);
    const crumbs = [];
    if (root && home && root !== home) {
      if (inside(root, home)) {
        crumbs.push(crumb("crumb-home", home.split("/").pop(), home, false));
        let path = home;
        for (const part of root.slice(home.length + 1).split("/")) {
          path = `${path}/${part}`;
          crumbs.push(div().flex_none().text_size(12).text_color(t.dim).child("/"));
          crumbs.push(crumb(`crumb-${path}`, part, path, path === root));
        }
      } else {
        crumbs.push(crumb("crumb-root", root, root, true));
        crumbs.push(crumb("crumb-home", `↩ ${home.split("/").pop()}`, home, false));
      }
    }
    return h_flex().h(42).flex_none().px(14).gap(6).items_center().border_b(1).border_color(t.border)
      .child(div().flex_none().font_semibold().child("Files"))
      .child(h_flex().id("crumbs").flex_1().min_w(0).gap(4).items_center().overflow_hidden()
        .when(!crumbs.length, (el) => el.child(div().truncate().text_size(12).text_color(t.dim).child(root?.split("/").pop() ?? "")))
        .children(crumbs))
      .when(this.busy, (el) => el.child(svg("mods/workspace-chrome/spinner.svg").size(12).flex_none().text_color(t.muted).spin(900)))
      .child(badge(this.model.showHidden, "H")).child(badge(this.model.showIgnored, "I"));
  }

  filterBar(t) {
    return h_flex().h(30).flex_none().mx(6).mt(6).px(8).gap(6).items_center().rounded(7).border_1().border_color(t.accent).bg(t.surface)
      .child(div().text_color(t.accent).child("/"))
      .child(div().flex_1().min_w(0).truncate().child(this.filter.query))
      .child(div().text_size(11).text_color(t.dim).child(`${this.filter.results.length}`));
  }

  row(r, i, t, focusedNow) {
    const mark = markOf(this.model.marks, r.abs);
    const markColor = mark === "A" ? t.ansi2 : mark === "D" || mark === "U" ? t.ansi1 : t.blocked;
    // No handlers here: a virtual list's rows take clicks through the list
    // (`clicked`).
    return h_flex().id(`row-${r.abs}`).h(28).flex_none().pl(8 + r.depth * 18).pr(8).gap(7).items_center().rounded(7).cursor_pointer()
      .when(i === this.selected, (el) => el.bg(focusedNow ? t.selected : t.hover))
      .hover((s) => s.bg(t.hover))
      .child(r.dir ? svg(`mods/files/chevron-${this.model.open.has(r.abs) ? "down" : "right"}.svg`).size(12).flex_none().text_color(t.dim) : div().w(12).flex_none())
      .child(svg(`mods/files/${r.dir ? "folder" : "file"}.svg`).size(14).flex_none().text_color(this.picked.has(r.abs) ? t.accent : t.muted))
      .child(div().flex_1().min_w(0).truncate().text_color(r.ignored ? t.dim : r.abs === this.current ? t.accent : t.foreground).child(r.name))
      .when(this.picked.has(r.abs), (el) => el.child(div().text_size(11).text_color(t.accent).child("●")))
      .when(Boolean(mark), (el) => el.child(div().text_size(11).text_color(markColor).child(mark)));
  }

  scrollKey(row) {
    return `${this.filter ? "find" : `tree:${this.model.root}`}:${row?.abs}`;
  }

  // A click on a row (the list reports its key, the row's path): select it,
  // take the keyboard, open it.
  clicked(key, cx) {
    const rows = this.rows(), at = rows.findIndex((r) => r.abs === key);
    if (at < 0) return;
    this.selected = at;
    this.scrolledTo = this.scrollKey(rows[at]); // already on screen: it was clicked
    this.handle.focus();
    cx.spawn(async (cx) => this.openRow(rows[at], cx));
  }

  render(cx) {
    this.follow(cx);
    if (this.stale) {
      this.stale = false;
      const gen = this.rootGen;
      cx.spawn(async (cx) => {
        await this.model.reload().catch(() => {});
        if (this.rootGen === gen) cx.notify();
      });
    }
    const reveal = shared("herdr-nvim.reveal");
    if (reveal && reveal.n !== this.revealed) {
      this.revealed = reveal.n;
      // A path (a dialog's result) is shown after listing again; none
      // (cmd-shift-e, <leader>E, the tree key) is nvim's current file.
      if (reveal.tab === this.place?.tab) {
        cx.spawn(async (cx) => (reveal.path ? this.reveal(reveal.path, cx, true) : this.revealCurrent(cx, true)));
      }
    }
    // Highlighted while this tree has the keyboard. Nothing tells a mod it
    // lost it, and the app's focus doesn't change when it goes back to the
    // same nvim pane, so nvim reports its FocusGained (next to the current
    // file, which this tree watches): every such wake renders this again.
    const t = theme(), rows = this.rows(), focusedNow = this.handle.is_focused();
    this.drawnFocused = focusedNow;
    if (this.revealing && !this.filter) {
      const at = rows.findIndex((r) => r.abs === this.revealing);
      if (at >= 0) { this.selected = at; this.revealing = null; }
    }
    this.selected = Number.isInteger(this.selected) ? Math.min(this.selected, Math.max(0, rows.length - 1)) : 0;
    // The selected row, by what it is (the list, the root, its path): a new
    // selection, a find starting or ending, or rows moving under the same
    // index each bring it into view; a scroll alone moves nothing.
    const key = rows.length ? this.scrollKey(rows[this.selected]) : null;
    if (key && key !== this.scrolledTo) {
      this.scrolledTo = key;
      this.scroll.scroll_to_item(this.selected);
    }
    const empty = this.place?.machine ? `This space is on ${this.place.machine}; herdr-nvim runs nvim on this computer.`
      : this.error ?? (this.filter ? (this.filter.query ? "No match" : "Type to find a file") : !this.model.root ? "No folder" : this.model.entries.has(this.model.root) ? "No files" : "Loading files…");
    return v_flex().id("herdr-nvim-files").size_full().min_h(0).rounded(10).overflow_hidden()
      // The controller hands this tree the keyboard with `pane.go`, which
      // needs it laid out: say when it is.
      // At most every 250 ms: a set redraws every mod copy in the window, and a
      // drag resizes this on every frame. The first size after a show counts.
      .on_size_change(() => {
        const now = Date.now();
        if (now - (this.laidOutAt ?? 0) < 250) return;
        this.laidOutAt = now;
        setShared("herdr-nvim.tree-laid-out", (shared("herdr-nvim.tree-laid-out") ?? 0) + 1);
      })
      .bg(t.background).border_1().border_color(focusedNow ? t.accent : t.border).text_color(t.foreground).text_size(13)
      // The rows as they are at the press, not as this render saw them.
      .track_focus(this.handle).on_key_down((e, cx) => this.key(e, this.rows(), cx))
      .on_mouse_down("left", (_e, cx) => this.pressed(cx))
      .child(this.header(t))
      .when(Boolean(this.filter), (el) => el.child(this.filterBar(t)))
      // Only the rows on screen are built (a tree can list thousands), and
      // a moved selection is scrolled into view.
      .child(rows.length === 0
        ? div().flex_1().p(14).text_size(12).text_color(t.dim).child(empty)
        : div().relative().flex_1().min_h(0).p(6)
          .child(v_virtual_list("herdr-nvim-tree", rows.length, 28, (i) => rows[i].abs,
            (range) => rows.slice(range.start, range.end).map((r, k) => this.row(r, range.start + k, t, focusedNow)))
            .track_scroll(this.scroll).size_full()
            .on_item_click((key, cx) => this.clicked(key, cx)))
          .child(Scrollbar.vertical("herdr-nvim-tree").absolute().inset_0()))
      .child(div().flex_none().px(12).py(6).border_t(1).border_color(t.border).text_size(11).text_color(t.dim).child(HINT));
  }
}
