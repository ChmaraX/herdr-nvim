// cmd-o (`herdr-nvim.pick`, taken by the controller, sync.js): a file from
// the agent's output, in a native dialog of this mod (props {pick}): the
// files the agent read and edited (its session log, newest first), then
// git's changes, with their diff counts and age. Typing filters those and
// searches the whole repo (`app-find`, as the tree's `/` does). ⏎ opens the
// file in the tab's nvim (at its line, when the agent named one).
//
// `herdr-nvim pick-file --list --session` gathers the candidates; the
// dialog is on screen before they arrive.

import { div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, Scrollbar, VirtualListScrollHandle, v_virtual_list } from "gpui-base";
import { closeOverlay, home, notify, openOverlay, props, run, theme } from "herdr";
import { nvim } from "./lib.js";
import { findFiles } from "./ops.js";

const ROW = 30;
const TYPE_PAUSE_MS = 80;

// `at`: the tab, as `place()` captured it when the key was pressed.
export function pick(at) {
  if (!at) return notify({ title: "herdr-nvim", body: "Open a tab first.", level: "warning" });
  openOverlay({ mod: "herdr-nvim", kind: "dialog", width: 820, height: 470, props: { pick: at } });
}

// Pure: how well `needle` matches the shown path `hay` (case-insensitive),
// null when it doesn't: the query as one piece first, else the terminal
// picker's subsequence rule (src/picker.rs `fuzzy_match`): higher for runs,
// word starts and the file name.
export function fuzzyScore(hay, needle) {
  if (!needle) return 0;
  const h = hay.toLowerCase(), n = needle.toLowerCase(), nameStart = hay.lastIndexOf("/") + 1;
  // The query as one piece ranks above any scattered match: in the file
  // name above the folders, earlier above later, shorter paths first.
  const at = h.lastIndexOf(n);
  if (at >= 0) return 10000 + (at >= nameStart ? 5000 - (at - nameStart) * 10 : -at) - h.length;
  let score = 0, hi = 0, prev = -2;
  for (const ch of n) {
    hi = h.indexOf(ch, hi);
    if (hi < 0) return null;
    score += hi === prev + 1 ? 12 : 1;
    if (hi === 0 || "/._- ".includes(h[hi - 1])) score += 4;
    if (hi >= nameStart) score += 2;
    prev = hi;
    hi++;
  }
  return score;
}

function age(now, then) {
  const s = Math.max(0, now - then);
  return s < 60 ? "now" : s < 3600 ? `${Math.floor(s / 60)}m` : s < 86400 ? `${Math.floor(s / 3600)}h` : `${Math.floor(s / 86400)}d`;
}

export class Picker {
  constructor(cx) {
    this.at = props().pick;
    this.handle = cx.focus_handle();
    this.scroll = VirtualListScrollHandle.new();
    this.input = InputState.new({ placeholder: "Type to search the repo" });
    this.input.on("change", (_e, cx) => this.typed(cx));
    this.input.on("submit", (_e, cx) => this.open(cx));
    this.input.focus();
    this.candidates = null; // from pick-file, newest first
    this.cwd = this.at.cwd ?? "";
    this.found = []; // the repo's matches for the query (absolute paths)
    this.query = "";
    this.selected = 0;
    this.seq = 0;
    this.message = null;
    cx.spawn(async (cx) => {
      const out = await nvim("pick-file", ["--list", "--session"], this.at);
      let list = null;
      try { list = out && JSON.parse(out); } catch {}
      if (!list) return this.close();
      this.cwd = list.cwd || this.cwd;
      this.maxFiles = list.max_files ?? 200;
      this.candidates = list.candidates.filter((c) => c.session);
      if (!this.candidates.length) this.message = "The agent hasn't touched a file yet, and git shows no change: type to search the repo.";
      cx.notify();
    });
  }

  close() {
    closeOverlay(props().overlay);
  }

  // Shown paths: inside the folder relative, under home with ~.
  display(path) {
    const h = home();
    if (this.cwd && path.startsWith(`${this.cwd}/`)) return path.slice(this.cwd.length + 1);
    if (h && path.startsWith(`${h}/`)) return `~/${path.slice(h.length + 1)}`;
    return path;
  }

  // The rows: with no query the agent's files (at most max_files); with one,
  // those that match, best first, then the repo's matches not among them.
  rows() {
    const cands = this.candidates ?? [];
    if (!this.query) return cands.slice(0, this.maxFiles ?? 200);
    const scored = [];
    for (const c of cands) {
      const s = fuzzyScore(this.display(c.path), this.query);
      if (s !== null) scored.push({ c, s });
    }
    scored.sort((a, b) => b.s - a.s);
    const seen = new Set(scored.map((x) => x.c.path));
    return [...scored.map((x) => x.c), ...this.found.filter((p) => !seen.has(p)).map((path) => ({ path, repo: true }))];
  }

  typed(cx) {
    this.query = this.input.value().trim();
    this.selected = 0;
    this.scroll.scroll_to_item(0);
    const seq = ++this.seq, query = this.query;
    this.found = [];
    cx.notify();
    if (!query) return;
    cx.spawn(async (cx) => {
      await cx.sleep(TYPE_PAUSE_MS);
      if (seq !== this.seq) return;
      const rel = await findFiles(this.cwd, query);
      if (seq !== this.seq) return;
      this.found = rel.map((p) => `${this.cwd}/${p}`);
      cx.notify();
    });
  }

  move(by, cx) {
    const n = this.rows().length;
    if (!n) return;
    this.selected = Math.max(0, Math.min(n - 1, this.selected + by));
    this.scroll.scroll_to_item(this.selected);
    cx.notify();
  }

  // ⏎ or a click: the dialog goes, the file opens in the tab's nvim, which
  // then has the keyboard.
  open(cx, row = this.rows()[this.selected]) {
    if (!row) return;
    this.close();
    const target = row.line ? `${row.path}:${row.line}` : row.path;
    cx.spawn(async () => {
      const out = await nvim("open", ["--no-focus", target], this.at);
      let terminal = null;
      try { terminal = out && JSON.parse(out).terminal; } catch {}
      if (terminal) run("pane.focus", { terminal });
    });
  }

  row(c, i, t, now) {
    const shown = this.display(c.path), cut = shown.lastIndexOf("/") + 1;
    const right = [];
    if (c.diff_stat) {
      right.push(div().text_color(t.ansi2).child(`+${c.diff_stat[0]}`));
      right.push(div().text_color(t.ansi1).child(`-${c.diff_stat[1]}`));
    } else if (c.newly_created) right.push(div().text_color(t.blocked).child("new"));
    if (c.touched_unix) right.push(div().text_color(t.dim).child(age(now, c.touched_unix)));
    return h_flex().id(`pick-${c.path}`).h(ROW).flex_none().px(12).gap(8).items_center().rounded(7).cursor_pointer()
      .when(i === this.selected, (el) => el.bg(t.selected))
      .hover((s) => s.bg(t.hover))
      .child(h_flex().flex_1().min_w(0).overflow_hidden()
        .child(div().flex_shrink().min_w(0).truncate().text_color(t.dim).child(shown.slice(0, cut)))
        .child(div().flex_none().text_color(c.repo ? t.muted : t.foreground).child(shown.slice(cut))))
      .child(h_flex().flex_none().gap(8).text_size(11.5).children(right));
  }

  render(cx) {
    const t = theme(), rows = this.rows(), now = Math.floor(Date.now() / 1000);
    this.selected = Math.min(this.selected, Math.max(0, rows.length - 1));
    const empty = this.candidates === null ? "Looking at the agent's files…"
      : this.query ? "No match" : this.message ?? "Nothing yet";
    return v_flex().id("herdr-nvim-pick").size_full().bg(t.popover).text_color(t.foreground).text_size(13)
      .track_focus(this.handle)
      .on_key_down((e, cx) => {
        const k = e.keystroke;
        if (k === "escape") this.close();
        else if (k === "down" || k === "ctrl-n") this.move(1, cx);
        else if (k === "up" || k === "ctrl-p") this.move(-1, cx);
        else if (k === "pagedown") this.move(10, cx);
        else if (k === "pageup") this.move(-10, cx);
        else return;
        cx.stop_propagation();
      })
      .child(h_flex().h(34).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
        .child(div().font_semibold().child("Open file from agent output"))
        .child(div().flex_1())
        .child(div().text_size(11.5).text_color(t.dim).child("↑↓ move · ⏎ open in nvim · esc close")))
      .child(div().flex_none().p(10).child(Input.new(this.input).h(34).px(10).rounded(8).border_1()
        .border_color(t.accent).bg(t.background).text_size(13)))
      .child(rows.length === 0
        ? div().flex_1().px(16).py(10).text_size(12.5).text_color(t.dim).child(empty)
        : div().relative().flex_1().min_h(0).px(6).pb(6)
          .child(v_virtual_list("herdr-nvim-pick-list", rows.length, ROW, (i) => rows[i].path,
            (range) => rows.slice(range.start, range.end).map((c, k) => this.row(c, range.start + k, t, now)))
            .track_scroll(this.scroll).size_full()
            .on_item_click((key, cx) => {
              const at = rows.findIndex((r) => r.path === key);
              if (at >= 0) { this.selected = at; this.open(cx, rows[at]); }
            }))
          .child(Scrollbar.vertical("herdr-nvim-pick-list").absolute().inset_0()));
  }
}
