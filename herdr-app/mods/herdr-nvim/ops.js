// The tree's client for herdr-nvim's file operations: each runs in the tab's
// nvim (`herdr-nvim fs`), so buffers and language servers follow. The one
// place that knows the request shapes (lua/herdr-nvim/fs.lua, `request`) and
// where a path may go.

import { timedExec } from "./profile.js";
import { exec, focused, notify, readTextFile, setShared, shared, window } from "herdr";
import { bin, nvim } from "./lib.js";

// The tab the tree works for: what `--ctx` takes. No folder: herdr-nvim
// finds where the tab works from its terminals (where its nvim starts). The
// tree's root is not it: one the person moved to, or a stopped nvim's.
export function tabOf(place) {
  return { space: place.space ?? focused()?.space, tab: place.tab, terminal: null, cwd: null };
}

// Every request the tree makes, by op (fs.lua's `ops`).
export const request = {
  defaults: () => ({ op: "defaults" }),
  // A name ending in / is a folder.
  create: (dir, name) => ({ op: "create", path: join(dir, name), folder: name.endsWith("/") }),
  rename: (from, to) => ({ op: "rename", path: from, to }),
  copy: (from, to) => ({ op: "copy", path: from, to }),
  move: (paths, dir) => ({ op: "move", paths, dir }),
  copyInto: (paths, dir) => ({ op: "copy_into", paths, dir }),
  delete: (paths) => ({ op: "delete", paths }),
  yank: (paths) => ({ op: "yank", paths }),
  paste: (dir) => ({ op: "paste", dir }),
};

// The request a dialog's typed value makes, from the dialog's action:
// {type: "create", dir} / {type: "rename" | "copy", from, base} take the
// name typed; {type: "request", request} is ready.
export function requestFor(action, value) {
  const v = value.trim();
  if (action.type === "create") return request.create(action.dir, v);
  if (action.type === "rename") return request.rename(action.from, join(action.base, v));
  if (action.type === "copy") return request.copy(action.from, join(action.base, v));
  return action.request;
}

// Every path a request writes to or takes from.
export function pathsOf(r) {
  return [r.path, r.to, r.dir, ...(r.paths ?? [])].filter(Boolean);
}

// Runs one operation; resolves with its JSON answer, or null (a toast said why).
export async function fsOp(place, request, opts = {}) {
  const out = await nvim("fs", [JSON.stringify({ root: place.cwd, ...request })], tabOf(place), opts);
  return out === null ? null : JSON.parse(out);
}

export async function findFiles(root, query) {
  const r = await timedExec("tree", "herdr-nvim app-find", bin(), ["app-find", "--root", root, "--query", query]).catch(() => null);
  return r?.code === 0 ? JSON.parse(r.stdout).paths : [];
}

// The file the tab's nvim shows, and the file to watch for its changes.
export async function current(place) {
  const out = await nvim("app-current", [], tabOf(place));
  return out === null ? { file: null, watch: null } : JSON.parse(out);
}

// herdr-nvim's `tree` setting ("native" or "nvim"), read at the moment it
// matters and shared with every copy of the mod ("herdr-nvim.tree"), so a
// change applies without reinstalling. `app-config` runs again only when the
// config file's text changed (read, not run), not on every action.
let config = null; // {path, text, tree, across}

// The settings the mod uses: {tree, across} (across: the direction a
// Preview / Source swap splits in, `sidebar::swap_direction`).
export async function appConfig() {
  const text = config?.path ? await readTextFile(config.path).catch(() => null) : undefined;
  if (config && text === config.text) return config;
  const r = await timedExec("wait", "herdr-nvim app-config", bin(), ["app-config"]).catch(() => null);
  let out = {};
  try { out = r?.code === 0 ? JSON.parse(r.stdout) : {}; } catch {}
  const tree = out.tree ?? "native";
  const path = out.config ?? null;
  const read = { path, tree, tree_key: out.tree_key ?? "<leader>e", across: out.across ?? "down", text: path ? await readTextFile(path).catch(() => null) : undefined };
  config = r?.code === 0 ? read : null;
  if (shared("herdr-nvim.tree") !== tree) setShared("herdr-nvim.tree", tree);
  return read;
}

export async function treeMode() {
  return (await appConfig()).tree;
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
