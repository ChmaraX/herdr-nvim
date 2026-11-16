// Checks of the mod's pure parts, outside the app: `node herdr-app/e2e/js-checks.mjs`
// (run.sh runs it first).
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
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
const { request } = await import("../mods/herdr-nvim/ops.js");
const path = await import("../mods/herdr-nvim/path.js");
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

// A folder's listing is herdr-nvim's (`app-list`, src/app/list.rs, which
// sorts it): its entries in its order, and the names git ignores.
const entry = (name, dir = false, ignored = false) => ({ name, dir, hidden: name.startsWith("."), ignored });
const listing = (...entries) => ok(JSON.stringify({ entries, truncated: false }));
const bin = "/nonexistent-app-home/mods/herdr-nvim/bin/herdr-nvim";
app.exec = async (cmd, args) => (cmd === bin && args[0] === "app-list" && args[1] === "/r"
  ? listing(entry("build", true, true), entry("src", true), entry(".env"), entry("a.txt", false, true)) : ok(""));
const r = await listFolder("/r");
assert.deepEqual(r.entries.map((e) => e.name), ["build", "src", ".env", "a.txt"]);
assert.deepEqual(r.ignored, new Set(["build", "a.txt"]));
app.exec = async () => ({ code: 1, stdout: "", stderr: "herdr-nvim: not a folder: /gone\n" });
await assert.rejects(listFolder("/gone"), /^Error: not a folder: \/gone$/);

// The tree model: a watch batch lists again only the folders on screen it
// touches; a build in an ignored folder lists nothing; the same listing
// again changes no rows.
const listings = {
  "/r": listing(entry("src", true), entry("target", true, true), entry(".env"), entry("x.md")),
  "/r/src": listing(entry("a.js")),
  "/r/target": listing(entry("out", false, true)),
};
let listed = [];
app.exec = async (cmd, args) => {
  if (cmd === bin && args[0] === "app-list") { listed.push(args[1]); return listings[args[1]]; }
  return args[0] === "rev-parse" ? ok("/r\n") : ok("");
};
const model = new TreeModel();
model.reset("/r");
await model.reload();
assert.deepEqual(model.rows().map((r) => r.abs), ["/r/src", "/r/x.md"]); // target/ ignored and .env hidden: not shown
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
// Hidden is what the listing says (a dotfile, or Windows' Hidden attribute);
// revealing a file in a hidden or ignored folder shows those.
assert.equal(await model.expandTo("/r/target/out"), "/r/target/out");
assert.ok(model.showIgnored && model.rows().some((r) => r.abs === "/r/target/out"));
model.showHidden = true;
assert.ok(model.rows().some((r) => r.abs === "/r/.env"));
model.showHidden = model.showIgnored = false;

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
assert.equal(path.join("/r/src", "../b/./c.txt"), "/r/b/c.txt");
assert.ok(path.inside("/r/src/a", "/r") && path.inside("/r", "/r") && !path.inside("/rx/a", "/r"));

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

// Mermaid in the preview: fenced ```mermaid blocks split out of the text
// (not one inside another fence, nor an unclosed one), each text part with
// the document's link definitions (not footnotes, not after an unclosed
// fence). A line starting with backticks that hold more backticks is code
// in a paragraph, not a fence (else it hides every diagram after it).
const { splitMermaid } = await import("../mods/herdr-nvim/mermaid.js");
assert.deepEqual(splitMermaid("# T\n\n```mermaid\ngraph TD\n  A-->B\n```\nafter"),
  [{ md: "# T\n" }, { mermaid: "graph TD\n  A-->B" }, { md: "after" }]);
assert.deepEqual(splitMermaid("~~~ Mermaid\nx\n~~~"), [{ mermaid: "x" }]);
assert.deepEqual(splitMermaid("````md\n```mermaid\nx\n```\n````"), [{ md: "````md\n```mermaid\nx\n```\n````" }]);
assert.deepEqual(splitMermaid("a\n```mermaid\nx"), [{ md: "a\n```mermaid\nx" }]);
assert.deepEqual(splitMermaid("```js\nx\n```\n[a]: http://a"), [{ md: "```js\nx\n```\n[a]: http://a" }]);
assert.deepEqual(splitMermaid("See [a][] and [b].[^n]\n```mermaid\nx\n```\n[b] again.\n\n```txt\n[c]: not one\n```\n[a]: http://a\n[b]: http://b \"B\"\n[^n]: note"),
  [{ md: "See [a][] and [b].[^n]\n\n[a]: http://a\n[b]: http://b \"B\"" }, { mermaid: "x" },
   { md: "[b] again.\n\n```txt\n[c]: not one\n```\n[a]: http://a\n[b]: http://b \"B\"\n[^n]: note\n\n[a]: http://a\n[b]: http://b \"B\"" }]);
assert.deepEqual(splitMermaid("[a]: http://a\n```mermaid\nx\n```\n```js\nunclosed"),
  [{ md: "[a]: http://a\n\n[a]: http://a" }, { mermaid: "x" }, { md: "```js\nunclosed" }]);
assert.deepEqual(splitMermaid("```` ```mermaid ```` blocks are drawn.\n\n```mermaid\nx\n```"),
  [{ md: "```` ```mermaid ```` blocks are drawn.\n" }, { mermaid: "x" }]);
assert.deepEqual(splitMermaid("```mermaid title=\"Plan\"\nx\n```"), [{ mermaid: "x" }]);

// --- Windows, and the programs the mods run ---------------------------------

// Both mods run herdr-nvim's launcher, named one way: the header mod carries
// the same path.js (it can't import this mod's files).
const modsDir = new URL("../mods/", import.meta.url);
assert.equal(readFileSync(new URL("herdr-nvim-header/path.js", modsDir), "utf8"), readFileSync(new URL("herdr-nvim/path.js", modsDir), "utf8"),
  "herdr-nvim-header/path.js is no longer a copy of herdr-nvim/path.js: copy it again");
assert.equal(path.bin(), "/nonexistent-app-home/mods/herdr-nvim/bin/herdr-nvim");
app.platform = "windows";
app.home = "C:\\Users\\Me\\.config\\herdr-app";
assert.equal(path.bin(), "C:\\Users\\Me\\.config\\herdr-app\\mods\\herdr-nvim\\bin\\herdr-nvim.exe");

// The mods start no program but herdr-nvim, git and the system's opener (no
// sh, cat, cp or PowerShell: they differ by OS, or Windows lacks them).
const started = new Set();
for (const mod of ["herdr-nvim", "herdr-nvim-header"]) {
  for (const file of readdirSync(new URL(`${mod}/`, modsDir)).filter((f) => f.endsWith(".js"))) {
    const source = readFileSync(new URL(`${mod}/${file}`, modsDir), "utf8");
    for (const [, program] of source.matchAll(/(?<![.\w])exec\(\s*([^,]+?)\s*,/g)) started.add(program.trim());
  }
}
assert.deepEqual([...started].sort(), ['"git"', "bin()", "opener"]);
const { systemOpen } = await import("../mods/herdr-nvim/ops.js");
const opened = [];
app.exec = async (cmd, args) => { opened.push([cmd, ...args]); return { code: 1, stdout: "", stderr: "" }; };
await systemOpen("C:/r/a b.txt");
app.platform = "linux";
await systemOpen("/r/a b.txt");
app.platform = "macos";
await systemOpen("/r/a b.txt");
assert.deepEqual(opened, [["explorer.exe", "C:\\r\\a b.txt"], ["xdg-open", "/r/a b.txt"], ["open", "/r/a b.txt"]]);

// Windows paths: every form the app, git and nvim give is one path, compared
// case aside; a drive's root is a root. Never case-folded on macOS or Linux.
app.platform = "windows";
for (const [given, canonical] of [
  ["\\\\?\\C:\\Users\\Me\\repo\\", "C:/Users/Me/repo"], ["c:\\x\\y", "C:/x/y"], ["C:/x\\y/", "C:/x/y"],
  ["C:", "C:/"], ["d:\\", "D:/"], ["\\\\?\\UNC\\host\\share\\x", "//host/share/x"], ["\\\\host\\share", "//host/share/"],
]) assert.equal(path.canonical(given), canonical, given);
assert.equal(path.dirname("C:/x"), "C:/");
assert.equal(path.dirname("C:/"), "C:/");
assert.equal(path.dirname("//host/share/x"), "//host/share/");
assert.equal(path.basename("C:/x/y.md"), "y.md");
assert.equal(path.join("C:/r/src", "..\\b\\.\\c.txt"), "C:/r/b/c.txt");
assert.equal(path.join("C:/", "../x"), "C:/x");
assert.ok(path.same("C:/Repo/x", "c:/repo/X") && path.inside("C:/REPO/src/a", "C:/repo") && !path.inside("C:/repox/a", "C:/repo"));
assert.equal(path.relative("C:/Repo/src/a.js", "C:/repo"), "src/a.js");
assert.equal(path.relative("D:/x", "D:/"), "x");
assert.equal(path.native("C:/r/a"), "C:\\r\\a");
assert.equal(path.tilde("C:/Users/Me/.config/herdr-nvim/config.toml", "c:/users/me"), "~/.config/herdr-nvim/config.toml");
assert.deepEqual(request.create("C:/r", "new\\"), { op: "create", path: "C:/r/new", folder: true });
app.platform = "macos";
assert.ok(!path.same("/r/X", "/r/x") && !path.inside("/R/a", "/r"));
assert.equal(path.canonical("C:\\x"), "C:\\x", "on macOS \\ is a character of a name");
assert.deepEqual(request.create("/r", "a\\"), { op: "create", path: "/r/a\\", folder: false });
assert.deepEqual(request.create("/r", "new/"), { op: "create", path: "/r/new", folder: true });
assert.equal(path.tilde("/Users/me/.config/herdr-nvim/config.toml", "/Users/me"), "~/.config/herdr-nvim/config.toml");

// The tree on Windows: git names the repo in its own case, nvim a file in
// what was typed; the marks, the reveal and the rows still meet.
app.platform = "windows";
const winListings = {
  "C:/r": listing(entry("Src", true), entry("x.md")),
  "C:/r/Src": listing(entry("A.js")),
};
app.exec = async (cmd, args, opts) => {
  if (cmd === path.bin() && args[0] === "app-list") return winListings[args[1]];
  if (args[0] === "rev-parse") return ok("c:/R\n");
  return ok(" M Src/A.js\0?? x.md\0");
};
const winModel = new TreeModel();
winModel.reset("C:/r");
await winModel.reload();
assert.equal(markOf(winModel.marks, "C:/r/Src/A.js"), "M");
assert.equal(markOf(winModel.marks, "C:/r/x.md"), "A");
assert.equal(await winModel.expandTo("c:/R/src/a.JS"), "C:/r/Src/A.js");
assert.deepEqual(winModel.rows().map((r) => r.abs), ["C:/r/Src", "C:/r/Src/A.js", "C:/r/x.md"]);

app.home = "/nonexistent-app-home";

console.log("js checks: ok");
