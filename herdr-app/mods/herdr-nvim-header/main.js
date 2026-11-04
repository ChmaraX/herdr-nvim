// herdr-nvim's pane header: the app's own pane header (pane-base.js, the
// shipped `mods/pane/main.js` as install found it; a mod can't import
// another mod's files) with one button on the tab's nvim and its preview:
// - on herdr-nvim's nvim pane, while nvim's file is markdown, an image or an
//   SVG: Preview (the file rendered in nvim's place);
// - on the preview pane: Source (nvim back as it was); for an
//   image or an SVG it reads "nvim", there being no source to show.
// Every other pane's header is the shipped one, unchanged.
//
// nvim's file comes from what the tab's nvim tells the tree (`app-current`'s
// `watch` file, lua/herdr-nvim/app.lua `write_state`), watched for changes.
//
// The button only asks herdr-nvim's controller for the swap
// (`herdr-nvim.swap:<tab>:<file>`, mods/herdr-nvim sync.js `startSwap`).

import { div } from "gpui-kit";
import { h_flex } from "gpui-base";
import { appHome, exec, panes, props, readTextFile, run, shared, spaces, terminals, theme, watchFolder } from "herdr";
import Pane from "./pane-base.js";

const KINDS = {
  md: "markdown", markdown: "markdown", mdx: "markdown",
  png: "image", jpg: "image", jpeg: "image", gif: "image", webp: "image", bmp: "image", ico: "image", svg: "image",
};
const kindOf = (path) => KINDS[/\.([^./]+)$/.exec(path ?? "")?.[1]?.toLowerCase()] ?? null;
const basename = (p) => p.replace(/^.*\//, "");
const dirname = (p) => p.replace(/\/[^/]*$/, "") || "/";
const bin = () => `${appHome()}/mods/herdr-nvim/bin/herdr-nvim`;

// How long a failed first look waits before asking again.
const RETRY_MS = 2000;

export default class Header extends Pane {
  init() {
    super.init();
    this.file = null; // nvim's file, on herdr-nvim's nvim pane
    this.watching = null;
  }

  // The view is gone: its watch loop ends.
  async close(cx) {
    this.closed = true;
    await super.close?.(cx);
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
  // nvim writes it, then the header watches that file. A first look that
  // fails (nvim just starting, a slow moment) is tried again a little later.
  follow(r, cx) {
    if (this.watching) return;
    this.watching = "starting";
    cx.spawn(async (cx) => {
      const out = await exec(bin(), ["app-current", "--ctx", this.ctxOf(r)]).catch(() => null);
      let at = null;
      try { at = out?.code === 0 ? JSON.parse(out.stdout) : null; } catch {}
      this.file = at?.file ?? null;
      cx.notify();
      if (!at?.watch) {
        await cx.sleep(RETRY_MS);
        this.watching = null;
        return cx.notify();
      }
      this.watching = at.watch;
      while (!this.closed) {
        // The folder holds every tab's file: only this tab's matters (a
        // batch naming the folder itself may be anything).
        const changed = await watchFolder(dirname(at.watch));
        if (this.closed) return;
        if (changed.length && !changed.some((p) => p.startsWith(at.watch) || p === dirname(at.watch))) continue;
        const text = await readTextFile(at.watch).catch(() => null);
        let file = null;
        try { file = (text && JSON.parse(text).file) || null; } catch {}
        if (file !== this.file) { this.file = file; cx.notify(); }
      }
    });
  }

  // herdr-nvim's controller (mods/herdr-nvim sync.js `startSwap`) does the
  // swap, so it knows the tab is mid-swap: Preview names the file, Source
  // names none.
  swap(r) {
    const file = r.role === "nvim" ? this.file : "";
    run(`herdr-nvim.swap:${r.tab}:${encodeURIComponent(file ?? "")}`);
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
              .text_color(t.foreground)
              .hover((s) => s.bg(t.hover))
              .on_click((_e, cx) => {
                cx.stop_propagation();
                this.swap(r);
              })
              .child(label),
          ),
      );
  }
}
