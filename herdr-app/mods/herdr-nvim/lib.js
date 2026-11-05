// herdr-nvim's mod helpers.

import { appHome, exec, focused, notify, setShared } from "herdr";
import { openCard } from "./card.js";
import { flush, instant, mark, profiling, timedExec } from "./profile.js";

// The install script writes the wrapper here (it bakes in the binary, the
// herdr-app path and this window's home). Other mods use it the same way:
// `<appHome>/mods/herdr-nvim/bin/herdr-nvim open --tab <tab> <file>:<line>`.
export function bin() {
  return `${appHome()}/mods/herdr-nvim/bin/herdr-nvim`;
}

// The focused tab as `herdr-nvim --ctx` takes it (parsed), or null without
// one. Capture it before any await: the person may switch tabs meanwhile.
export function place(f = focused()) {
  if (!f?.space || !f?.tab) return null;
  // A folder only with a terminal: it is that terminal's (a focused mod pane,
  // such as the tree, has none; herdr-nvim then asks the tab's terminals).
  const terminal = f.terminal ?? null;
  return { space: f.space, tab: f.tab, terminal, cwd: terminal ? f.cwd ?? null : null };
}

export function context() {
  const p = place();
  return p && JSON.stringify(p);
}

// A command's `notice` (e.g. the tab's nvim runs an older herdr-nvim and was
// kept for its unsaved work) as a warning toast.
function noticeIn(stdout) {
  let notice = null;
  try {
    notice = JSON.parse(stdout)?.notice;
  } catch {}
  if (notice) notify({ title: "herdr-nvim", body: notice, level: "warning", timeout: 15000 });
}

// Runs `herdr-nvim <command> [args] --ctx <tab>` (the focused tab unless
// given); resolves with its stdout, or null after a toast saying what went
// wrong. No Neovim to start the tab's nvim with (exit 3): the card that
// offers to install it instead, unless `quiet`.
export async function nvim(command, args = [], tab = null, { quiet = false } = {}) {
  const ctx = tab ? JSON.stringify(tab) : context();
  if (!ctx) {
    notify({ title: "herdr-nvim", body: "Open a tab first.", level: "warning" });
    return null;
  }
  try {
    const r = await timedExec("wait", `herdr-nvim ${command}`, bin(), [command, ...args, "--ctx", ctx]);
    if (r.code === 0) {
      noticeIn(r.stdout);
      return r.stdout;
    }
    if (r.code === 3) {
      let missing = null;
      try { missing = JSON.parse(r.stdout).nvim_missing; } catch {}
      if (missing) {
        if (!quiet) openCard({ kind: "nvim", ...missing });
        return null;
      }
    }
    notify({ title: `herdr-nvim ${command} failed`, body: (r.stderr || r.stdout).trim().slice(-400), level: "error" });
  } catch (e) {
    notify({ title: "herdr-nvim is not installed here", body: `${bin()}: ${e}`, level: "error" });
  }
  return null;
}

// The controller's decisions, in the profile while profiling (profile.js).
export function trace(what, data) {
  instant("sync", what, data, "decide");
}

// `herdr-nvim.profile` (cmd-alt-shift-p): profiling on, or off with its
// report (src/profile.rs) named in a toast.
export async function toggleProfile() {
  if (!profiling()) {
    const r = await exec(bin(), ["app-profile", "on"]).catch((e) => ({ code: 1, stderr: String(e) }));
    let file = null;
    try { file = r.code === 0 ? JSON.parse(r.stdout).file : null; } catch {}
    if (!file) return notify({ title: "herdr-nvim: profiling did not start", body: (r.stderr || r.stdout || "").trim().slice(-300), level: "error" });
    setShared("herdr-nvim.profile", file);
    return notify({ title: "herdr-nvim: profiling", body: "Use the app as usual. cmd-alt-shift-l marks a moment that felt slow; cmd-alt-shift-p stops and writes the report.", timeout: 8000 });
  }
  setShared("herdr-nvim.profile", "");
  await flush();
  const r = await exec(bin(), ["app-profile", "off", "--report"]).catch((e) => ({ code: 1, stderr: String(e) }));
  let report = null;
  try { report = r.code === 0 ? JSON.parse(r.stdout) : null; } catch {}
  if (!report) return notify({ title: "herdr-nvim: no profile report", body: (r.stderr || r.stdout || "").trim().slice(-300), level: "error" });
  notify({ title: "herdr-nvim: profile saved", body: `${report.headline}. ${report.summary}`, timeout: 20000 });
}

// `herdr-nvim.profile-mark` (cmd-alt-shift-l): "that felt slow".
export function markLag() {
  if (!profiling()) return notify({ title: "herdr-nvim", body: "Not profiling: cmd-alt-shift-p starts it.", level: "warning" });
  mark();
  notify({ title: "herdr-nvim", body: "Marked.", timeout: 1500 });
}
