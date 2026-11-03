// The folder data behind the file tree: one folder's entries and the repo's
// git marks. Listing follows the shipped `files` mod (Apache-2.0, Herdr): a
// POSIX sh glob, `.git` skipped, at most 500 entries per folder.

import { exec } from "herdr";

export const PER_FOLDER = 500;

// One process per folder: its entries ("d"/"f" + name), then an "x" record,
// then the names git ignores there (`check-ignore -z`, nothing outside a repo).
const LIST = 'dir=$1; shift; n=0; for p in "$dir"/* "$dir"/.[!.]* "$dir"/..?*; do [ -e "$p" ] || [ -L "$p" ] || continue; '
  + 'name=${p##*/}; [ "$name" = .git ] && continue; '
  + 'if [ -d "$p" ] && [ ! -L "$p" ]; then kind=d; else kind=f; fi; printf "%s\\0%s\\0" "$kind" "$name"; set -- "$@" "$name"; '
  + 'n=$((n+1)); [ "$n" -ge ' + PER_FOLDER + ' ] && break; done; test -d "$dir" || exit 1; printf "x\\0x\\0"; '
  + '[ "$#" -gt 0 ] && printf "%s\\0" "$@" | git -C "$dir" check-ignore -z --stdin 2>/dev/null; exit 0';

// {entries: folders first, then by name; ignored: the names git ignores}.
export async function listFolder(path) {
  const r = await exec("sh", ["-c", LIST, "herdr-nvim", path]);
  if (r.code !== 0) throw new Error(r.stderr.trim() || `Cannot list ${path}`);
  const fields = r.stdout.split("\0"), entries = [];
  let i = 0;
  for (; i + 1 < fields.length && fields[i] !== "x"; i += 2) entries.push({ name: fields[i + 1], dir: fields[i] === "d" });
  const ignored = new Set(fields.slice(i + 2).filter(Boolean).map((name) => name.replace(/\/$/, "")));
  entries.sort((a, b) => Number(b.dir) - Number(a.dir) || a.name.localeCompare(b.name));
  return { entries, ignored };
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
    marks[`${top}/${entry.slice(3)}`] = xy === "??" || xy.includes("A") ? "A" : xy.includes("U") ? "U" : xy.includes("D") ? "D" : "M";
  }
  return marks;
}
