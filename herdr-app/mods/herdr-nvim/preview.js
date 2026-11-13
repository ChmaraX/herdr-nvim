// The preview: a file shown as what it is, in the tab's nvim's place
// (the controller, sync.js `startSwap`, splits nvim's pane with this mod, props
// {preview: <file>, tab}, then closes nvim's pane; nvim keeps running).
// Markdown is rendered, images and SVGs drawn. It reloads when the file
// changes (saved in nvim, written by an agent). The pane header's Source
// button (mods/herdr-nvim-header) puts nvim back.
//
// Images are drawn with image(), which takes a path inside the app home: a
// copy goes to `mods/herdr-nvim/.cache` (hidden, so writing there reloads no
// mod), named by its content. Mermaid blocks in markdown are drawn there too
// (mermaid.js). The pictures any preview shows are kept, and of the rest the
// newest CACHE_KEEP.

import { div, image } from "gpui-kit";
import { v_flex, TextView } from "gpui-base";
import { appHome, exec, panes, setShared, shared, theme, watchFolder } from "herdr";
import * as fs from "fs/promises";
import { drawMermaid, hashText, modeOf, pngSize, splitMermaid } from "./mermaid.js";

const KINDS = {
  md: "markdown", markdown: "markdown", mdx: "markdown",
  png: "image", jpg: "image", jpeg: "image", gif: "image", webp: "image", bmp: "image", ico: "image", svg: "image",
};
const MAX_BYTES = { markdown: 2 * 1024 * 1024, image: 30 * 1024 * 1024 };
const CACHE = "mods/herdr-nvim/.cache";
const CACHE_KEEP = 40;
// shared(): which file each tab's preview shows, for its pane header.
const SHOWN = "herdr-nvim.previews";
// shared(): the cache's files, {recent: names newest first, shown: {tab: names}}.
const CACHED = "herdr-nvim.preview-cache";

// "markdown", "image", or null: what a file previews as.
export function previewKind(path) {
  const ext = /\.([^./]+)$/.exec(path ?? "")?.[1]?.toLowerCase();
  return (ext && KINDS[ext]) || null;
}

const dirname = (p) => p.replace(/\/[^/]*$/, "") || "/";

// `byTab` with `tab` set to `value`, and only the tabs that still show a
// preview kept.
function withTab(byTab, tab, value) {
  const live = new Set(panes().filter((p) => p.terminal === null && p.mod === "herdr-nvim").map((p) => p.tab));
  return { ...Object.fromEntries(Object.entries(byTab ?? {}).filter(([t]) => live.has(t))), [tab]: value };
}

// FNV-1a over the bytes: the cache name of an image's content.
function hashOf(bytes) {
  let h = 0x811c9dc5;
  for (let i = 0; i < bytes.length; i++) h = Math.imul(h ^ bytes[i], 0x01000193) >>> 0;
  return h.toString(16).padStart(8, "0") + bytes.length.toString(16);
}

// An image copied into the cache: its name there.
async function cached(bytes, ext) {
  const name = `${hashOf(bytes)}.${ext}`;
  const dir = `${appHome()}/${CACHE}`;
  if (!(await fs.exists(`${dir}/${name}`))) {
    await fs.mkdir(dir, { recursive: true });
    await fs.writeFile(`${dir}/${name}`, bytes);
  }
  return name;
}

// `tab`'s preview shows these cache files now. Those and every other
// preview's stay; of the rest, the newest CACHE_KEEP do.
async function keepShown(tab, names) {
  const was = shared(CACHED);
  // Older installs kept only the names.
  const { recent = [], shown = {} } = Array.isArray(was) ? { recent: was } : was ?? {};
  const nowShown = withTab(shown, tab, names);
  const onScreen = new Set(Object.values(nowShown).flat());
  const order = [...new Set([...names, ...recent])];
  const gone = new Set(order.filter((n) => !onScreen.has(n)).slice(CACHE_KEEP));
  // Written before the first await, so two previews never undo each other.
  setShared(CACHED, { recent: order.filter((n) => !gone.has(n)), shown: nowShown });
  for (const name of gone) await fs.unlink(`${appHome()}/${CACHE}/${name}`).catch(() => {});
}

// A diagram drawn (or found drawn) into the cache in `mode` ("dark" or
// "light"): {name, width, height} in pixels as shown (drawn at twice that),
// or {error}.
async function diagram(code, mode) {
  const name = `mermaid-${mode}-${hashText(code)}.png`;
  const dir = `${appHome()}/${CACHE}`;
  if (!(await fs.exists(`${dir}/${name}`))) {
    await fs.mkdir(dir, { recursive: true });
    const error = await drawMermaid(code, mode, `${dir}/${name}`);
    if (error) return { error };
  }
  const size = pngSize(await fs.readFile(`${dir}/${name}`).catch(() => null));
  return size ? { name, width: size.width / 2, height: size.height / 2 } : { error: "The diagram was not drawn." };
}

// A mod reads files only inside the home folder: one elsewhere (/tmp, a
// screenshot under /private/var/folders) is read through a process.
async function readText(file) {
  try {
    return await fs.readFile(file, "utf8");
  } catch {
    const r = await exec("cat", [file]);
    if (r.code !== 0) throw new Error(r.stderr);
    return r.stdout;
  }
}

async function readBytes(file) {
  try {
    return await fs.readFile(file);
  } catch {
    // Copied into the cache (inside the app home), read there, then gone.
    const dir = `${appHome()}/${CACHE}`;
    await fs.mkdir(dir, { recursive: true });
    const copy = `${dir}/.copy-${Date.now()}-${Math.random().toString(36).slice(2)}`;
    const r = await exec("cp", [file, copy]);
    if (r.code !== 0) throw new Error(r.stderr);
    try {
      return await fs.readFile(copy);
    } finally {
      await fs.unlink(copy).catch(() => {});
    }
  }
}

// {kind: "markdown", text, parts?} (parts: with mermaid blocks, each
// diagram's pictures by mode in `drawn`), {kind: "image", name}, or {error}.
async function load(file) {
  const kind = previewKind(file);
  if (!kind) return { error: "Can't preview this kind of file." };
  let data;
  try {
    data = kind === "markdown" ? await readText(file) : await readBytes(file);
  } catch {
    return { error: "The file is gone." };
  }
  if (data.length > MAX_BYTES[kind]) return { error: `Too big to preview (${(data.length / 1048576).toFixed(1)} MB).` };
  if (kind === "markdown") {
    const parts = splitMermaid(data);
    if (!parts.some((p) => p.mermaid !== undefined)) return { kind, text: data };
    return { kind, text: data, parts: parts.map((p) => (p.mermaid === undefined ? p : { ...p, drawn: {} })) };
  }
  const ext = /\.([^./]+)$/.exec(file)[1].toLowerCase();
  return { kind, name: await cached(data, ext) };
}

// Markdown as the preview renders it: every TextView of it is made here.
const markdown = (id, text) => TextView.markdown(id, text).selectable(true);

export class Preview {
  constructor(props, cx) {
    this.file = props.preview;
    this.tab = props.tab;
    this.shown = null;
    // The diagrams' theme: the one Herdr shows, followed in render().
    this.mode = modeOf(theme().background);
    this.wake = null;
    setShared(SHOWN, withTab(shared(SHOWN), this.tab, this.file));
    cx.spawn(async (cx) => {
      // A changed path of the folder's root means "anything may have changed".
      const ours = (changed) => !changed.length || changed.some((p) => p === this.file || p === dirname(this.file));
      while (!this.closed) {
        this.shown = await load(this.file);
        await this.keepShown();
        cx.notify();
        this.nudge();
        while (!this.closed && !ours(await watchFolder(dirname(this.file))));
      }
    });
    cx.spawn((cx) => this.drawDiagrams(cx));
  }

  // The pane is gone (main.js `close`): the watch and draw loops end.
  close() {
    this.closed = true;
    this.nudge();
  }

  // Tells drawDiagrams there may be something new to draw.
  nudge() {
    const wake = this.wake;
    this.wake = null;
    wake?.();
  }

  // The diagrams of what the pane shows, in the theme it shows, one at a time
  // (a newer load or another theme is taken up after the one at hand), each
  // shown as it lands.
  async drawDiagrams(cx) {
    while (!this.closed) {
      const s = this.shown;
      const mode = this.mode;
      const part = s?.parts?.find((p) => p.drawn && !p.drawn[mode]);
      if (!part) {
        await new Promise((resolve) => (this.wake = resolve));
        continue;
      }
      part.drawn[mode] = await diagram(part.mermaid, mode).catch((e) => ({ error: String(e?.message ?? e) }));
      if (this.shown === s) {
        await this.keepShown();
        cx.notify();
      }
    }
  }

  // The cache files on screen now, kept for as long as they are.
  keepShown() {
    const s = this.shown;
    const names = s?.kind === "image" ? [s.name] : (s?.parts ?? []).map((p) => p.drawn?.[this.mode]?.name).filter(Boolean);
    return keepShown(this.tab, names);
  }

  // Markdown with mermaid blocks: the text between them as markdown, each
  // diagram as its picture, at most the pane's width, all in one scroll.
  parts(s, t) {
    const room = this.width ? Math.max(40, this.width - 36) : null;
    const diagramOf = (p, i) => {
      const d = p.drawn[this.mode];
      if (!d) return div().py(6).text_color(t.dim).child("Drawing diagram…");
      if (d.error) {
        return v_flex()
          .gap(4)
          .child(div().text_color(t.ansi1).child(`Mermaid: ${d.error}`))
          .child(markdown(`herdr-nvim-preview-${i}`, "```mermaid\n" + p.mermaid + "\n```"));
      }
      const w = room ? Math.min(d.width, room) : d.width;
      return div().py(6).child(image(`${CACHE}/${d.name}`).w(w).h((d.height * w) / d.width));
    };
    return v_flex()
      .id("herdr-nvim-preview-parts")
      .flex_1()
      .min_h(0)
      .w_full()
      .px(18)
      .py(12)
      .overflow_y_scrollbar()
      .on_size_change((size, cx) => {
        if (size.width !== this.width) {
          this.width = size.width;
          cx.notify();
        }
      })
      .children(s.parts.map((p, i) => (p.drawn ? diagramOf(p, i) : markdown(`herdr-nvim-preview-${i}`, p.md))));
  }

  render() {
    const t = theme();
    const mode = modeOf(t.background);
    if (mode !== this.mode) {
      this.mode = mode;
      this.nudge();
    }
    const s = this.shown;
    const body = !s
      ? div()
      : s.error
        ? div().m_auto().text_color(t.dim).child(s.error)
        : s.kind === "image"
          ? div().flex_1().min_h(0).w_full().p(14).child(image(`${CACHE}/${s.name}`).size_full())
          : s.parts
            ? this.parts(s, t)
            : markdown("herdr-nvim-preview", s.text).scrollable(true).flex_1().min_h(0).px(18).py(12);
    return v_flex()
      .id("herdr-nvim-preview")
      .size_full()
      .bg(t.background)
      .text_size(13)
      .text_color(t.foreground)
      .child(body);
  }
}
