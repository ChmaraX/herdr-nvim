# herdr-nvim

Neovim in every tab, with a native file tree beside it, opened and closed as
one. The tab bar's **Files** button (or `cmd-e`) shows a narrow native tree of
the tab's folder on the right and, directly left of it, the tab's own nvim: a
real nvim terminal in a full-height pane, backed by a persistent per-tab nvim
(buffers, LSPs and pending comments survive closing it). Picking a file in the
tree opens it in that nvim. `cmd-o` lists the files the tab's agent read and
edited, newest first. Code comments written in nvim go to the tab's agent with
file:line context.

![herdr-nvim](preview.png)

## What you see

- **Files panel** (right edge of the window, 300px by default): a 42px header
  with "Files" (semibold) and the folder name in `dim`, a ring spinner
  (`svg(...).spin(900)`, `muted`) while a file opens. Below, 28px rows:
  chevron, folder/file icon (the shipped `files` mod's SVGs, `muted`), name,
  and a git mark (`A` in `ansi2`, `D`/`U` in `ansi1`, `M` in `blocked`); the
  selected row on `selected`, hover on `hover`; background `background`, a
  1px `border` frame with 10px corners. A footer hint in `dim`: "Click or ⏎
  opens in this tab's nvim · ↑↓ ← →".
- **nvim pane**, directly left of the panel: it splits the whole tab, so it is
  as tall as the tab, and the tab's other panes keep their arrangement on its
  left (the tab area is shared half and half; drag the divider). Its header
  reads `nvim · <file>`. nvim's own file explorer is not opened.
- **Picker** (`cmd-o`): an 820×470 dialog titled "Open file from agent
  output" on a 30px `chrome` bar with a `border` line and a `dim` hint; under
  it herdr-nvim's terminal picker: the files the agent's session log shows it
  touched (newest first), then uncommitted git changes, with `+N -M` diff
  stats and ages; typing fuzzy-searches the whole repo.
- **Annotations** (inside nvim, default `<leader>a…` maps): `ac` comment the
  line or selection, `al` list, `as` paste all comments into the agent's
  input, `aS` send them (submitted), `ai` reference `path:line`.

### The tree's keys (LazyVim's Snacks explorer defaults)

The tree takes the keyboard from `pane.go` (the person's pane keys), from a
click, and from `cmd-shift-e`, which opens the pair if needed and selects the
file nvim shows. In nvim, LazyVim's `<leader>E`, `<leader>fe` and `<leader>fE`
do the same instead of opening nvim's own explorer.

**The tree key** (`[app] tree_key`, default `<leader>e`) toggles the tree
like LazyVim's explorer:

| Where | the tree key |
|---|---|
| nvim, tree hidden | shows the tree and puts the keyboard there, on nvim's file |
| nvim, tree showing | hides the tree; nvim stays open with the keyboard |
| the tree | hides the tree and gives nvim the keyboard (the same keys, with nvim's leader: `space` `e` in LazyVim) |

Which nvim gets it (with `tree = "native"`; read when nvim starts): a key
that opens a known explorer (Snacks, neo-tree, nvim-tree) is taken over; a
free key (plain nvim) is mapped; a key mapped to anything else, or one that
starts or ends a longer mapping, is left alone. `tree_key = ""` maps
nothing, and any lhs works (`"<C-n>"`, `"<leader>t"`). `cmd-e` and the Files
button still open and close both together.

| Key | Does |
|---|---|
| `j`/`k`, ↓/↑, `g`/`G` | move; first/last |
| `l`, ⏎, → | open the file in the tab's nvim (it takes the keyboard), or open/close the folder |
| `h`, ← | close the folder (the open one, else the row's) and stand on it |
| `/` | find: type to fuzzy-search every file under the root (fff); ⏎ opens, Esc returns |
| `a` | new file in the row's folder (a dialog); end with `/` for a folder; a new file opens in nvim |
| `r` | rename (a path in the root; nvim's buffers and language servers follow) |
| Tab | select the row (●) for `m`, `c`, `d`, `y` |
| `m` | move the selected rows into the row's folder (confirm); with none selected, rename |
| `c` | copy the selected rows into the row's folder (confirm); with none, copy to a new name |
| `d` | delete (confirm); to the trash when the person's Snacks says so; their buffers close |
| `y` / `p` | yank the paths into nvim's register / paste those files into the row's folder |
| `H` / `I` | show hidden / git-ignored files (start as the person's Snacks explorer has them) |
| Backspace / `.` | root up a folder / the row's folder becomes the root |
| `o` | open with the system's app |
| `Z` / `u` | close every folder / list again |
| `q`, Esc | back to the tab's nvim (Snacks closes its window; here the pair closes with cmd-e) |
| the tree key (`space` `e`) | hide the tree, back to nvim (nvim stays) |
| `~` / `?` | root: back to the tab's folder / every key, in a list |

## How it behaves

- **`tree` setting** (herdr-nvim's config, `[app] tree`, read at each use;
  `install.sh --tree native|nvim` writes it): `native` (default) is what this
  recipe describes; `nvim` makes the Files button and `cmd-e` toggle only the
  tab's nvim pane, with nvim's own explorer opened inside it (the Files panel
  shows for a moment and is put away), and leaves `<leader>e` as it was.
- **`tree_key` setting** (`[app] tree_key = "<leader>e"`, the default; any
  nvim lhs, or `""` for none): the key above. nvim reads it when it starts:
  after a change, close the pair and stop the tab's nvim
  (`bin/herdr-nvim daemons stop <space:tab>`), or open a new tab.
- **Files button = `cmd-e`**: in a tab without them, both open together (the
  panel, and the tab's nvim pane with the keyboard); pressing again closes
  both. Closing never stops the tab's nvim: reopening reattaches to the same
  process, same buffer and cursor. With nvim open and the tree hidden (by the
  tree key), they show the tree; pressed again, they close both.
- **Switching tabs or spaces**: the panel follows the tab you land on. A tab
  whose nvim pane is open shows the panel with it, unless the tree key hid
  the tree in that tab (it stays hidden until the panel is shown there again,
  or that nvim closes); a tab without one shows
  no panel. Looking at a tab never opens an nvim in it: nvim panes are per
  tab, the panel is per window. Switching the panel to Changes leaves the nvim
  pane open (Changes can open files in it); Files brings the tree back.
- **The pair stays a pair**: if the nvim pane opens some other way (the
  picker, `open`, another mod), the panel opens with it; if it goes (`:q`,
  closing the pane), the panel goes too.
- Click a file in the tree (or ↑↓ and ⏎ once the tree has the keyboard; ←/→
  fold and unfold): it opens in the tab's nvim pane, which takes the keyboard.
  Folders expand in place. The tree follows the focused tab's folder and
  refreshes when anything under it changes (a folder watch, no polling).
- **Picker**: reads the focused tab's agent (the focused terminal if it is an
  agent, else the agent in the same tab, else the first agent of the space).
  Its session log comes first (pi: `~/.pi/agent/sessions/--<cwd>--/*.jsonl`;
  Claude Code: `~/.claude/projects/<cwd>/*.jsonl`; the newest log written
  since the agent's process started), then uncommitted git changes; without a
  log, the paths on the agent's screen. ⏎ opens the file in the tab's nvim
  (at the line the agent named, for a path taken from its screen) and focuses
  it; Esc closes the dialog. With nothing to list, an 8-second warning toast
  says why.
- Annotations go to the tab's agent: the only agent of the space, or the only
  one in this tab; otherwise nvim asks which. "Send" pastes and presses Enter
  once the person has stopped typing for ~3 s (`hc send`); "paste" leaves the
  text in the agent's input. A working agent gets a warning in nvim; blocked,
  done and idle agents are treated alike.
- Closing a tab stops that tab's nvim (and its LSP servers) within a second;
  closing a space stops all of its tabs'. Unsaved buffers are discarded. An
  nvim whose tab no longer exists is stopped when the watcher starts (the
  first nvim pane after a restart).
- Cmd+click on a path in a terminal is Herdr's: it opens the system default
  app, not nvim (no link handlers yet, herdrdev/herdr-app#228). Use the tree
  or the picker; `bin/herdr-nvim open-link <path>` opens a path in the
  focused tab's nvim, taking the line from the agent's own `path:line`
  mention when the path came without one (for a system opener).
- Other mods open files in a tab's nvim with
  `<appHome>/mods/herdr-nvim/bin/herdr-nvim open --tab <tab id> <file>[:line[:col]]`
  (relative paths resolve against the tab's folder; prints
  `{"terminal": "tN"}`; the panel opens with it). It needs no herdr-nvim mod
  copy on screen.

## Built from

- **Layout tweak, no layout file shipped** (`src/app/layout_patch.rs`):
  - the node `files` (the `panel-body` stack's child that the Files button
    selects) gets `content: {mod: "herdr-nvim", props: {replaces: <old>,
    panel_width: 300, panel_width_was: 368}}`;
  - `floating` gets `{id: "herdr-nvim-sync", content: {mod: "herdr-nvim",
    props: {role: "sync"}}, edge: "bottom", align: "end"}`: an empty floating
    block (0×0, invisible) whose mod copy is always rendered. A hidden block's
    copy is not rendered at all, so the tree itself can't see the panel close;
  - the stack's parent (`panel`) gets `size: {px: 300}` (`install.sh
    --panel-width N`), only if its width is still the shipped 368 or the
    width a previous install set; a person's drag resizes it as before.
  Herdr's own layouts can't change content, so the install clones the shown
  shipped layout first (`layout.clone {name, as: "<name>-mods"}`, which
  switches to it); a personal layout is patched in place. Each write re-reads
  the file and replaces it whole only if unchanged meanwhile; other slots
  (`changes`) are left for other mods.
- **Mod `herdr-nvim`** (`place: none`; overlay: popup 1×1, frame none), roles
  by where it is shown:
  - `files` slot → `tree-view.js` + `tree.js`: the tree (POSIX `sh` listing
    like the shipped `files` mod's, 500 per folder; `git status --porcelain
    -z` marks; `watchFolder(root)`; `track_focus` + `on_key_down`). A file
    pick `exec`s `bin/herdr-nvim open <abs path> --ctx <focused()>`.
  - `role: "sync"` → `sync.js`, rendered on every live change (`layout()`,
    `focused()`, `terminals()`). It compares the shown tab, "Files shown"
    (`panel.js`: no hidden node above the slot and the stack showing it),
    "another child shown" and "tab has a live terminal labelled
    `herdr-nvim.sidebar=<tab>`" with its previous look and acts on what
    changed: tab → `layout.show`/`layout.hide` to match the tab's nvim pane;
    Files on the same tab → `exec bin/herdr-nvim toggle --state open|close`;
    nvim pane appeared or went → panel to match. One action at a time;
    `toggle` prints the terminal it opened or closed, and until
    `terminals()` shows that, the result is assumed.
  - overlay (`mod.open:herdr-nvim`, `cmd-e`): exactly the Files button
    (`layout.select {stack, child}` + `layout.show`, or `layout.hide`), then
    `closeOverlay`.
- **Mod `herdr-nvim-pick`**: `mod.open` → `exec bin/herdr-nvim pick-file
  --ctx …`, which prints `{"terminal": "tN"}` (the picker started by
  herdr-core, unlabeled, so no tab shows it) or `{"empty": "<why>"}`; then
  `closeOverlay` itself and `openOverlay({kind: "dialog", layout: column
  [{mod: herdr-nvim-pick, props: {bar: true}, 30px}, {terminal: tN}]})`. A
  dialog focuses its first terminal; it closes when the picker exits.
  `pick-file --list` prints the candidates instead (what the e2e checks).
- `lib.js` (identical in both folders: mods can't import each other) runs the
  wrapper `<appHome>/mods/herdr-nvim/bin/herdr-nvim` and posts errors with
  `notify`.
- **The wrapper** (written by the install) exports `HERDR_NVIM_HOST=app`,
  `HERDR_NVIM_SELF`, `HERDR_APP_BIN`, `HERDR_APP_HOME` and the plugin root,
  then execs the herdr-nvim binary (Rust, this repo) from
  `${XDG_DATA_HOME:-~/.local/share}/herdr-nvim/app`. It does:
  - `open`: makes sure the tab's daemon and nvim pane exist, then
    `nvim --remote` + `cursor(line, col)`, and `pane.focus {terminal}` once no
    dialog is left to take the keyboard back.
  - `toggle [--state open|close]`: finds the tab's pane by its label
    (`terminal.list`); closes it with `pane.close`, or `pane.split {direction:
    right, pane: <the tab's root node id from the core's herdr-app document>,
    argv: [wrapper, sidebar, --cwd, dir]}`.
  - `sidebar` (in the new pane): reads its tab from its `herdr.space`/
    `herdr.tab` labels, starts the tab's headless nvim daemon if needed
    (socket `…/herdr-nvim/app/<space12>_<tab12>.sock`, `titlestring` "nvim ·
    %t"), starts the watcher, and execs `nvim --remote-ui`.
  - `pick-file`: finds the agent's session log from `terminal.get` (its
    status report or program for the kind, its folder, its process's age from
    `ps -o etime=`; `PI_CODING_AGENT_DIR` / `CLAUDE_CONFIG_DIR` move the
    roots) and mines it with the TUI plugin's session parser.
  - `app-watch`: a named core terminal `herdr-nvim-watch` (no labels, so no
    tab shows it) that follows `hc events` and, on each change of the
    `herdr-app` document, stops daemons whose tab is gone; it ends when none
    are left.
  - `app-agents` / `app-send`: what the nvim plugin calls for the agent list
    (`terminal.list`, agents = terminals with a reported status) and for
    sending (`hc send` to submit; paste = a short framed `terminal.attach`
    that types the text, bracketed when the program asked for it).
- **The tree** (`tree-view.js` keys and drawing, `tree-model.js` folders,
  `dialog.js` its prompts as a dialog of this mod with props {dialog},
  `ops.js` its client): `mod.json` `"focusable": true`; `"focus"` from
  `nextCommand()` focuses it and selects nvim's current file. Every file
  operation is `exec bin/herdr-nvim fs '<request>'`, done by the tab's nvim
  (`lua/herdr-nvim/fs.lua`: Snacks' rename/trash/bufdelete when present, the
  same LSP `willRenameFiles`/`didRenameFiles` steps otherwise), refused
  outside the tab's folder unless the dialog asked first. nvim writes the
  file it shows to `$HERDR_NVIM_CURRENT_FILE` on BufEnter; the tree watches
  that folder. Paths are real paths (`/private/tmp`, not `/tmp`).
- **Keys** (`keys.json`, merged into `global` only where free):
  `cmd-e` → `mod.open:herdr-nvim`, `cmd-o` → `mod.open:herdr-nvim-pick`,
  `cmd-shift-e` → `herdr-nvim.reveal` (a command the controller takes).

## Watch out

- It runs programs (`exec` of the wrapper, `hc`, `nvim`, `git`, `ps`), reads
  files outside its folder (the repo, the agent's output and session logs)
  and edits `keys.json` and one layout. No network.
- The clone (`main-mods`) no longer follows Herdr's updates of `main`; run
  the install again after an app update (it upgrades in place) or uninstall
  and reinstall to start from the new `main`.
- Narrowing the panel in the layout file drops a width the person had
  dragged (a file edit beats a click); later drags stick.
- `terminals()` lags a command by a moment: a controller that reacts to it
  must not read its own just-closed pane as still open (see `sync.js`).
- The tab bar remembers the panel per space and restores it on a space
  switch; the controller then makes it follow the shown tab. A restore the
  person never saw (a space whose Files panel was open before the install)
  can close that tab's nvim pane once.
- A session log row has no line (as in the TUI: with a log, the screen's
  `path:line` mentions are not scraped). Two agents of the same kind in the
  same folder can't be told apart: the log written last wins.
- A terminal block in a layout is removed from the layout file when its
  terminal ends, so don't put the nvim terminal itself in the `files` slot.
- Overlay terminal blocks can't be focused by command (`pane.focus` fails);
  only a dialog/popup focuses its first terminal when it opens.
- Processes started from a terminal that keep using `hc` after it ends must
  drop `HC_SCOPE`/`HC_TERMINAL_ID`, or herdr-core ends them with it.
- The split always goes right (or down with `position = "bottom"` in
  `~/.config/herdr-nvim/config.toml`); `left`/`top` fall back to right/down.
- Spaces on other machines are not supported (the tree says so).
- The picker stays a terminal program on purpose: its repo-wide ranking (fff:
  typo tolerance, frecency) is the Rust core the TUI plugin shares.
- macOS was tested; the wrapper and install script are POSIX sh/bash.

- Keys go to the tree only after it has the keyboard: `pane.go` reaches it
  only once it is laid out (the controller waits for its first size).
- A dialog closing gives the tree the keyboard back (`"focus"` again).

## Done when

- `herdr-app check` passes for both mod folders and `herdr-app status` shows
  no errors; a second install reports "already uses herdr-nvim"; the clone
  differs from `main` only in the `files` content, the panel size and
  `floating`.
- The Files button commands in a tab open the panel (300px) and the tab's
  nvim pane, focused, with no nvim explorer window; opening `src/greet.js`
  from the tree shows it in that pane; Files again closes both and the daemon
  keeps running.
- `cmd-e` in a second tab does the same with its own daemon; switching to the
  first tab hides the panel and opens no pane there, switching back shows it;
  switching to another space and back does the same; `cmd-e` again closes
  both.
- `cmd-o` over an agent whose pi log read `package.json` and then edited
  `src/notes.md` lists `src/notes.md`, `package.json`, then the git change
  `src/greet.js`; `note` + ⏎ opens notes.md in the tab's nvim (focused) and
  the panel with it.
- `<leader>ac` + `<leader>aS` in nvim: the agent receives "Code review
  comments … src/notes.md:3 … Comment: …" submitted; `<leader>as` leaves it in
  the input unsubmitted.
- `bin/herdr-nvim open --tab <tab> src/notes.md:3:6` with both closed opens
  the pane on line 3, column 6, and the panel with it.
- `cmd-shift-e` selects nvim's file in the tree with the keyboard there; `r`
  renames it and nvim's buffer follows; `pane.go` right and a click
  (`herdr-nvim.click`, the tree's mouse-down handler) reach the tree too,
  with one pane highlighted; an nvim whose `<leader>e` opens no explorer
  keeps it.
- `<leader>e` in a plain nvim (mapped, as it was free): hides the tree with
  nvim kept and focused, shows it on nvim's file, and the same keys in the
  tree hide it; Files and `cmd-e` then show the tree, and again close both;
  back in that tab from another, the hidden tree stays hidden;
  `tree_key = ""` maps nothing.
- `a` makes files and `newdir/` folders; `m` with a Tab selection moves
  after a confirm; `d` deletes after a confirm and the buffer goes; `/needle`
  finds a file three folders down; `I` and `H` show `build/` and `.hidden`.
- `tree = nvim`: `cmd-e` and Files open and close only the nvim pane, with
  nvim's explorer inside.
- Closing the tab stops its nvim; closing the space stops the rest and the
  `herdr-nvim-watch` terminal ends.
- Uninstall: keys removed, the layout's Files is the shipped browser again,
  the clone gone if nothing else changed it.

`herdr-app/e2e/run.sh` does every check above in a private app and core.
