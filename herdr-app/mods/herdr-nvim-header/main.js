// herdr-nvim's pane header: the app's own pane header (pane-base.js, the
// shipped `mods/pane/main.js` as install found it; a mod can't import
// another mod's files) with one button on the tab's nvim and its preview:
// - on herdr-nvim's nvim pane, while nvim's file is markdown, an image or an
//   SVG: Preview (the file rendered in nvim's place, `app-preview`);
// - on the preview pane: Source (nvim back as it was, `app-source`); for an
//   image or an SVG it reads "nvim", there being no source to show.
// Every other pane's header is the shipped one, unchanged.
//
// nvim's file comes from what the tab's nvim tells the tree (`app-current`'s
// `watch` file, lua/herdr-nvim/app.lua `write_state`), watched for changes.
//
// The swap runs here, in the app, in one go: split this pane across its
// parent with the other one (`across`, from `app-current --across`: both
// nest in this pane's slot), then close this pane, so the new one has its
// exact place and the app draws the change once. nvim keeps running: its
// pane only attaches to the tab's daemon (`herdr-nvim sidebar`).

import { div } from "gpui-kit";
import { h_flex } from "gpui-base";
import { appHome, exec, notify, panes, props, readTextFile, run, setShared, shared, spaces, terminals, theme, watchFolder } from "herdr";
import Pane from "./pane-base.js";

const KINDS = {
  md: "markdown", markdown: "markdown", mdx: "markdown",
  png: "image", jpg: "image", jpeg: "image", gif: "image", webp: "image", bmp: "image", ico: "image", svg: "image",
};
const kindOf = (path) => KINDS[/\.([^./]+)$/.exec(path ?? "")?.[1]?.toLowerCase()] ?? null;
const basename = (p) => p.replace(/^.*\//, "");
const dirname = (p) => p.replace(/\/[^/]*$/, "") || "/";
const bin = () => `${appHome()}/mods/herdr-nvim/bin/herdr-nvim`;

export default class Header extends Pane {
  init() {
    super.init();
    this.file = null; // nvim's file, on herdr-nvim's nvim pane
    this.watching = null;
    this.across = null; // the swap's split direction
    this.cwd = null; // the tab's nvim's folder
    this.busy = false;
  }

  // What this pane is to herdr-nvim: {role: "nvim", tab} (the tab's nvim),
  // {role: "preview", tab, file}, or null.
  role() {
    const { pane, terminal } = props() ?? {};
    if (terminal) {
      const tab = terminals().find((t) => t.address === terminal)?.labels?.["herdr-nvim.sidebar"];
      return tab ? { role: "nvim", tab, terminal } : null;
    }
    const p = panes().find((x) => x.pane === pane);
    if (p?.terminal !== null || p?.mod !== "herdr-nvim") return null;
    return { role: "preview", tab: p.tab, file: (shared("herdr-nvim.previews") ?? {})[p.tab] ?? null };
  }

  // The preview's header names its file, as nvim's does.
  me() {
    const p = super.me();
    const r = p && this.role();
    return r?.role === "preview" && r.file ? { ...p, title: `Preview · ${basename(r.file)}` } : p;
  }

  ctxOf(r) {
    const space = spaces().find((s) => s.tabs.some((t) => t.id === r.tab))?.id;
    return JSON.stringify({ space, tab: r.tab, terminal: r.terminal ?? null, cwd: null });
  }

  // nvim's file, followed: the first look asks `app-current` where the tab's
  // nvim writes it (and the swap's direction and folder), then the header
  // watches that file. The preview's header only needs the direction.
  follow(r, cx) {
    if (this.watching) return;
    this.watching = "starting";
    cx.spawn(async (cx) => {
      const out = await exec(bin(), ["app-current", "--across", "--ctx", this.ctxOf(r)]).catch(() => null);
      const at = out?.code === 0 ? JSON.parse(out.stdout) : null;
      this.across = at?.across ?? null;
      this.cwd = at?.tab_root ?? null;
      this.file = at?.file ?? null;
      cx.notify();
      if (r.role !== "nvim" || !at?.watch) return;
      this.watching = at.watch;
      for (;;) {
        await watchFolder(dirname(at.watch));
        const text = await readTextFile(at.watch).catch(() => null);
        let file = null;
        try { file = (text && JSON.parse(text).file) || null; } catch {}
        if (file !== this.file) { this.file = file; cx.notify(); }
      }
    });
  }

  swap(r, cx) {
    if (this.busy) return;
    const pane = props()?.pane;
    // For herdr-nvim's controller (sync.js): mid-swap, the tab shows neither
    // pane for a moment; that is not nvim closing (which would hide the tree).
    const mark = { tab: r.tab, at: Date.now(), done: null };
    if (this.across && this.cwd && pane) {
      this.busy = true;
      setShared("herdr-nvim.swap", mark);
      const before = new Set(panes().filter((p) => p.tab === r.tab).map((p) => p.pane));
      if (r.role === "nvim") {
        run("pane.split", { direction: this.across, pane, mod: "herdr-nvim", props: { preview: this.file, tab: r.tab, cwd: this.cwd } });
      } else {
        run("pane.split", { direction: this.across, pane, argv: [bin(), "sidebar", "--cwd", this.cwd] });
      }
      cx.spawn(async (cx) => {
        // Close this pane only once the new one is in the tab: closed first,
        // its slot would be gone and the new pane shared into the row (a
        // terminal's pane comes a moment after its split).
        const deadline = Date.now() + 3000;
        let made = null;
        while (!made && Date.now() < deadline) {
          made = panes().find((p) => p.tab === r.tab && !before.has(p.pane)) ?? null;
          if (!made) await cx.sleep(25);
        }
        if (made) run("pane.close", { pane });
        else notify({ title: "herdr-nvim", body: "The new pane did not show up; nothing was closed.", level: "error" });
        setShared("herdr-nvim.swap", { ...mark, done: Date.now() });
        this.busy = false;
        cx.notify();
      });
      return;
    }
    // Not known yet (app-current still running or failed): herdr-nvim does it.
    this.busy = true;
    cx.notify();
    const args = r.role === "nvim" ? ["app-preview", "--file", this.file] : ["app-source"];
    setShared("herdr-nvim.swap", mark);
    cx.spawn(async (cx) => {
      const out = await exec(bin(), [...args, "--ctx", this.ctxOf(r)]).catch((e) => ({ code: 1, stderr: String(e) }));
      setShared("herdr-nvim.swap", { ...mark, done: Date.now() });
      if (out.code !== 0) notify({ title: `herdr-nvim ${args[0]} failed`, body: (out.stderr || out.stdout || "").trim().slice(-400), level: "error" });
      this.busy = false;
      cx.notify();
    });
  }

  render(cx) {
    const head = super.render(cx);
    const r = this.role();
    if (r) this.follow(r, cx);
    const label =
      r?.role === "nvim" ? (kindOf(this.file) ? "Preview" : null)
      : r?.role === "preview" ? (kindOf(r.file) === "image" ? "nvim" : "Source")
      : null;
    if (!label) return head;
    const t = theme();
    // Its own slot at the right end, outside the shipped header: that one's
    // hover buttons (split, zoom, close) and its truncation never cover or
    // push away the button.
    return h_flex()
      .size_full()
      .child(div().flex_1().min_w(0).h_full().child(head))
      .child(
        h_flex()
          .flex_none()
          .h_full()
          .pr(8)
          .pl(4)
          .items_center()
          .bg(t.background)
          .border_b(1)
          .border_color(t.border)
          .child(
            div()
              .id(`hn-swap-${props()?.pane}`)
              .px(8)
              .h(20)
              .flex()
              .items_center()
              .rounded(5)
              .border_1()
              .border_color(t.border)
              .text_size(11.5)
              .font_medium()
              .text_color(this.busy ? t.dim : t.foreground)
              .hover((s) => s.bg(t.hover))
              .on_click((_e, cx) => {
                cx.stop_propagation();
                this.swap(r, cx);
              })
              .child(label),
          ),
      );
  }
}
