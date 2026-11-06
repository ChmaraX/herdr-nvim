// The tree's keys, in one table: tree-view.js runs them (each keystroke
// names an action, a method of Tree.actions), keys-help.js lists them (`?`),
// and RECIPE.md's key table is this table (`node tree-keys.js` prints it;
// the e2e checks the recipe still matches).
//
// An entry: its help group, how the list shows its keys (`label`) and what
// they do (`what`, short; `recipe`, the recipe's longer line), and `on`:
// keystroke (as GPUI names it, or the typed character) → action.

const KEYS = [
  { group: "Navigate", label: "j k  ↓ ↑", md: "`j`/`k`, ↓/↑", what: "move", recipe: "move",
    on: { j: "down", down: "down", k: "up", up: "up" } },
  { group: "Navigate", label: "g G", md: "`g`/`G`", what: "first / last", recipe: "first / last row",
    on: { g: "first", G: "last" } },
  { group: "Navigate", label: "l  ⏎  →", md: "`l`, ⏎, →", what: "open in nvim, or open a folder",
    recipe: "open the file in the tab's nvim (it takes the keyboard), or open/close the folder",
    on: { l: "open", enter: "open", right: "open" } },
  { group: "Navigate", label: "h  ←", md: "`h`, ←", what: "close the folder",
    recipe: "close the folder (the open one, else the row's) and stand on it", on: { h: "close", left: "close" } },
  { group: "Navigate", label: "/", md: "`/`", what: "find a file",
    recipe: "find: type to fuzzy-search every file under the root (fff); ⏎ opens, Esc returns", on: { "/": "find" } },
  { group: "Navigate", label: "⌫", md: "Backspace", what: "root: up a folder", recipe: "root up a folder",
    on: { backspace: "rootUp" } },
  { group: "Navigate", label: ".", md: "`.`", what: "root: this folder", recipe: "the row's folder becomes the root",
    on: { ".": "rootHere" } },
  { group: "Navigate", label: "~", md: "`~`", what: "root: back to the tab's folder", recipe: "root: back to the tab's folder",
    on: { "~": "rootHome" } },
  { group: "Navigate", label: "q  Esc", md: "`q`, Esc", what: "back to nvim",
    recipe: "back to the tab's nvim (Snacks closes its window; here the pair closes with cmd-e)",
    on: { q: "leave", escape: "leave" } },
  { group: "Files", label: "a", md: "`a`", what: "new file (end with / for a folder)",
    recipe: "new file in the row's folder (a dialog); end with `/` for a folder; a new file opens in nvim", on: { a: "add" } },
  { group: "Files", label: "r", md: "`r`", what: "rename",
    recipe: "rename (a path in the root; nvim's buffers, unsaved edits and language servers follow)", on: { r: "rename" } },
  { group: "Files", label: "Tab", md: "Tab", what: "select", recipe: "select the row (●) for `m`, `c`, `d`, `y`",
    on: { tab: "pick" } },
  { group: "Files", label: "m", md: "`m`", what: "move selected here (none: rename)",
    recipe: "move the selected rows into the row's folder (confirm); with none selected, rename", on: { m: "move" } },
  { group: "Files", label: "c", md: "`c`", what: "copy selected here (none: copy to a name)",
    recipe: "copy the selected rows into the row's folder (confirm); with none, copy to a new name", on: { c: "copy" } },
  { group: "Files", label: "d", md: "`d`", what: "delete",
    recipe: "delete (confirm); to the trash when the person's Snacks says so; their buffers close", on: { d: "delete" } },
  { group: "Files", label: "y p", md: "`y` / `p`", what: "yank paths / paste them here",
    recipe: "yank the paths into nvim's register / paste those files into the row's folder", on: { y: "yank", p: "paste" } },
  { group: "Files", label: "o", md: "`o`", what: "open with the system app", recipe: "open with the system's app",
    on: { o: "systemOpen" } },
  { group: "View", label: "H", md: "`H`", what: "hidden files", recipe: "show hidden files (starts as the person's Snacks explorer has it)",
    on: { H: "hidden" } },
  { group: "View", label: "I", md: "`I`", what: "git-ignored files", recipe: "show git-ignored files (starts as the person's Snacks explorer has it)",
    on: { I: "ignored" } },
  { group: "View", label: "Z", md: "`Z`", what: "close every folder", recipe: "close every folder", on: { Z: "collapse" } },
  { group: "View", label: "u", md: "`u`", what: "list again", recipe: "list again", on: { u: "reload" } },
  { group: "View", label: "?", md: "`?`", what: "this list", recipe: "every key, in a list", on: { "?": "help" } },
];

// The tree key's row (herdr-nvim's `[app] tree_key`, read from nvim).
const TREE_KEY_ROW = { group: "Navigate", what: "hide the tree, back to nvim (in nvim: show it)",
  md: "the tree key (`space` `e`)", recipe: "hide the tree, back to nvim (nvim stays)" };

// keystroke → action.
export const ACTIONS = new Map(KEYS.flatMap((entry) => Object.entries(entry.on)));

// The help list's groups, in order: [title, [[label, what], ...]], with the
// tree key's row when it has one.
export function helpGroups(treeKey) {
  const groups = [];
  for (const entry of KEYS) {
    let group = groups.find(([title]) => title === entry.group);
    if (!group) groups.push((group = [entry.group, []]));
    group[1].push([entry.label, entry.what]);
  }
  if (treeKey) groups.find(([title]) => title === TREE_KEY_ROW.group)[1].push([treeKey, TREE_KEY_ROW.what]);
  return groups;
}

// RECIPE.md's table.
export function recipeTable() {
  const rows = [...KEYS, TREE_KEY_ROW].map((entry) => `| ${entry.md} | ${entry.recipe} |`);
  return ["| Key | Does |", "|---|---|", ...rows].join("\n");
}

// `node tree-keys.js`: print the recipe's table.
if (typeof process !== "undefined" && process.argv?.[1]?.endsWith("tree-keys.js")) console.log(recipeTable());
