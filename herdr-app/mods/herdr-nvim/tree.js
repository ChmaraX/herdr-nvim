// The folder data behind the file tree: one folder's entries and the repo's
// git marks.

import { exec } from "herdr";
import { bin } from "./lib.js";

// One folder as the tree shows it (`herdr-nvim app-list`, src/app/list.rs):
// {entries: [{name, dir, hidden, ignored}], folders first, then by name, at
// most 500; ignored: the names git ignores}.
export async function listFolder(path) {
  const r = await exec(bin(), ["app-list", path]);
  if (r.code !== 0) throw new Error(r.stderr.trim().replace(/^herdr-nvim: /, "") || `Cannot list ${path}`);
  const listing = JSON.parse(r.stdout);
  return {
    entries: listing.entries,
    ignored: new Set(listing.entries.filter((e) => e.ignored).map((e) => e.name)),
  };
}

// A folder's repo top (null outside one).
const tops = new Map();
// Only a repo found is kept: a folder may become one (`git init`).
async function topOf(cwd) {
  if (!tops.has(cwd)) {
    const top = await exec("git", ["rev-parse", "--show-toplevel"], { cwd }).catch(() => null);
    if (top?.code !== 0) return null;
    tops.set(cwd, top.stdout.trim());
  }
  return tops.get(cwd);
}

// Absolute path -> "M" | "A" | "D" | "U" for files git reports, {} outside a repo.
export async function gitMarks(cwd) {
  const top = await topOf(cwd);
  if (!top) return {};
  // Writes nothing under .git, so the folder watch doesn't fire again.
  const status = await exec("git", ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=normal"], { cwd }).catch(() => null);
  const marks = {};
  if (status?.code !== 0) return marks;
  for (const entry of status.stdout.split("\0").filter(Boolean)) {
    const xy = entry.slice(0, 2);
    // An untracked folder comes once, as `dir/`: its row gets the mark.
    marks[`${top}/${entry.slice(3).replace(/\/$/, "")}`] = xy === "??" || xy.includes("A") ? "A" : xy.includes("U") ? "U" : xy.includes("D") ? "D" : "M";
  }
  return marks;
}

// A row's mark: its own, else "A" inside a new folder (git lists that
// folder alone, not the files in it).
export function markOf(marks, abs) {
  if (marks[abs]) return marks[abs];
  for (let dir = abs.replace(/\/[^/]*$/, ""); dir.includes("/"); dir = dir.replace(/\/[^/]*$/, "")) {
    if (marks[dir] === "A") return "A";
  }
  return null;
}
