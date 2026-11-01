# herdr-nvim

Neovim in every tab, with a native file tree beside it. The tab bar's
**Files** button shows a native tree of the tab's folder (git marks
included); picking a file opens it in the tab's own nvim: a real nvim
terminal in a full-height pane on the right of the tab, backed by a
persistent per-tab nvim (buffers, LSPs, pending comments survive closing the
pane). A picker opens the files the agent mentioned or changed, and code
comments written in nvim go to the tab's agent with file:line context.

![herdr-nvim](preview.png)

## What you see

- **Files panel** (the tab bar's own Files button, top right; the panel is
  368px wide on the right of the window): a 42px header with "Files"
  (semibold), the folder name in `dim`, a spinner while a file opens, and an
  `nvim` button (theme `surface`, `border`; reads "Close nvim" on `selected`
  while this tab's nvim pane is open). Below, 28px rows: chevron, folder/file
  icon (the shipped `files` mod's SVGs, `muted`), name, and a git mark (`A`
  in `ansi2`, `D`/`U` in `ansi1`, `M` in `blocked`); the selected row on
  `selected`, hover on `hover`. A footer hint in `dim`:
  "Click or ⏎ opens in this tab's nvim · ↑↓ ← →".
- **nvim pane**: splits the whole tab, so it is as tall as the tab and every
  other pane keeps its arrangement on the left (half the width; drag the
  divider). Its header reads `nvim · <file>`.
- **Picker** (`cmd-o`): an 820×470 dialog titled "Open file from agent
  output" on a 30px `chrome` bar with a `border` line and a `dim` hint; under
  it herdr-nvim's terminal picker: newest agent-touched files first with
  `+N -M` diff stats and ages; typing fuzzy-searches the whole repo.
- **Annotations** (inside nvim, default `<leader>a…` maps): `ac` comment the
  line or selection, `al` list, `as` paste all comments into the agent's
  input, `aS` send them (submitted), `ai` reference `path:line`.

## How it behaves

- Click a file in the tree (or select it with ↑↓ and press ⏎; ←/→ fold and
  unfold folders once the tree has the keyboard): the tab's nvim pane opens if
  it isn't open, shows the file, and takes the keyboard. Folders expand in
  place. The tree follows the focused tab's folder and refreshes when anything
  under it changes (a folder watch, no polling).
- `nvim` in the header or `cmd-e`: open or close this tab's nvim pane. Closing
  it does not stop that tab's nvim: reopening reattaches to the same process,
  same buffer and cursor.
- Switching tabs shows each tab's own nvim pane (or none): they are panes of
  their tab. Two tabs never share an nvim. The panel itself is per window and
  per space, as the tab bar remembers it.
- The picker reads the focused tab's agent (the focused terminal if it is an
  agent, else the agent in the same tab, else the first agent of the space):
  its screen and history plus the repo's git changes. ⏎ opens the file in the
  tab's nvim at the line the agent named, and focuses it; Esc closes it. No
  agent, or no paths: a toast says so. Errors from any action are toasts.
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
  `{"terminal": "tN"}`). It needs no herdr-nvim mod copy on screen.

## Built from

- **Layout tweak, no layout file shipped**: the shown layout's node `files`
  (the child of the `panel-body` stack that the Files button selects) gets
  `content: {mod: "herdr-nvim", props: {replaces: <old content>}}`. Herdr's
  own layouts can't change content, so the install clones the shown shipped
  layout first (`layout.clone {name, as: "<name>-mods"}`, which switches to
  it) and edits only that node of the clone's file, re-reading the file and
  replacing it whole only if unchanged meanwhile. A personal layout is edited
  in place. Other slots (`changes`) are left for other mods.
- **Mod `herdr-nvim`** (`mods/herdr-nvim`, `place: none`; overlay: popup
  1×1, frame none). In a layout block it is the tree: `main.js` (view, keys
  via `track_focus` + `on_key_down`), `tree.js` (a POSIX `sh` listing like the
  shipped `files` mod's, folders first, 500 per folder; `git status
  --porcelain -z` marks), `watchFolder(root)` to refresh. A file pick `exec`s
  `bin/herdr-nvim open <abs path> --ctx <focused()>`; the header button
  `bin/herdr-nvim toggle --ctx …`; the button's state is
  `terminals().some(t => t.labels["herdr-nvim.sidebar"] === tab)`. Opened
  with `mod.open:herdr-nvim` (a key) it toggles once and `closeOverlay`s.
- **Mod `herdr-nvim-pick`**: `mod.open` → `exec bin/herdr-nvim pick-file
  --ctx …`, which prints `{"terminal": "tN"}` (the picker started by
  herdr-core, unlabeled, so no tab shows it) or `{"empty": "<why>"}`; then
  `closeOverlay` itself and `openOverlay({kind: "dialog", layout: column
  [{mod: herdr-nvim-pick, props: {bar: true}, 30px}, {terminal: tN}]})`. A
  dialog focuses its first terminal; the dialog closes when the picker exits.
- `lib.js` (identical in both folders: mods can't import each other) runs the
  wrapper `<appHome>/mods/herdr-nvim/bin/herdr-nvim` and posts errors with
  `notify`.
- **The wrapper** (written by the install) exports `HERDR_NVIM_HOST=app`,
  `HERDR_NVIM_SELF`, `HERDR_APP_BIN`, `HERDR_APP_HOME` and the plugin root,
  then execs the herdr-nvim binary (Rust, this repo) from
  `${XDG_DATA_HOME:-~/.local/share}/herdr-nvim/app`. It does:
  - `open`: makes sure the tab's daemon and nvim pane exist, then
    `nvim --remote` + `cursor(line, col)`, and `pane.focus {terminal}`.
  - `toggle`: finds the tab's sidebar by the label `herdr-nvim.sidebar=<tab>`
    (`terminal.list`); closes it with `pane.close`, or `pane.split {direction:
    right, pane: <the tab's root node id from the core's herdr-app document>,
    argv: [wrapper, sidebar, --cwd, dir]}`.
  - `sidebar` (in the new pane): reads its tab from its `herdr.space`/
    `herdr.tab` labels, starts the tab's headless nvim daemon if needed
    (socket `…/herdr-nvim/app/<space12>_<tab12>.sock`, `titlestring` "nvim ·
    %t"), starts the watcher, and execs `nvim --remote-ui`.
  - `app-watch`: a named core terminal `herdr-nvim-watch` (no labels, so no
    tab shows it) that follows `hc events` and, on each change of the
    `herdr-app` document, stops daemons whose tab is gone; it ends when none
    are left.
  - `app-agents` / `app-send`: what the nvim plugin calls for the agent list
    (`terminal.list`, agents = terminals with a reported status) and for
    sending (`hc send` to submit; paste = a short framed `terminal.attach`
    that types the text, bracketed when the program asked for it).
- **Keys** (`keys.json`, merged into `global` only where free):
  `cmd-e` → `mod.open:herdr-nvim`, `cmd-o` → `mod.open:herdr-nvim-pick`.

## Watch out

- It runs programs (`exec` of the wrapper, `hc`, `nvim`, `git`), reads files
  outside its folder (the repo, the agent's output) and edits `keys.json` and
  one layout. No network.
- The clone (`main-mods`) no longer follows Herdr's updates of `main`; run
  the install again after an app update (it is idempotent) or uninstall and
  reinstall to start from the new `main`.
- A terminal block in a layout is removed from the layout file when its
  terminal ends, so don't put the nvim terminal itself in the `files` slot.
- Overlay terminal blocks can't be focused by command (`pane.focus` fails);
  only a dialog/popup focuses its first terminal when it opens.
- Processes started from a terminal that keep using `hc` after it ends must
  drop `HC_SCOPE`/`HC_TERMINAL_ID`, or herdr-core ends them with it.
- The split always goes right (or down with `position = "bottom"` in
  `~/.config/herdr-nvim/config.toml`); `left`/`top` fall back to right/down.
- Spaces on other machines are not supported (the sidebar would run there).
- The tree needs a click before it takes the keyboard (mods can't be focused
  by command). With the panel and the nvim pane open, a narrow window gets
  three columns; drag the panel narrower or close it.
- The picker stays a terminal program on purpose: its repo-wide ranking (fff:
  typo tolerance, frecency) is the Rust core the TUI plugin shares.
- macOS was tested; the wrapper and install script are POSIX sh/bash.

## Done when

- `herdr-app check` passes for both mod folders and `herdr-app status` shows
  no errors; a second install reports "already uses herdr-nvim".
- Files shows the native tree of the tab's folder with git marks; opening
  `src/greet.js` from it opens a right-hand nvim pane on greet.js with focus in
  it; the toggle closes the pane and reopening finds the same daemon pid and
  buffer.
- `bin/herdr-nvim open --tab <tab> src/notes.md:3:6` with the panel hidden
  leaves that tab's nvim at line 3, column 6.
- `cmd-e` in a second tab opens a sidebar with a different daemon pid.
- `cmd-o` over an agent that printed `src/notes.md:3` shows the picker;
  typing `note` + Enter leaves the tab's nvim on `notes.md` line 3, focused.
- `<leader>ac` + `<leader>aS` in the sidebar: the agent receives "Code review
  comments … src/notes.md:3 … Comment: …" submitted; `<leader>as` leaves it in
  the input unsubmitted.
- Closing the tab stops its nvim; closing the space stops the rest and the
  `herdr-nvim-watch` terminal ends.
- Uninstall: keys removed, the layout's Files is the shipped browser again,
  the clone gone if nothing else changed it.

`herdr-app/e2e/run.sh` does every check above in a private app and core.
