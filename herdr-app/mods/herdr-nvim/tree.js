// The folder data behind the file tree: one folder's entries and the repo's
// git marks. Listing follows the shipped `files` mod (Apache-2.0, Herdr): a
// POSIX sh glob, `.git` skipped, at most 500 entries per folder.

import { exec } from "herdr";

export const PER_FOLDER = 500;

const LIST = 'n=0; for p in "$1"/* "$1"/.[!.]* "$1"/..?*; do [ -e "$p" ] || [ -L "$p" ] || continue; '
  + 'name=${p##*/}; [ "$name" = .git ] && continue; '
  + 'if [ -d "$p" ] && [ ! -L "$p" ]; then kind=d; else kind=f; fi; printf "%s\\0%s\\0" "$kind" "$name"; '
  + 'n=$((n+1)); [ "$n" -ge ' + PER_FOLDER + ' ] && break; done; test -d "$1"';

// Folders first, then by name.
export async function listFolder(path) {
  const r = await exec("sh", ["-c", LIST, "herdr-nvim", path]);
  if (r.code !== 0) throw new Error(r.stderr.trim() || `Cannot list ${path}`);
  const fields = r.stdout.split("\0"), out = [];
  for (let i = 0; i + 1 < fields.length; i += 2) out.push({ name: fields[i + 1], dir: fields[i] === "d" });
  return out.sort((a, b) => Number(b.dir) - Number(a.dir) || a.name.localeCompare(b.name));
}

// Absolute path -> "M" | "A" | "D" | "U" for files git reports, {} outside a repo.
export async function gitMarks(cwd) {
  const top = await exec("git", ["rev-parse", "--show-toplevel"], { cwd }).catch(() => null);
  if (top?.code !== 0) return {};
  // Writes nothing under .git, so the folder watch doesn't fire again.
  const status = await exec("git", ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=normal"], { cwd }).catch(() => null);
  const marks = {};
  if (status?.code !== 0) return marks;
  for (const entry of status.stdout.split("\0").filter(Boolean)) {
    const xy = entry.slice(0, 2);
    marks[`${top.stdout.trim()}/${entry.slice(3)}`] = xy === "??" || xy.includes("A") ? "A" : xy.includes("U") ? "U" : xy.includes("D") ? "D" : "M";
  }
  return marks;
}
