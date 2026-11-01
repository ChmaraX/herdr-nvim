// herdr-nvim for the Herdr app. nvim stays a real terminal: a full-height
// pane of the tab, backed by that tab's persistent nvim daemon. Around it the
// UI is native.
//
// Two roles, one mod:
// - In a layout block (the install puts it in the side panel's `files` slot,
//   so the tab bar's Files button shows it): a native file tree of the focused
//   tab's folder with git marks. Picking a file opens it in the tab's nvim
//   (opening the sidebar if needed); the header button toggles the sidebar.
// - As an overlay (`mod.open:herdr-nvim`, a key): toggles the sidebar once.

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { props, closeOverlay, focused, spaces, panes, terminals, theme, watchFolder } from "herdr";
import { nvim } from "./lib.js";
import { listFolder, gitMarks } from "./tree.js";

const ROWS = 3000;

// The focused tab and its folder. A focused layout block (this tree) has no
// pane: the tab's folder stays what it was.
function place(previous) {
  const f = focused();
  const space = spaces().find((s) => s.id === f?.space) ?? spaces().find((s) => s.active);
  const pane = f?.pane ? panes().find((p) => p.pane === f.pane) : null;
  const cwd = pane?.cwd || (f?.tab === previous?.tab ? previous?.cwd : null) || f?.cwd || space?.folder || null;
  return { tab: f?.tab ?? null, cwd, machine: space?.machine ?? null };
}

function sidebarOpen(tab) {
  return terminals().some((t) => t.state === "live" && t.labels?.["herdr-nvim.sidebar"] === tab);
}

export default class HerdrNvim extends View {
  init(_props, cx) {
    const overlay = props()?.overlay;
    if (overlay) {
      cx.spawn(async () => {
        await nvim("toggle");
        closeOverlay(overlay);
      });
      return;
    }
    this.handle = cx.focus_handle();
    this.place = null;
    this.reset(null);
  }

  reset(root) {
    this.root = root;
    this.entries = new Map(); // folder ("" = root) -> entries
    this.open = new Set();
    this.marks = {};
    this.selected = 0;
    this.error = null;
    this.busy = false;
  }

  // Follow the focused tab's folder; each folder is listed once and again
  // whenever something under it changes (no polling).
  follow(cx) {
    const next = place(this.place);
    const moved = next.cwd !== this.place?.cwd || next.machine !== this.place?.machine;
    this.place = next;
    if (!moved) return;
    this.reset(next.cwd);
    if (!next.cwd || next.machine) return;
    const root = next.cwd;
    this.load("", cx);
    cx.spawn(async (cx) => {
      try {
        for (;;) {
          this.marks = await gitMarks(root);
          if (this.root !== root) return;
          cx.notify();
          await watchFolder(root);
          if (this.root !== root) return;
          for (const folder of ["", ...this.open]) this.load(folder, cx);
        }
      } catch (e) {
        if (this.root === root) { this.error = String(e); cx.notify(); }
      }
    });
  }

  load(folder, cx) {
    const root = this.root;
    cx.spawn(async (cx) => {
      try {
        const entries = await listFolder(folder ? `${root}/${folder}` : root);
        if (this.root !== root) return;
        this.entries.set(folder, entries);
      } catch (e) {
        if (this.root === root && !folder) this.error = String(e);
      }
      cx.notify();
    });
  }

  // The rows on screen, depth first through the open folders.
  rows(folder = "", depth = 0, out = []) {
    for (const entry of this.entries.get(folder) ?? []) {
      if (out.length >= ROWS) break;
      const path = folder ? `${folder}/${entry.name}` : entry.name;
      out.push({ ...entry, path, depth });
      if (entry.dir && this.open.has(path)) this.rows(path, depth + 1, out);
    }
    return out;
  }

  activate(row, cx) {
    if (!row) return;
    if (row.dir) {
      if (this.open.has(row.path)) this.open.delete(row.path);
      else { this.open.add(row.path); if (!this.entries.has(row.path)) this.load(row.path, cx); }
      cx.notify();
      return;
    }
    this.busy = true;
    cx.notify();
    cx.spawn(async (cx) => {
      await nvim("open", [`${this.root}/${row.path}`]);
      this.busy = false;
      cx.notify();
    });
  }

  key(e, rows, cx) {
    const row = rows[this.selected];
    if (e.key === "down") this.selected = Math.min(rows.length - 1, this.selected + 1);
    else if (e.key === "up") this.selected = Math.max(0, this.selected - 1);
    else if (e.key === "enter") this.activate(row, cx);
    else if (e.key === "right" && row?.dir && !this.open.has(row.path)) this.activate(row, cx);
    else if (e.key === "left" && row?.dir && this.open.has(row.path)) this.activate(row, cx);
    else return;
    cx.stop_propagation();
    cx.notify();
  }

  header(t) {
    const tab = this.place?.tab, on = tab && sidebarOpen(tab);
    return h_flex().h(42).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
      .child(div().font_semibold().child("Files"))
      .child(div().flex_1().min_w(0).truncate().text_size(12).text_color(t.dim).child(this.root?.split("/").pop() ?? ""))
      .when(this.busy, (el) => el.child(svg("mods/workspace-chrome/spinner.svg").size(12).flex_none().text_color(t.muted).spin(900)))
      .child(h_flex().id("nvim-toggle").h(24).px(8).gap(5).items_center().rounded(6).text_size(12)
        .bg(on ? t.selected : t.surface).text_color(on ? t.foreground : t.muted).border_1().border_color(t.border)
        .hover((s) => s.bg(t.hover).text_color(t.foreground)).cursor_pointer()
        .on_click((_e, cx) => { cx.spawn(async (cx) => { await nvim("toggle"); cx.notify(); }); })
        .child(svg("icons/nvim.svg").size(13)).child(on ? "Close nvim" : "nvim"));
  }

  row(r, i, t) {
    const mark = this.marks[`${this.root}/${r.path}`];
    const markColor = mark === "A" ? t.ansi2 : mark === "D" || mark === "U" ? t.ansi1 : t.blocked;
    return h_flex().id(`row-${r.path}`).h(28).flex_none().pl(8 + r.depth * 18).pr(8).gap(7).items_center().rounded(7).cursor_pointer()
      .when(i === this.selected, (el) => el.bg(t.selected))
      .hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => { this.selected = i; this.handle.focus(); this.activate(r, cx); })
      .child(r.dir ? svg(`mods/files/chevron-${this.open.has(r.path) ? "down" : "right"}.svg`).size(12).flex_none().text_color(t.dim) : div().w(12).flex_none())
      .child(svg(`mods/files/${r.dir ? "folder" : "file"}.svg`).size(14).flex_none().text_color(t.muted))
      .child(div().flex_1().min_w(0).truncate().child(r.name))
      .when(Boolean(mark), (el) => el.child(div().text_size(11).text_color(markColor).child(mark)));
  }

  render(cx) {
    if (props()?.overlay) return div();
    this.follow(cx);
    const t = theme(), rows = this.rows();
    this.selected = Math.min(this.selected, Math.max(0, rows.length - 1));
    const empty = this.place?.machine ? `This space is on ${this.place.machine}; herdr-nvim runs nvim on this computer.`
      : this.error ?? (!this.root ? "No folder" : this.entries.has("") ? "No files" : "Loading files…");
    return v_flex().id("herdr-nvim-files").size_full().min_h(0).rounded(10).overflow_hidden()
      .bg(t.background).border_1().border_color(t.border).text_color(t.foreground).text_size(13)
      .track_focus(this.handle).on_key_down((e, cx) => this.key(e, rows, cx))
      .child(this.header(t))
      .child(v_flex().id("herdr-nvim-tree").flex_1().min_h(0).overflow_y_scrollbar().p(6)
        .children(rows.map((r, i) => this.row(r, i, t)))
        .when(rows.length === 0, (el) => el.child(div().p(8).text_size(12).text_color(t.dim).child(empty))))
      .child(div().flex_none().px(12).py(6).border_t(1).border_color(t.border).text_size(11).text_color(t.dim)
        .child("Click or ⏎ opens in this tab's nvim · ↑↓ ← →"));
  }
}
