// Where cmd-o's time goes, written to `mods/herdr-nvim/.cache/timing.log`
// (hidden, so writing there reloads no mod): the steps of a pick
// (`mark`) and, for 15 s after one starts, how often and how long this
// mod's copies render (`timed`). The newest 300 lines are kept.

import { appHome } from "herdr";
import * as fs from "fs/promises";

const FILE = "mods/herdr-nvim/.cache/timing.log";
const KEEP = 300;
const WATCH_MS = 15000;
let lines = null, writing = null, since = 0;
const renders = new Map(); // name -> {n, ms, max}

async function flush() {
  if (writing) return;
  writing = (async () => {
    const dir = `${appHome()}/mods/herdr-nvim/.cache`;
    await fs.mkdir(dir, { recursive: true }).catch(() => {});
    await fs.writeFile(`${appHome()}/${FILE}`, lines.slice(-KEEP).join("\n") + "\n").catch(() => {});
  })();
  await writing;
  writing = null;
}

function stamp() {
  const d = new Date();
  return `${d.toTimeString().slice(0, 8)}.${String(d.getMilliseconds()).padStart(3, "0")}`;
}

// One step of a pick; `start` begins a 15 s window of render counts.
export async function mark(label, start = false) {
  if (!lines) lines = ((await fs.readFile(`${appHome()}/${FILE}`, "utf8").catch(() => "")) || "").split("\n").filter(Boolean);
  if (start) { since = Date.now(); renders.clear(); }
  lines.push(`${stamp()} +${since ? Date.now() - since : 0}ms ${label}`);
  flush();
}

// Runs `body` (a render), counting it while a window is open; once the
// window ends, one line per copy: renders, total and longest ms.
export function timed(name, body) {
  if (!since) return body();
  const t0 = Date.now();
  try {
    return body();
  } finally {
    const ms = Date.now() - t0, r = renders.get(name) ?? { n: 0, ms: 0, max: 0 };
    renders.set(name, { n: r.n + 1, ms: r.ms + ms, max: Math.max(r.max, ms) });
    if (Date.now() - since > WATCH_MS) {
      const summary = [...renders].map(([k, v]) => `${k}: ${v.n} renders, ${v.ms} ms, longest ${v.max} ms`).join("; ");
      since = 0;
      mark(`renders in the 15 s after cmd-o: ${summary || "none"}`);
    }
  }
}
