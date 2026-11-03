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

import { div } from "gpui-kit";
import { h_flex } from "gpui-base";
import { appHome, exec, notify, panes, props, readTextFile, setShared, shared, spaces, terminals, theme, watchFolder } from "herdr";
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
  // nvim writes it, then the header watches that file.
  follow(r, cx) {
    if (this.watching) return;
    this.watching = "starting";
    cx.spawn(async (cx) => {
      const out = await exec(bin(), ["app-current", "--ctx", this.ctxOf(r)]).catch(() => null);
      const at = out?.code === 0 ? JSON.parse(out.stdout) : null;
      this.file = at?.file ?? null;
      cx.notify();
      if (!at?.watch) return;
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
    this.busy = true;
    cx.notify();
    const args = r.role === "nvim" ? ["app-preview", "--file", this.file] : ["app-source"];
    // For herdr-nvim's controller (sync.js): mid-swap, the tab shows neither
    // pane for a moment; that is not nvim closing (which would hide the tree).
    const swap = { tab: r.tab, at: Date.now(), done: null };
    setShared("herdr-nvim.swap", swap);
    cx.spawn(async (cx) => {
      const out = await exec(bin(), [...args, "--ctx", this.ctxOf(r)]).catch((e) => ({ code: 1, stderr: String(e) }));
      setShared("herdr-nvim.swap", { ...swap, done: Date.now() });
      if (out.code !== 0) notify({ title: `herdr-nvim ${args[0]} failed`, body: (out.stderr || out.stdout || "").trim().slice(-400), level: "error" });
      this.busy = false;
      cx.notify();
    });
  }

  render(cx) {
    const head = super.render(cx);
    const r = this.role();
    if (r?.role === "nvim") this.follow(r, cx);
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
