// herdr-nvim's mod helpers. Identical in both mod folders (mods can't import each
// other); `herdr-app/install.sh` refuses to install copies that differ.

import { appHome, exec, focused, notify } from "herdr";

// The install script writes the wrapper here (it bakes in the binary, the
// herdr-app path and this window's home). Other mods use it the same way:
// `<appHome>/mods/herdr-nvim/bin/herdr-nvim open --tab <tab> <file>:<line>`.
export function bin() {
  return `${appHome()}/mods/herdr-nvim/bin/herdr-nvim`;
}

// The focused tab as `herdr-nvim --ctx` takes it, or null without one.
export function context() {
  const f = focused();
  if (!f?.space || !f?.tab) return null;
  return JSON.stringify({ space: f.space, tab: f.tab, terminal: f.terminal ?? null, cwd: f.cwd ?? null });
}

// Runs `herdr-nvim <command> [args] --ctx <tab>` (the focused tab unless
// given); resolves with its stdout, or null after a toast saying what went
// wrong.
export async function nvim(command, args = [], tab = null) {
  const ctx = tab ? JSON.stringify(tab) : context();
  if (!ctx) {
    notify({ title: "herdr-nvim", body: "Open a tab first.", level: "warning" });
    return null;
  }
  try {
    const r = await exec(bin(), [command, ...args, "--ctx", ctx]);
    if (r.code === 0) return r.stdout;
    notify({ title: `herdr-nvim ${command} failed`, body: (r.stderr || r.stdout).trim().slice(-400), level: "error" });
  } catch (e) {
    notify({ title: "herdr-nvim is not installed here", body: `${bin()}: ${e}`, level: "error" });
  }
  return null;
}
