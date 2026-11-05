// Profiling: the mods' side of the trace `herdr-nvim app-profile on` starts
// (Chrome trace events, one JSON object a line, in the file herdr-nvim's own
// processes write to as well; src/profile.rs makes the report). The shared
// value `herdr-nvim.profile` names that file, "" while off; off, nothing
// here does anything.
//
// Recorded: the person's actions and how long until their result showed,
// the herdr-nvim commands and programs the mods wait on, slow renders and a
// count of all of them, and freezes of the app's main thread (a timer
// that comes late). Buffered, and appended about once a second.
//
// mods/herdr-nvim/profile.js and mods/herdr-nvim-header/profile.js are the
// same file: a mod can't import another mod's files (js-checks compares).

import { exec, shared } from "herdr";

const KEY = "herdr-nvim.profile";
const PID = 1; // the mods; one track (tid) per role, the actions on 0
// (`wait`: the herdr-nvim commands a mod waits on, from any role.)
const TID = { sync: 1, tree: 2, preview: 3, dialog: 4, overlay: 5, header: 6, wait: 7 };
const SLOW_RENDER_MS = 3;
const FLUSH_MS = 1000;
// A timer this much later than asked: the main thread was busy meanwhile.
const STALL_MS = 150;

let buffer = [], target = null, flushedAt = 0, renders = {};

const us = (ms) => Math.round(ms * 1000);

export function profiling() {
  const file = shared(KEY);
  return typeof file === "string" && file ? file : null;
}

function push(event) {
  const file = profiling();
  if (!file) return;
  target = file;
  buffer.push(JSON.stringify(event));
  if (buffer.length >= 200 || Date.now() - flushedAt > FLUSH_MS) flush();
}

// Appends what is buffered (and the render counts) to the profile, or to the
// one just stopped: what was recorded before the stop still goes there.
export function flush() {
  flushedAt = Date.now();
  target = profiling() ?? target;
  if (Object.keys(renders).length) {
    buffer.push(JSON.stringify({ ph: "C", name: "renders", ts: us(Date.now()), pid: PID, args: renders }));
    renders = {};
  }
  if (!target || !buffer.length) return Promise.resolve();
  const lines = buffer;
  buffer = [];
  return exec("sh", ["-c", 'printf "%s\\n" "$@" >> "$0"', target, ...lines]).catch(() => {});
}

// A step of `role` that takes time: call the returned function when it ends.
export function span(role, name, args) {
  if (!profiling()) return () => {};
  const start = Date.now();
  return (more) => push({ ph: "X", cat: "mod", name, ts: us(start), dur: us(Date.now() - start), pid: PID, tid: TID[role] ?? 0, args: { ...args, ...more } });
}

// `exec`, timed: `name` is what the trace calls it.
export async function timedExec(role, name, command, args, options) {
  const end = span(role, name);
  try {
    const r = await exec(command, args, options);
    end({ code: r?.code });
    return r;
  } catch (e) {
    end({ error: String(e) });
    throw e;
  }
}

export function instant(role, name, args, cat = "mod") {
  push({ ph: "i", s: "t", cat, name, ts: us(Date.now()), pid: PID, tid: TID[role] ?? 0, args });
}

// An action of the person's, from `start` (ms) until its result showed now:
// the time they waited.
export function action(name, start, args) {
  push({ ph: "X", cat: "action", name, ts: us(start), dur: us(Date.now() - start), pid: PID, tid: 0, args });
}

// "That was laggy": the person's mark (cmd-alt-shift-l).
export function mark() {
  push({ ph: "i", s: "g", cat: "mark", name: "lag mark", ts: us(Date.now()), pid: PID, tid: 0 });
}

// A render of `role`'s view that began at `start` (ms): counted, and kept
// as a span when slow.
export function rendered(role, start) {
  if (!profiling()) {
    if (buffer.length) flush();
    return;
  }
  renders[role] = (renders[role] ?? 0) + 1;
  const ms = Date.now() - start;
  if (ms >= SLOW_RENDER_MS) push({ ph: "X", cat: "render", name: `render ${role}`, ts: us(start), dur: us(ms), pid: PID, tid: TID[role] ?? 0 });
  else if (Date.now() - flushedAt > FLUSH_MS) flush();
}

// The freeze watch (the controller runs it while profiling): a timer every
// 100 ms on the app's main thread; one that comes STALL_MS late or more
// records the freeze. Returns its task (cancel it when profiling stops).
export function watchFreezes(cx) {
  let last = Date.now();
  return cx.timer.every(100, () => {
    const now = Date.now(), late = now - last - 100;
    if (late >= STALL_MS) push({ ph: "X", cat: "stall", name: "main thread busy", ts: us(last + 100), dur: us(late), pid: PID, tid: 0 });
    last = now;
    if (now - flushedAt > FLUSH_MS) flush();
  });
}
