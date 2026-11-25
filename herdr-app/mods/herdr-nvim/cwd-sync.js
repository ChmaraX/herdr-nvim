// The tab's nvim follows a `cd` in the tab's terminal. It stays parked, in
// its first folder, while Files is closed, and the tree is rooted at nvim's
// folder; so each time Files shows it (sync.js `look()`), nvim moves to the
// tab's folder for where the tab's terminal is now (fs.lua `ops.cd`).
// The app's `panes().cwd` doesn't follow a `cd`: herdr-core is asked.

import { exec, hc } from "herdr";
import { nvimTerminals, ownPanes } from "./panel.js";
import { bin } from "./path.js";

// The tab's own terminals: not its nvim, not a mod pane, not remote.
export const ownTerminals = (tab) => ownPanes(tab).map((p) => p.terminal).filter((t) => t && !t.includes("/"));

// The terminal nvim follows: the one last focused (`last`), else the first.
export function shellOf(tab, last) {
  const own = ownTerminals(tab);
  return own.includes(last) ? last : own[0] ?? null;
}

// Where the terminal's program is now. Not `cwd`: that is where it started.
async function folderOf(terminal) {
  const r = await hc(["get", terminal]);
  return r.code === 0 ? JSON.parse(r.stdout).foreground?.cwd ?? null : null;
}

// Moves the tab's nvim (waiting up to 3 s for Files to start it; never
// starting one) to `shell`'s folder. Silent: the person didn't ask for it,
// so a failure (nvim at a prompt) is not worth a toast.
export async function alignCwd(at, shell, cx) {
  const path = shell && (await folderOf(shell));
  if (!path) return;
  for (const end = Date.now() + 3000; !nvimTerminals(at.tab).length; ) {
    if (Date.now() > end) return;
    await cx.sleep(100);
  }
  const ctx = JSON.stringify({ space: at.space, tab: at.tab, terminal: null, cwd: null });
  await exec(bin(), ["fs", JSON.stringify({ op: "cd", path }), "--ctx", ctx]);
}
