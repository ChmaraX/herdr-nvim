// The mods' paths, and where herdr-nvim's launcher is. herdr-nvim-header has
// this same file (a mod can't import another mod's files); js-checks keeps
// the two equal.
//
// A path here is absolute, with "/" between names and none at the end except
// a root's: "/", or on Windows a drive's "C:/" or a share's "//host/share/".
// On Windows the app, git and nvim name one file C:\x, \\?\C:\x, c:/x or a
// mix: `canonical` makes them one form where they enter a mod. Windows
// compares names case aside (`key`); macOS and Linux compare them exactly,
// as a volume there may be case-sensitive.

import { appHome, window } from "herdr";

export const windows = () => window()?.platform === "windows";

// herdr-nvim's launcher, which the mods run, and other mods too:
// `<appHome>/mods/herdr-nvim/bin/herdr-nvim open --tab <tab> <file>:<line>`.
// On Windows it is the .exe, named in full.
export function bin() {
  return windows() ? `${appHome()}\\mods\\herdr-nvim\\bin\\herdr-nvim.exe` : `${appHome()}/mods/herdr-nvim/bin/herdr-nvim`;
}

const ROOT_WINDOWS = /^(?:[A-Za-z]:|\/\/[^/]+\/[^/]+)?\//;
const rootOf = (path) => (windows() ? ROOT_WINDOWS.exec(path)?.[0] : path.startsWith("/") ? "/" : null) ?? "";

// On Windows "\" separates names too; elsewhere a name may hold it.
const slashes = (path) => (windows() ? path.replace(/\\/g, "/") : path);

export function canonical(path) {
  if (!path || !windows()) return path;
  const p = slashes(path.replace(/^\\\\\?\\UNC\\/i, "\\\\").replace(/^\\\\\?\\/, "")).replace(/^[a-z]:/, (d) => d.toUpperCase());
  if (/^[A-Z]:$/.test(p)) return `${p}/`;
  const trimmed = p.replace(/\/+$/, "");
  return rootOf(`${trimmed}/`) === `${trimmed}/` ? `${trimmed}/` : trimmed;
}

// What two paths are compared by. Each character folds alone, so a key is
// as long as its path.
export function key(path) {
  if (!windows()) return path;
  return path.replace(/./gsu, (c) => {
    const lower = c.toLowerCase();
    return lower.length === c.length ? lower : c;
  });
}

export function same(a, b) {
  return a === b || (Boolean(a && b) && key(a) === key(b));
}

// Whether `path` is the folder `root` or inside it.
export function inside(path, root) {
  const p = key(path), r = key(root);
  return p === r || p.startsWith(r.endsWith("/") ? r : `${r}/`);
}

// `path` from inside the folder `root` ("" for the root itself), or null
// when it isn't inside.
export function relative(path, root) {
  if (!inside(path, root)) return null;
  return same(path, root) ? "" : path.slice(root.endsWith("/") ? root.length : root.length + 1);
}

// The folder `path` is in; a root's is itself.
export function dirname(path) {
  const root = rootOf(path);
  const cut = path.lastIndexOf("/");
  return cut < root.length ? root || path : path.slice(0, cut);
}

// The last name in `path`; a root's is the root.
export function basename(path) {
  return rootOf(path) === path ? path : path.slice(path.lastIndexOf("/") + 1);
}

// The entry `name` (or names a listing or git gave, "/" between them) in the
// folder `dir`.
export function child(dir, name) {
  return dir.endsWith("/") ? dir + name : `${dir}/${name}`;
}

// `name` (a name, or a path typed in a dialog) in the folder `dir`, with
// "." and ".." resolved.
export function join(dir, name) {
  const root = rootOf(dir) || "/";
  const parts = [];
  for (const part of `${dir.slice(root.length)}/${slashes(name)}`.split("/")) {
    if (part === "..") parts.pop();
    else if (part && part !== ".") parts.push(part);
  }
  return root + parts.join("/");
}

// Whether a typed name asks for a folder: it ends with "/" (or "\" on Windows).
export function namesFolder(name) {
  return /\/$/.test(slashes(name));
}

// `path` as the system's own programs take it (Explorer reads "/" as an option).
export function native(path) {
  return windows() ? path.replace(/\//g, "\\") : path;
}

// `path` with the home folder `home` as "~".
export function tilde(path, home) {
  const rest = home ? relative(path, home) : null;
  return rest === null ? path : rest ? `~/${rest}` : "~";
}
