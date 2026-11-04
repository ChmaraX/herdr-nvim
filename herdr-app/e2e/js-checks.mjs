// Checks of the mod's pure parts, outside the app: `node herdr-app/e2e/js-checks.mjs`
// (run.sh runs it first).
import assert from "node:assert/strict";
import { register } from "node:module";
import { wanted } from "../mods/herdr-nvim/sync-rule.js";
import { ACTIONS, helpGroups, recipeTable } from "../mods/herdr-nvim/tree-keys.js";

const opts = { native: true, local: true, kept: false, hidByTreeKey: false };
const tab = (files, nvim, other = false) => ({ tab: "a", files, other, nvim });

// A tab switch: the panel follows the tab's nvim.
assert.deepEqual(wanted(tab(false, true), { ...tab(true, true), tab: "b" }, opts), { panel: "show" });
assert.deepEqual(wanted(tab(true, false), null, opts), { panel: "hide" });
assert.deepEqual(wanted(tab(false, true), null, { ...opts, kept: true }), { panel: null });
// The panel opened or closed on the same tab: nvim follows it.
assert.deepEqual(wanted(tab(true, false), tab(false, false), opts), { kept: false, nvim: true });
// Files right after a tab switch, seen in one look: opens that tab's nvim.
assert.deepEqual(wanted(tab(true, false), { ...tab(false, true), tab: "b" }, opts), { kept: false, nvim: true });
assert.deepEqual(wanted(tab(false, true), tab(true, true), opts), { nvim: true });
assert.deepEqual(wanted(tab(false, true), tab(true, true), { ...opts, hidByTreeKey: true }), { kept: true });
assert.deepEqual(wanted(tab(false, true, true), tab(true, true), opts), { nvim: false });
// nvim appeared or went: the panel follows it; nvim gone clears the mark.
assert.deepEqual(wanted(tab(false, true), tab(false, false), opts), { panel: "show" });
assert.deepEqual(wanted(tab(true, false), tab(true, true), { ...opts, kept: true }), { kept: false, panel: "hide" });
assert.deepEqual(wanted(tab(true, true), tab(true, true), opts), {});
// tree = "nvim": the panel never stays.
assert.deepEqual(wanted(tab(true, true), null, { ...opts, native: false }), { panel: "hide" });

// The key table: every key one action, the list and the recipe from it.
assert.equal(ACTIONS.get("j"), "down");
assert.equal(ACTIONS.get("r"), "rename");
assert.ok(helpGroups("space e").find(([g]) => g === "Navigate")[1].some(([k]) => k === "space e"));
assert.ok(recipeTable().startsWith("| Key | Does |"));

// --- The mod's files that use the app, on a stand-in `herdr` -----------------
// (herdr-stub.mjs: each check says what the app reports).
register(`data:text/javascript,${encodeURIComponent(`
  export async function resolve(spec, ctx, next) {
    if (["herdr", "gpui-kit", "gpui-base"].includes(spec)) return { url: ${JSON.stringify(new URL("./herdr-stub.mjs", import.meta.url).href)}, shortCircuit: true };
    return next(spec, ctx);
  }`)}`);
const { app } = await import("./herdr-stub.mjs");
const { gitMarks, listFolder, markOf } = await import("../mods/herdr-nvim/tree.js");
const { TreeModel } = await import("../mods/herdr-nvim/tree-model.js");
const { hideTreeKeepNvim, panelState, takeKeepNvim } = await import("../mods/herdr-nvim/panel.js");
const { inside, join } = await import("../mods/herdr-nvim/ops.js");
const { Sync } = await import("../mods/herdr-nvim/sync.js");
const ok = (stdout) => ({ code: 0, stdout, stderr: "" });

// Git marks: an untracked folder comes once, as `dir/`; it and the files in
// it are new.
app.exec = async (cmd, args) =>
  args[0] === "rev-parse" ? ok("/r\n")
  : ok(" M src/a.js\0?? newdir/\0A  b.js\0 D gone.txt\0UU c.js\0");
const marks = await gitMarks("/r");
assert.deepEqual(marks, { "/r/src/a.js": "M", "/r/newdir": "A", "/r/b.js": "A", "/r/gone.txt": "D", "/r/c.js": "U" });
assert.equal(markOf(marks, "/r/newdir"), "A");
assert.equal(markOf(marks, "/r/newdir/deep/x.md"), "A");
assert.equal(markOf(marks, "/r/src/a.js"), "M");
assert.equal(markOf(marks, "/r/src/other.js"), null);

// A folder's listing (the script's records): folders first, then by name;
// the names git ignores, a folder's without its slash.
app.exec = async () => ok("f\0b.txt\0d\0src\0f\0a.txt\0d\0build\0x\0x\0build/\0a.txt\0");
assert.deepEqual(await listFolder("/r"), {
  entries: [{ name: "build", dir: true }, { name: "src", dir: true }, { name: "a.txt", dir: false }, { name: "b.txt", dir: false }],
  ignored: new Set(["build", "a.txt"]),
});
app.exec = async () => ({ code: 1, stdout: "", stderr: "" });
await assert.rejects(listFolder("/gone"), /Cannot list \/gone/);

// The tree model: a watch batch lists again only the folders on screen it
// touches; a build in an ignored folder lists nothing; the same listing
// again changes no rows.
const listings = { "/r": "d\0src\0d\0target\0f\0x.md\0x\0x\0target\0", "/r/src": "f\0a.js\0x\0x\0", "/r/target": "f\0out\0x\0x\0" };
let listed = [];
app.exec = async (cmd, args) => {
  if (cmd === "sh") { listed.push(args[3]); return ok(listings[args[3]]); }
  return args[0] === "rev-parse" ? ok("/r\n") : ok("");
};
const model = new TreeModel();
model.reset("/r");
await model.reload();
assert.deepEqual(model.rows().map((r) => r.abs), ["/r/src", "/r/x.md"]); // target/ ignored, hidden
await model.toggle("/r/src");
assert.deepEqual(model.rows().map((r) => r.abs), ["/r/src", "/r/src/a.js", "/r/x.md"]);
listed = [];
assert.deepEqual(await model.reloadChanged(["/r/target/out", "/r/.git/index"]), { marks: "none" });
assert.deepEqual(listed, []);
const version = model.version;
await model.reloadChanged(["/r/src/a.js"]);
assert.deepEqual(listed, ["/r/src"]);
assert.equal(model.version, version, "the same listing changed no rows");
listed = [];
await model.reloadChanged(["/r/elsewhere/deep/file"]); // in no folder on screen
assert.deepEqual(listed, []);
listed = [];
await model.reloadChanged(["/r"]); // the root itself: anything may have changed
assert.deepEqual(listed.sort(), ["/r", "/r/src"]);

// The Files panel: shown, hidden, or another child (Changes) on top.
const files = { id: "files", type: "block", content: { mod: "herdr-nvim" } };
const stack = (active, hidden = false) => ({
  id: "root", children: [{ id: "panel", hidden, children: [{ id: "body", type: "stack", active, children: [files, { id: "changes" }] }] }],
});
assert.deepEqual(panelState(stack("files")), { stack: "body", slot: "files", panel: "panel", files: true, other: false });
assert.deepEqual(panelState(stack("changes")), { stack: "body", slot: "files", panel: "panel", files: false, other: true });
assert.equal(panelState(stack("files", true)).files, false);
// Without a stack, the slot's own `hidden` says it (hidePanel hides it).
assert.equal(panelState({ id: "root", children: [{ ...files }] }).files, true);
assert.equal(panelState({ id: "root", children: [{ ...files, hidden: true }] }).files, false);
assert.equal(panelState({ id: "root", children: [] }), null);

// The tree key's hide: the next hide of that tab is its own (once), and
// nothing it stores is null (the stand-in's setShared refuses one).
hideTreeKeepNvim(panelState(stack("files")), "t", "t9");
assert.equal(takeKeepNvim("other"), false);
hideTreeKeepNvim(panelState(stack("files")), "t", "t9");
assert.equal(takeKeepNvim("t"), true);
assert.equal(takeKeepNvim("t"), false);

// Paths the tree sends.
assert.equal(join("/r/src", "../b/./c.txt"), "/r/b/c.txt");
assert.ok(inside("/r/src/a", "/r") && inside("/r", "/r") && !inside("/rx/a", "/r"));

// The controller's idea of a tab's nvim: its own open counts until the app
// lists the pane, and stops counting once the pane ended without ever being
// listed live (closed by something else meanwhile), or EXPECT_MS later.
app.spaces = [{ id: "s", tabs: [{ id: "t" }] }];
app.terminals = [];
app.exec = async () => ok(JSON.stringify({ terminal: "t9" }));
const sync = new Sync({ spawn() {}, notify() {} });
await sync.toggle({ space: "s", tab: "t", terminal: null, cwd: "/r" }, "open");
assert.equal(sync.nvimOf("t"), true, "just opened, not listed yet");
app.terminals = [{ address: "t9", state: "finished", labels: { "herdr-nvim.sidebar": "t" } }];
assert.equal(sync.nvimOf("t"), false, "ended before it was ever listed live");
app.terminals = [];
await sync.toggle({ space: "s", tab: "t", terminal: null, cwd: "/r" }, "open");
const now = Date.now;
Date.now = () => now() + 3500;
assert.equal(sync.nvimOf("t"), false, "never listed: the expectation runs out");
Date.now = now;
app.terminals = [{ address: "t9", state: "live", labels: { "herdr-nvim.sidebar": "t" } }];
assert.equal(sync.nvimOf("t"), true);

console.log("js checks: ok");
