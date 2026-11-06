// What the tree shows: a root folder (the tab's folder, or one the person
// moved to with `.` / Backspace), the folders that are open, the entries of
// each, git marks, which entries git ignores, and the hidden/ignored toggles.
// Paths are absolute; a folder's entries are listed on first open and again
// when something under the root changes (a folder watch, no polling).

import { gitMarks, listFolder } from "./tree.js";

const ROWS = 3000;

// `git status` runs at most this often for file events (a build or an agent
// writing files is a lot of them).
export const GIT_EVERY_MS = 2000;

// How many folders list at once.
const AT_ONCE = 6;

async function each(items, fn) {
  const queue = [...items];
  await Promise.all(Array.from({ length: Math.min(AT_ONCE, queue.length) }, async () => {
    while (queue.length) await fn(queue.shift());
  }));
}

export function dirname(path) {
  return path.replace(/\/[^/]*$/, "") || "/";
}

export class TreeModel {
  constructor() {
    this.reset(null);
    this.showHidden = false;
    this.showIgnored = false;
  }

  reset(root) {
    this.version = (this.version ?? 0) + 1;
    this.root = root;
    this.marksAt = 0;
    this.entries = new Map(); // folder -> [{name, dir}]
    this.loads = new Map(); // folder -> the latest listing's number
    this.ignored = new Map(); // folder -> Set of ignored names
    this.open = new Set();
    this.marks = {};
  }

  // Lists `folder` (and which of its entries git ignores).
  // The latest listing of a folder wins, whichever finishes first; one that
  // changed nothing changes no rows. Returns whether it changed any.
  async load(folder) {
    const root = this.root, seq = (this.loads.get(folder) ?? 0) + 1;
    this.loads.set(folder, seq);
    const { entries, ignored } = await listFolder(folder);
    if (this.root !== root || this.loads.get(folder) !== seq) return false;
    const same = (a, b) => a && a.length === b.length && a.every((e, i) => e.name === b[i].name && e.dir === b[i].dir);
    const was = this.ignored.get(folder);
    if (same(this.entries.get(folder), entries) && was && was.size === ignored.size && [...ignored].every((n) => was.has(n))) return false;
    this.entries.set(folder, entries);
    this.ignored.set(folder, ignored);
    this.version++;
    return true;
  }

  async reload() {
    await each([this.root, ...this.open], (folder) => this.load(folder).catch(() => {}));
    await this.refreshMarks();
  }

  async refreshMarks() {
    const root = this.root;
    this.marksAt = Date.now();
    const marks = await gitMarks(root);
    if (this.root === root) this.marks = marks;
  }

  // After a watch batch (absolute paths; one that holds the root means
  // everything): lists again only the folders on screen a changed path is in
  // (or is), so events inside closed folders list nothing. Git marks follow
  // unless they ran less than GIT_EVERY_MS ago: then the result says "later"
  // and the caller runs refreshMarks once that time is up.
  async reloadChanged(changed) {
    const root = this.root;
    if (!changed?.length || changed.includes(root)) {
      await this.reload();
      return { marks: "done" };
    }
    // Paths inside a folder git ignores (target/, node_modules/: a build) or
    // inside .git change nothing the tree shows unless ignored files show.
    const unseen = (path) => {
      if (path.includes("/.git/") || path.endsWith("/.git")) return true;
      if (this.showIgnored || !path.startsWith(`${root}/`)) return false;
      let folder = root;
      for (const part of path.slice(root.length + 1).split("/")) {
        if (this.ignored.get(folder)?.has(part)) return true;
        folder = `${folder}/${part}`;
      }
      return false;
    };
    const seen = changed.filter((path) => !unseen(path));
    if (!seen.length) return { marks: "none" };
    const listed = new Set([root, ...this.open]);
    const touched = new Set();
    for (const path of seen) for (const folder of [dirname(path), path]) if (listed.has(folder)) touched.add(folder);
    await each(touched, (folder) => this.load(folder).catch(() => {}));
    if (Date.now() - this.marksAt < GIT_EVERY_MS) return { marks: "later" };
    await this.refreshMarks();
    return { marks: "done" };
  }

  shown(folder, entry) {
    if (!this.showHidden && entry.name.startsWith(".")) return false;
    if (!this.showIgnored && this.ignored.get(folder)?.has(entry.name)) return false;
    return true;
  }

  // The rows on screen, depth first through the open folders; built again
  // only after something they show changed (a render builds none).
  rows() {
    const key = `${this.version}:${this.showHidden}:${this.showIgnored}`;
    if (this.rowsKey !== key) {
      this.rowsKey = key;
      this.rowsCache = this.walk();
    }
    return this.rowsCache;
  }

  walk(folder = this.root, depth = 0, out = []) {
    for (const entry of this.entries.get(folder) ?? []) {
      if (out.length >= ROWS) break;
      if (!this.shown(folder, entry)) continue;
      const abs = `${folder}/${entry.name}`;
      out.push({ ...entry, abs, depth, ignored: this.ignored.get(folder)?.has(entry.name) ?? false });
      if (entry.dir && this.open.has(abs)) this.walk(abs, depth + 1, out);
    }
    return out;
  }

  async toggle(abs) {
    if (this.open.has(abs)) return this.close(abs);
    this.open.add(abs);
    this.version++;
    if (!this.entries.has(abs)) await this.load(abs);
  }

  close(abs) {
    for (const folder of [...this.open]) if (folder === abs || folder.startsWith(`${abs}/`)) this.open.delete(folder);
    this.version++;
  }

  collapseAll() {
    this.open.clear();
    this.version++;
  }

  // Opens every folder on the way to `abs` (inside the root); true if it can
  // be shown.
  async expandTo(abs) {
    if (!this.root || !abs.startsWith(`${this.root}/`)) return false;
    const parts = abs.slice(this.root.length + 1).split("/");
    let folder = this.root;
    for (const part of parts.slice(0, -1)) {
      folder = `${folder}/${part}`;
      if (!this.open.has(folder)) {
        this.open.add(folder);
        this.version++;
      }
      if (!this.entries.has(folder)) await this.load(folder).catch(() => {});
    }
    const name = parts[parts.length - 1];
    if (name.startsWith(".")) this.showHidden = true;
    if (this.ignored.get(dirname(abs))?.has(name)) this.showIgnored = true;
    return true;
  }
}
