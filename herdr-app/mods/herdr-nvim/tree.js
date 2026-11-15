// The folder data behind the file tree: one folder's entries and the repo's
// git marks. Listing follows the shipped `files` mod (Apache-2.0, Herdr): a
// POSIX sh glob, `.git` skipped, at most 500 entries per folder.

import { exec, window } from "herdr";

const PER_FOLDER = 500;

export function normalizePath(p) {
  if (!p || typeof p !== "string") return p;
  return p.replace(/^\\\\\?\\/, "").replace(/\\/g, "/").replace(/\/$/, "");
}

async function listFolderWindows(dirPath) {
  const clean = normalizePath(dirPath);
  const escaped = clean.replace(/['‘’‚‛]/g, "$&$&");
  const ps = `[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); & { param($p) @(Get-ChildItem -LiteralPath $p -Force -ErrorAction Stop | Where-Object Name -ne '.git' | Select-Object -First 501 | ForEach-Object { @{ name=$_.Name; dir=($_.PSIsContainer -and (-not ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -or -not $_.LinkType -or ($_.Target -and [IO.Directory]::Exists(([IO.Path]::Combine([IO.Path]::GetDirectoryName($_.FullName), $_.Target[0])))))) } }) | ConvertTo-Json -Compress } '` + escaped + `'`;
  const r = await exec("powershell", ["-NoProfile", "-Command", ps]).catch((e) => ({ code: 1, stderr: String(e), stdout: "" }));
  if (r.code !== 0) throw new Error(r.stderr.trim() || `Cannot list ${dirPath}`);
  const raw = JSON.parse(r.stdout || "[]");
  const list = Array.isArray(raw) ? raw : [raw];
  const entries = list.map((e) => ({ name: String(e.name), dir: Boolean(e.dir) }));
  entries.sort((a, b) => Number(b.dir) - Number(a.dir) || a.name.localeCompare(b.name));

  const ignored = new Set();
  const top = await topOf(clean);
  if (top && entries.length > 0) {
    const names = entries.map((e) => (e.dir ? `${e.name}/` : e.name));
    const gitRes = await exec("git", ["-C", clean, "check-ignore", "--", ...names.slice(0, 500)]).catch(() => null);
    if (gitRes && (gitRes.code === 0 || gitRes.code === 1) && gitRes.stdout) {
      for (const line of gitRes.stdout.split("\n")) {
        const item = line.trim().replace(/^["']|["']$/g, "").replace(/\/$/, "");
        if (item) ignored.add(item);
      }
    }
  }
  return { entries, ignored };
}

// One process per folder: its entries ("d"/"f" + name), then an "x" record,
// then the names git ignores there (`check-ignore -z`, nothing outside a repo).
const LIST = 'dir=$1; shift; n=0; for p in "$dir"/* "$dir"/.[!.]* "$dir"/..?*; do [ -e "$p" ] || [ -L "$p" ] || continue; '
  + 'name=${p##*/}; [ "$name" = .git ] && continue; '
  + 'if [ -d "$p" ] && [ ! -L "$p" ]; then kind=d; else kind=f; fi; printf "%s\\0%s\\0" "$kind" "$name"; set -- "$@" "$name"; '
  + 'n=$((n+1)); [ "$n" -ge ' + PER_FOLDER + ' ] && break; done; test -d "$dir" || exit 1; printf "x\\0x\\0"; '
  + '[ "$#" -gt 0 ] && printf "%s\\0" "$@" | git -C "$dir" check-ignore -z --stdin 2>/dev/null; exit 0';

// {entries: folders first, then by name; ignored: the names git ignores}.
export async function listFolder(path) {
  const norm = normalizePath(path);
  if (window()?.platform === "windows") {
    return listFolderWindows(norm);
  }
  const r = await exec("sh", ["-c", LIST, "herdr-nvim", norm]);
  if (r.code !== 0) throw new Error(r.stderr.trim() || `Cannot list ${norm}`);
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
  const norm = normalizePath(cwd);
  if (!tops.has(norm)) {
    const top = await exec("git", ["rev-parse", "--show-toplevel"], { cwd: norm }).catch(() => null);
    if (top?.code !== 0) return null;
    tops.set(norm, normalizePath(top.stdout.trim()));
  }
  return tops.get(norm);
}

// Absolute path -> "M" | "A" | "D" | "U" for files git reports, {} outside a repo.
export async function gitMarks(cwd) {
  const norm = normalizePath(cwd);
  const top = await topOf(norm);
  if (!top) return {};
  // Writes nothing under .git, so the folder watch doesn't fire again.
  const status = await exec("git", ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=normal"], { cwd: norm }).catch(() => null);
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
  const norm = normalizePath(abs);
  if (marks[norm]) return marks[norm];
  for (let dir = norm.replace(/\/[^/]*$/, ""); dir.includes("/"); dir = dir.replace(/\/[^/]*$/, "")) {
    if (marks[dir] === "A") return "A";
  }
  return null;
}
