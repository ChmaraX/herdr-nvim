// The tab's nvim follows the tab's shell when Files shows it.
//
// Each tab has its own nvim (label `herdr-nvim.sidebar=<tab>`), started where
// the tab was the first time Files opened there, then parked while Files is
// closed. A `cd` in the tab's shell never reaches it, and the tree's root is
// nvim's folder. So each time Files shows a tab's nvim (Files opened, or a
// switch to a tab while Files is open), sync.js calls `alignCwd`: nvim `:cd`s
// to the tab's folder for where the tab's shell is now (its git repo's top,
// else that folder: fs.lua `ops.cd`) when it is elsewhere; its DirChanged
// then moves the tree's root (tree-view.js `watchCurrent`).
//
// The app's `panes().cwd` does not follow a `cd`, so the shell's folder is
// asked of herdr-core (`hc get`: its foreground program's folder).
//
// A split tab has several terminals, maybe in different folders: the one
// that counts is the one the person last worked in. sync.js `look()` runs on
// every focus change and keeps it per tab (`lastShell`).

import { hc } from "herdr";
import { nvim } from "./lib.js";
import { nvimTerminals, ownPanes } from "./panel.js";

// The tab's own terminals (not its nvim, not a mod pane), on this machine.
export const ownTerminals = (tab) => ownPanes(tab).map((p) => p.terminal).filter((t) => t && !t.includes("/"));

// The terminal whose folder the tab's nvim should be in: the tab's own
// terminal last focused (`last`, if still in the tab), else its first one.
export function shellOf(tab, last) {
  const own = ownTerminals(tab);
  return own.includes(last) ? last : own[0] ?? null;
}

// A terminal's folder now: its foreground program's, as herdr-core sees it.
async function folderOf(terminal) {
  const r = await hc(["get", terminal]);
  if (r.code !== 0) return null;
  const t = JSON.parse(r.stdout);
  return t.foreground?.cwd || t.cwd || null;
}

// Waits (at most 3 s) for the tab's nvim (Files may have just started it),
// then moves it to `shell`'s folder. Never starts an nvim of its own.
export async function alignCwd(at, shell, cx) {
  const path = shell && (await folderOf(shell));
  if (!path) return;
  for (const end = Date.now() + 3000; !nvimTerminals(at.tab).length; ) {
    if (Date.now() > end) return;
    await cx.sleep(100);
  }
  await nvim("fs", [JSON.stringify({ op: "cd", path })], { space: at.space, tab: at.tab, terminal: null, cwd: null }, { quiet: true });
}
