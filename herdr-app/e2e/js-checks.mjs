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
assert.deepEqual(wanted(tab(true, false), tab(false, false), { ...opts, native: false }), { panel: "hide" });

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
const { panelState } = await import("../mods/herdr-nvim/panel.js");
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
app.panes = [{ pane: "p9", terminal: "t9", tab: "t" }];
assert.equal(sync.nvimOf("t"), true);
// Parked (Files closed it: running, in no pane): not open.
app.panes = [];
assert.equal(sync.nvimOf("t"), false, "a parked nvim counts as open");

// The controller's own show, not on screen yet: the next look waits for it
// (read as is, it is the person closing Files, and nvim would close).
app.focused = { space: "s", tab: "t", terminal: "t1", cwd: "/r" };
app.terminals = [{ address: "t9", state: "live", labels: { "herdr-nvim.sidebar": "t" } }];
app.panes = [{ pane: "p9", terminal: "t9", tab: "t" }];
app.layout = stack("files", true); // the panel hidden, nvim on screen
app.ran = [];
// A real wait, on a timer that doesn't keep the checks running (the
// controller's config loop sleeps between reads for good).
const cx = { spawn: (fn) => { fn(cx); }, notify() {}, sleep: (ms) => new Promise((done) => setTimeout(done, ms).unref()) };
const controller = new Sync(cx);
controller.look(cx); // nvim without its tree: show the panel
assert.deepEqual(app.ran.map(([name]) => name), ["layout.select", "layout.show"]);
controller.look(cx); // the app hasn't applied it yet
controller.look(cx);
await new Promise((done) => setTimeout(done, 20)); // whatever those looks started
app.layout = stack("files"); // now it has
controller.look(cx);
await new Promise((done) => setTimeout(done, 20));
assert.deepEqual(app.ran.map(([name]) => name), ["layout.select", "layout.show"], "nothing closed or opened meanwhile");

// The tree key (a command from nvim or the tree): the tree hides, nvim stays
// and gets the keyboard, and the tab remembers it; the next hide by Files
// closes nvim as usual.
app.layout = stack("files");
app.ran = [];
const keyed = new Sync(cx);
keyed.look(cx); // the pair on screen: nothing to do
await keyed.toggleTree(cx);
assert.deepEqual(app.ran.map(([name]) => name), ["layout.hide", "pane.focus"]);
app.layout = stack("files", true);
keyed.look(cx);
await new Promise((done) => setTimeout(done, 20));
assert.ok(!app.ran.some(([name]) => name === "pane.close"), "the tree key closed nvim");
assert.equal(keyed.tabState("t").kept, true);
app.layout = stack("files");
keyed.look(cx); // Files again: shows the tree, nvim already there
await new Promise((done) => setTimeout(done, 20));
app.layout = stack("files", true);
keyed.look(cx); // Files off: nvim closes with it
await new Promise((done) => setTimeout(done, 20));
assert.ok(app.ran.some(([name, args]) => name === "pane.close" && args.pane === "p9"), "Files off kept nvim");

// Preview: herdr-nvim swaps the preview in (app-preview); until the lists
// show it, the tab still has its nvim (else the tree would hide: a flicker).
app.exec = async (cmd, args) => ok(args[0] === "app-preview" ? JSON.stringify({ preview: "pv", closed: "t9" }) : "{}");
const swapper = new Sync(cx);
await swapper.startSwap("t", "/r/x.md", cx);
app.terminals = []; app.panes = []; // nvim's pane gone, the preview not listed yet
assert.equal(swapper.nvimOf("t"), true, "mid-swap, the tab lost its nvim");
app.panes = [{ pane: "pv", terminal: null, tab: "t", mod: "herdr-nvim" }];
assert.equal(swapper.nvimOf("t"), true);
assert.equal(swapper.tabState("t").expect, null, "the swap's expectation outlived what it waited for");

// No Neovim to start a tab's nvim with (herdr-nvim exits 3): one card for
// requests failing together, none for the tree's quiet lookup, no toast.
const { nvim } = await import("../mods/herdr-nvim/lib.js");
const { welcomeRows } = await import("../mods/herdr-nvim/card.js");
app.focused = { space: "s", tab: "t", terminal: "t1", cwd: "/r" };
app.exec = async () => ({ code: 3, stdout: JSON.stringify({ nvim_missing: { bin: "nvim", found: null, need: "0.10" } }), stderr: "" });
app.overlays = []; app.shared["herdr-nvim.card-at"] = 0;
const { openCard } = await import("../mods/herdr-nvim/card.js");
openCard({ kind: "welcome", rows: [], config: "" }); // just before: it holds nothing back
app.overlays = [];
assert.equal(await nvim("fs", ["{}"], null, { quiet: true }), null);
assert.equal(app.overlays.length, 0, "the tree's quiet lookup opened the card");
assert.equal(await nvim("toggle", ["--state", "open"]), null);
assert.equal(await nvim("open", ["/r/x.js"]), null);
assert.deepEqual(app.overlays.map((o) => o.props.card.kind), ["nvim"], "not exactly one card for two failing requests");
assert.equal(app.overlays[0].props.card.bin, "nvim");

// The welcome lists the keys keys.json binds (any section), not ones the
// install couldn't take; nvim's own explorer has no tree rows.
const keys = { global: { "cmd-e": "mod.open:herdr-nvim", "cmd-shift-e": "herdr-nvim.reveal" }, terminal: { "cmd-k o": "herdr-nvim.pick" } };
const rows = welcomeRows(keys, { tree: "native", tree_key: "<leader>e" });
assert.deepEqual(rows.map(([k]) => k), ["cmd-e", "cmd-k o", "cmd-shift-e", "<leader>e", "?"]);
assert.deepEqual(welcomeRows({ global: { "cmd-e": "mod.open:herdr-nvim" } }, { tree: "nvim", tree_key: "<leader>e" }),
  [["cmd-e", "open or close nvim"]]);

// Remote labels are local to their core; matching a tab must include its machine.
const { nvimPanes, parkedNvim } = await import("../mods/herdr-nvim/panel.js");
const { place, isRemoteTab } = await import("../mods/herdr-nvim/lib.js");
app.spaces = [{ id: "box/s1", machine: "box", tabs: [{ id: "box/t1" }] }];
app.focused = { space: "box/s1", tab: "box/t1", terminal: "box/t7", cwd: "/remote" };
app.terminals = [
  { address: "box/t9", state: "live", labels: { "herdr-nvim.sidebar": "t1" } },
  { address: "other/t9", state: "live", labels: { "herdr-nvim.sidebar": "t1" } },
];
app.panes = [{ pane: "box/p9", terminal: "box/t9", tab: "box/t1" }];
assert.deepEqual(nvimPanes("box/t1"), ["box/t9"]);
assert.deepEqual(nvimPanes("other/t1"), []);
assert.equal(parkedNvim("other/t1"), "other/t9");
assert.deepEqual(place(), { space: "box/s1", tab: "box/t1", terminal: "box/t7", cwd: "/remote" });
assert.ok(isRemoteTab("box/t1") && !isRemoteTab("t1"));
app.exec = async (_cmd, args) => {
  assert.deepEqual(args.slice(0, 3), ["toggle", "--state", "close"]);
  assert.equal(JSON.parse(args.at(-1)).tab, "box/t1");
  return ok(JSON.stringify({ closed: "box/t9" }));
};
app.ran = [];
const remoteSync = new Sync({ spawn() {}, notify() {} });
await remoteSync.toggle(place(), "close");
assert.deepEqual(app.ran, [], "remote toggles must use the machine-aware Rust path");
await remoteSync.startSwap("box/t1", "/remote/image.png", { notify() {} });
assert.deepEqual(app.ran, [], "unsupported remote previews must not create local preview panes");

console.log("js checks: ok");
