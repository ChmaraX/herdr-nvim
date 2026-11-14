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
// (mermaid.js), once per code and theme, so editing the text around them or
// reloading draws nothing again. The pictures any preview shows are kept,
// and of the rest the newest CACHE_KEEP.

import { div, image } from "gpui-kit";
import { v_flex, TextView } from "gpui-base";
import { appHome, exec, panes, setShared, shared, theme, watchFolder } from "herdr";
import * as fs from "fs/promises";
import { drawMermaid, modeOf, pngSize, splitMermaid } from "./mermaid.js";

const KINDS = {
  md: "markdown", markdown: "markdown", mdx: "markdown",
  png: "image", jpg: "image", jpeg: "image", gif: "image", webp: "image", bmp: "image", ico: "image", svg: "image",
};
const MAX_BYTES = { markdown: 2 * 1024 * 1024, image: 30 * 1024 * 1024 };
const CACHE = "mods/herdr-nvim/.cache";
const CACHE_KEEP = 40;
// Markdown's padding left and right.
const PAD_X = 18;
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

// FNV-1a over a cache file's content, an image's bytes or a diagram's code
// (its UTF-16 units): its name in the cache.
function hashOf(data) {
  const unit = typeof data === "string" ? (i) => data.charCodeAt(i) : (i) => data[i];
  let h = 0x811c9dc5;
  for (let i = 0; i < data.length; i++) h = Math.imul(h ^ unit(i), 0x01000193) >>> 0;
  return h.toString(16).padStart(8, "0") + data.length.toString(16);
}

// A diagram's two pictures in the cache, by mode.
const diagramNames = (code) => {
  const hash = hashOf(code);
  return { dark: `mermaid-dark-${hash}.png`, light: `mermaid-light-${hash}.png` };
};

// The cache's shared record (older installs kept a bare list: read as empty).
function cacheRecord() {
  const { recent = [], shown = {} } = shared(CACHED) ?? {};
  return { recent, shown };
}

// `name` goes into the cache now: the newest of its files, so no preview's
// clean-up takes it for a leftover.
function noteCached(name) {
  const { recent, shown } = cacheRecord();
  setShared(CACHED, { recent: [name, ...recent.filter((n) => n !== name)], shown });
}

// `tab`'s preview shows `names` now. Every preview's are kept and, of the
// other files the record knows, the newest CACHE_KEEP; anything else in the
// folder goes (a draw that landed after its file changed, a leftover). Files
// starting with "." are writes in flight (copies, a diagram's partial PNG).
async function keepCached(tab, names) {
  const { recent, shown } = cacheRecord();
  const nowShown = withTab(shown, tab, names);
  const pinned = new Set(Object.values(nowShown).flat());
  const spare = recent.filter((n) => !pinned.has(n)).slice(0, CACHE_KEEP);
  setShared(CACHED, { recent: [...new Set([...names, ...recent])].filter((n) => pinned.has(n) || spare.includes(n)), shown: nowShown });
  const dir = `${appHome()}/${CACHE}`;
  const files = await fs.readdir(dir).catch(() => []);
  // The record as it is now: another preview may have noted a file since.
  const now = cacheRecord();
  const keep = new Set([...now.recent, ...Object.values(now.shown).flat()]);
  for (const f of files) if (!f.startsWith(".") && !keep.has(f)) await fs.unlink(`${dir}/${f}`).catch(() => {});
}

// An image copied into the cache: its name there.
async function cached(bytes, ext) {
  const name = `${hashOf(bytes)}.${ext}`;
  const dir = `${appHome()}/${CACHE}`;
  noteCached(name);
  if (!(await fs.exists(`${dir}/${name}`))) {
    await fs.mkdir(dir, { recursive: true });
    await fs.writeFile(`${dir}/${name}`, bytes);
  }
  return name;
}

// The diagram `name` (one of diagramNames(code)) drawn, or found drawn, into
// the cache: {width, height} in pixels as shown (drawn at twice that), or
// {error}.
async function diagram(code, mode, name) {
  const dir = `${appHome()}/${CACHE}`;
  if (!(await fs.exists(`${dir}/${name}`))) {
    noteCached(name);
    await fs.mkdir(dir, { recursive: true });
    const error = await drawMermaid(code, mode, `${dir}/${name}`);
    if (error) return { error };
  }
  const size = pngSize(await fs.readFile(`${dir}/${name}`).catch(() => null));
  return size ? { width: size.width / 2, height: size.height / 2 } : { error: "The diagram was not drawn." };
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

// {kind: "markdown", text} or, with mermaid blocks, {kind: "markdown",
// parts} (a diagram's part carries its pictures' names), {kind: "image",
// name}, or {error}.
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
    return { kind, parts: parts.map((p) => (p.mermaid === undefined ? p : { ...p, names: diagramNames(p.mermaid) })) };
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
    // The diagrams drawn for what the pane shows, by picture name:
    // {width, height} or {error}. A reload keeps those it still shows.
    this.drawn = new Map();
    this.wake = null;
    setShared(SHOWN, withTab(shared(SHOWN), this.tab, this.file));
    cx.spawn(async (cx) => {
      // A changed path of the folder's root means "anything may have changed".
      const ours = (changed) => !changed.length || changed.some((p) => p === this.file || p === dirname(this.file));
      while (!this.closed) {
        const s = await load(this.file);
        if (this.closed) break;
        this.shown = s;
        const names = this.pictures();
        for (const name of this.drawn.keys()) if (!names.includes(name)) this.drawn.delete(name);
        keepCached(this.tab, names); // the pane shows first, the clean-up after
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
      const mode = this.mode;
      const part = this.shown?.parts?.find((p) => p.names && !this.drawn.has(p.names[mode]));
      if (!part) {
        await new Promise((resolve) => (this.wake = resolve));
        continue;
      }
      const name = part.names[mode];
      const d = await diagram(part.mermaid, mode, name).catch((e) => ({ error: String(e?.message ?? e) }));
      if (this.closed) break;
      this.drawn.set(name, d);
      cx.notify();
    }
  }

  // The cache files of what the pane shows: an image, or each diagram's
  // pictures in both themes (so switching back finds them still there).
  pictures() {
    const s = this.shown;
    if (s?.kind === "image") return [s.name];
    return (s?.parts ?? []).flatMap((p) => (p.names ? [p.names.dark, p.names.light] : []));
  }

  // Markdown with mermaid blocks: the text between them as markdown, each
  // diagram as its picture, at most the pane's width, all in one scroll.
  parts(s, t) {
    const room = this.width ? Math.max(40, this.width - 2 * PAD_X) : null;
    const diagramOf = (p, i) => {
      const name = p.names[this.mode];
      const d = this.drawn.get(name);
      if (!d) return div().py(6).text_color(t.dim).child("Drawing diagram…");
      if (d.error) {
        return v_flex()
          .gap(4)
          .child(div().text_color(t.ansi1).child(`Mermaid: ${d.error}`))
          .child(markdown(`herdr-nvim-preview-${i}`, "```mermaid\n" + p.mermaid + "\n```"));
      }
      const w = room ? Math.min(d.width, room) : d.width;
      return div().py(6).child(image(`${CACHE}/${name}`).w(w).h((d.height * w) / d.width));
    };
    return v_flex()
      .id("herdr-nvim-preview-parts")
      .flex_1()
      .min_h(0)
      .w_full()
      .px(PAD_X)
      .py(12)
      .overflow_y_scrollbar()
      .on_size_change((size, cx) => {
        if (size.width !== this.width) {
          this.width = size.width;
          cx.notify();
        }
      })
      .children(s.parts.map((p, i) => (p.names ? diagramOf(p, i) : markdown(`herdr-nvim-preview-${i}`, p.md))));
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
            : markdown("herdr-nvim-preview", s.text).scrollable(true).flex_1().min_h(0).px(PAD_X).py(12);
    return v_flex()
      .id("herdr-nvim-preview")
      .size_full()
      .bg(t.background)
      .text_size(13)
      .text_color(t.foreground)
      .child(body);
  }
}
