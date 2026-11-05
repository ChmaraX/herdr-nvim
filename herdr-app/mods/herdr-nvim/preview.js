// The preview: a file shown as what it is, in the tab's nvim's place
// (the controller, sync.js `startSwap`, splits nvim's pane with this mod, props
// {preview: <file>, tab}, then closes nvim's pane; nvim keeps running).
// Markdown is rendered, images and SVGs drawn. It reloads when the file
// changes (saved in nvim, written by an agent). The pane header's Source
// button (mods/herdr-nvim-header) puts nvim back.
//
// Images are drawn with image(), which takes a path inside the app home: a
// copy goes to `mods/herdr-nvim/.cache` (hidden, so writing there reloads no
// mod), named by its content, the newest CACHE_KEEP kept.

import { div, image } from "gpui-kit";
import { v_flex, TextView } from "gpui-base";
import { timedExec } from "./profile.js";
import { appHome, panes, setShared, shared, theme, watchFolder } from "herdr";
import * as fs from "fs/promises";

const KINDS = {
  md: "markdown", markdown: "markdown", mdx: "markdown",
  png: "image", jpg: "image", jpeg: "image", gif: "image", webp: "image", bmp: "image", ico: "image", svg: "image",
};
const MAX_BYTES = { markdown: 2 * 1024 * 1024, image: 30 * 1024 * 1024 };
const CACHE = "mods/herdr-nvim/.cache";
const CACHE_KEEP = 40;
// shared(): which file each tab's preview shows, for its pane header.
const SHOWN = "herdr-nvim.previews";

// "markdown", "image", or null: what a file previews as.
export function previewKind(path) {
  const ext = /\.([^./]+)$/.exec(path ?? "")?.[1]?.toLowerCase();
  return (ext && KINDS[ext]) || null;
}

const dirname = (p) => p.replace(/\/[^/]*$/, "") || "/";

// FNV-1a over the bytes: the cache name of an image's content.
function hashOf(bytes) {
  let h = 0x811c9dc5;
  for (let i = 0; i < bytes.length; i++) h = Math.imul(h ^ bytes[i], 0x01000193) >>> 0;
  return h.toString(16).padStart(8, "0") + bytes.length.toString(16);
}

async function cached(bytes, ext) {
  const name = `${hashOf(bytes)}.${ext}`;
  const dir = `${appHome()}/${CACHE}`;
  if (!(await fs.exists(`${dir}/${name}`))) {
    await fs.mkdir(dir, { recursive: true });
    await fs.writeFile(`${dir}/${name}`, bytes);
  }
  // The newest first; the rest go (every preview's names, so none on screen).
  const keep = [name, ...((shared("herdr-nvim.preview-cache") ?? []).filter((n) => n !== name))];
  for (const old of keep.slice(CACHE_KEEP)) await fs.unlink(`${dir}/${old}`).catch(() => {});
  setShared("herdr-nvim.preview-cache", keep.slice(0, CACHE_KEEP));
  return `${CACHE}/${name}`;
}

// A mod reads files only inside the home folder: one elsewhere (/tmp, a
// screenshot under /private/var/folders) is read through a process.
async function readText(file) {
  try {
    return await fs.readFile(file, "utf8");
  } catch {
    const r = await timedExec("preview", "preview: read file", "cat", [file]);
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
    const r = await timedExec("preview", "preview: copy image", "cp", [file, copy]);
    if (r.code !== 0) throw new Error(r.stderr);
    try {
      return await fs.readFile(copy);
    } finally {
      await fs.unlink(copy).catch(() => {});
    }
  }
}

// {kind, text} / {kind, src}, or {error}.
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
  if (kind === "markdown") return { kind, text: data };
  const ext = /\.([^./]+)$/.exec(file)[1].toLowerCase();
  return { kind, src: await cached(data, ext) };
}

export class Preview {
  constructor(props, cx) {
    this.file = props.preview;
    this.tab = props.tab;
    this.shown = null;
    // Only tabs that still show a preview keep their entry.
    const showing = new Set(panes().filter((p) => p.terminal === null && p.mod === "herdr-nvim").map((p) => p.tab));
    const kept = Object.fromEntries(Object.entries(shared(SHOWN) ?? {}).filter(([tab]) => showing.has(tab)));
    setShared(SHOWN, { ...kept, [this.tab]: this.file });
    cx.spawn(async (cx) => {
      // A changed path of the folder's root means "anything may have changed".
      const ours = (changed) => !changed.length || changed.some((p) => p === this.file || p === dirname(this.file));
      while (!this.closed) {
        this.shown = await load(this.file);
        cx.notify();
        while (!this.closed && !ours(await watchFolder(dirname(this.file))));
      }
    });
  }

  // The pane is gone (main.js `close`): the watch loop ends.
  close() {
    this.closed = true;
  }

  render() {
    const t = theme();
    const s = this.shown;
    const body = !s
      ? div()
      : s.error
        ? div().m_auto().text_color(t.dim).child(s.error)
        : s.kind === "image"
          ? div().flex_1().min_h(0).w_full().p(14).child(image(s.src).size_full())
          : TextView.markdown("herdr-nvim-preview", s.text).selectable(true).scrollable(true).flex_1().min_h(0).px(18).py(12);
    return v_flex()
      .id("herdr-nvim-preview")
      .size_full()
      .bg(t.background)
      .text_size(13)
      .text_color(t.foreground)
      .child(body);
  }
}
