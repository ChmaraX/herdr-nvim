// What the tree shows: a root folder (the tab's folder, or one the person
// moved to with `.` / Backspace), the folders that are open, the entries of
// each, git marks, which entries git ignores, and the hidden/ignored toggles.
// Paths are absolute; a folder's entries are listed on first open and again
// when something under the root changes (a folder watch, no polling).

import { gitMarks, ignoredIn, listFolder } from "./tree.js";

export const ROWS = 3000;

// `git status` runs at most this often for file events (a build or an agent
// writing files is a lot of them).
export const GIT_EVERY_MS = 2000;

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
    this.root = root;
    this.marksAt = 0;
    this.entries = new Map(); // folder -> [{name, dir}]
    this.ignored = new Map(); // folder -> Set of ignored names
    this.open = new Set();
    this.marks = {};
  }

  // Lists `folder` (and which of its entries git ignores).
  async load(folder) {
    const root = this.root;
    const entries = await listFolder(folder);
    const ignored = await ignoredIn(folder, entries.map((e) => e.name));
    if (this.root !== root) return;
    this.entries.set(folder, entries);
    this.ignored.set(folder, ignored);
  }

  async reload() {
    const root = this.root;
    for (const folder of [root, ...this.open]) await this.load(folder).catch(() => {});
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
    const listed = new Set([root, ...this.open]);
    const touched = new Set();
    for (const path of changed) for (const folder of [dirname(path), path]) if (listed.has(folder)) touched.add(folder);
    for (const folder of touched) await this.load(folder).catch(() => {});
    if (Date.now() - this.marksAt < GIT_EVERY_MS) return { marks: "later" };
    await this.refreshMarks();
    return { marks: "done" };
  }

  shown(folder, entry) {
    if (!this.showHidden && entry.name.startsWith(".")) return false;
    if (!this.showIgnored && this.ignored.get(folder)?.has(entry.name)) return false;
    return true;
  }

  // The rows on screen, depth first through the open folders.
  rows(folder = this.root, depth = 0, out = []) {
    for (const entry of this.entries.get(folder) ?? []) {
      if (out.length >= ROWS) break;
      if (!this.shown(folder, entry)) continue;
      const abs = `${folder}/${entry.name}`;
      out.push({ ...entry, abs, depth, ignored: this.ignored.get(folder)?.has(entry.name) ?? false });
      if (entry.dir && this.open.has(abs)) this.rows(abs, depth + 1, out);
    }
    return out;
  }

  async toggle(abs) {
    if (this.open.has(abs)) return this.close(abs);
    this.open.add(abs);
    if (!this.entries.has(abs)) await this.load(abs);
  }

  close(abs) {
    for (const folder of [...this.open]) if (folder === abs || folder.startsWith(`${abs}/`)) this.open.delete(folder);
  }

  // Opens every folder on the way to `abs` (inside the root); true if it can
  // be shown.
  async expandTo(abs) {
    if (!this.root || !abs.startsWith(`${this.root}/`)) return false;
    const parts = abs.slice(this.root.length + 1).split("/");
    let folder = this.root;
    for (const part of parts.slice(0, -1)) {
      folder = `${folder}/${part}`;
      this.open.add(folder);
      if (!this.entries.has(folder)) await this.load(folder).catch(() => {});
    }
    const name = parts[parts.length - 1];
    if (name.startsWith(".")) this.showHidden = true;
    if (this.ignored.get(dirname(abs))?.has(name)) this.showIgnored = true;
    return true;
  }
}
