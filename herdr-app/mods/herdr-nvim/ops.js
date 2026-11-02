// The tree's client for herdr-nvim's file operations: each runs in the tab's
// nvim (`herdr-nvim fs`), so buffers and language servers follow. One place
// that knows the request shapes (lua/herdr-nvim/fs.lua) and where a path may go.

import { exec, focused, notify, setShared, shared, window } from "herdr";
import { bin, nvim } from "./lib.js";

// The tab the tree works for, with its folder: what `--ctx` takes.
export function tabOf(place) {
  return { space: place.space ?? focused()?.space, tab: place.tab, terminal: null, cwd: place.cwd };
}

// Runs one operation; resolves with its JSON answer, or null (a toast said why).
export async function fsOp(place, request) {
  const out = await nvim("fs", [JSON.stringify({ root: place.cwd, ...request })], tabOf(place));
  return out === null ? null : JSON.parse(out);
}

export async function findFiles(root, query) {
  const r = await exec(bin(), ["app-find", "--root", root, "--query", query]).catch(() => null);
  return r?.code === 0 ? JSON.parse(r.stdout).paths : [];
}

// The file the tab's nvim shows, and the file to watch for its changes.
export async function current(place) {
  const out = await nvim("app-current", [], tabOf(place));
  return out === null ? { file: null, watch: null } : JSON.parse(out);
}

// herdr-nvim's `tree` setting ("native" or "nvim"), read from its config at
// the moment it matters and shared with every copy of the mod
// ("herdr-nvim.tree"), so a change applies without reinstalling.
export async function treeMode() {
  const r = await exec(bin(), ["app-config"]).catch(() => null);
  const tree = r?.code === 0 ? JSON.parse(r.stdout).tree : "native";
  if (shared("herdr-nvim.tree") !== tree) setShared("herdr-nvim.tree", tree);
  return tree;
}

export async function systemOpen(path) {
  const opener = window()?.platform === "windows" ? "explorer.exe" : window()?.platform === "linux" ? "xdg-open" : "open";
  const r = await exec(opener, [path]).catch((e) => ({ code: 1, stderr: String(e) }));
  if (r.code !== 0) notify({ title: "Cannot open", body: r.stderr || path, level: "error" });
}

// Whether `path` is the folder `root` or inside it.
export function inside(path, root) {
  return path === root || path.startsWith(`${root.replace(/\/$/, "")}/`);
}

export function join(dir, name) {
  const parts = [];
  for (const part of `${dir}/${name}`.split("/")) {
    if (part === "..") parts.pop();
    else if (part && part !== ".") parts.push(part);
  }
  return `/${parts.join("/")}`;
}
