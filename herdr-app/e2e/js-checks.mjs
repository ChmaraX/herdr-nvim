// Checks of the mod's pure parts, outside the app: `node herdr-app/e2e/js-checks.mjs`
// (run.sh runs it first).
import assert from "node:assert/strict";
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

console.log("js checks: ok");
