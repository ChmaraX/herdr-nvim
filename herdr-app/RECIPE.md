# herdr-nvim

Neovim beside your agents. The Files button (or `cmd-e`) opens a file tree of
the tab's repo with the tab's own nvim next to it, and a file you pick opens
there. `cmd-o` lists the files the agent just read and edited, and comments
you write in nvim go to the agent with their `file:line`.

![herdr-nvim](preview.png)

## Adding it (for the app's agent)

Don't copy the folders: herdr-nvim needs its binary, and it edits the
person's layout, which copying doesn't do. With the app open, run its
installer:

```sh
curl -fsSL https://raw.githubusercontent.com/ChmaraX/herdr-nvim/main/install.sh | sh -s -- --app
```

Tell the person first what it does:

- downloads the prebuilt `herdr-nvim` for this machine from the GitHub
  release, checked against its SHA-256, into `~/.local/share/herdr-nvim/app`
  (macOS and Linux);
- runs programs (an nvim per tab, `hc`, `git`) and reads the tab's repo and
  the agent's session logs; the mod itself uses no network;
- adds two mods (`herdr-nvim`, `herdr-nvim-header`), adds its keys where they
  are free, and edits the layout on screen (on one of Herdr's own, a copy of
  it, which it switches to).

It needs Neovim 0.10+. Without it the installer asks to install it
(Homebrew's, else Neovim's own release into `~/.local/share/herdr-nvim/nvim`,
no sudo); add `--install-nvim` once the person has said yes. Running the
installer again upgrades; `--uninstall` takes all of it out.

## What you see

- **Files panel**, at the window's right edge, 300px: a 42px header with
  "Files" and the folder name in `dim`, and a spinner while a file opens.
  28px rows: chevron, the shipped `files` mod's icons, name, git mark (`A` in
  `ansi2`, `D`/`U` in `ansi1`, `M` in `blocked`). Selected row `selected`,
  hover `hover`; a 1px `border` frame with 10px corners, `accent` while the
  tree has the keyboard. Away from the tab's folder, the header shows crumbs
  back to it (`accent`). Footer: "? keys" in `dim`.
- **nvim pane**, just left of the panel, as tall as the tab; the tab's other
  panes keep their arrangement on its left. Its header reads `nvim · <file>`.
- **Preview button** on that header while nvim's file is markdown, an image
  or an SVG (11.5px, a 1px `border` pill). The preview takes nvim's place at
  nvim's size: markdown rendered, an image fitted with 14px around it. Its
  header reads `Preview · <file>` with a **Source** button (**nvim** for an
  image).
- **Picker** (`cmd-o`): an 820×470 dialog, "Open file from agent output" on a
  38px `chrome` bar with its keys as keycaps. Below it, a terminal picker:
  the agent's files newest first, then uncommitted git changes with `+N -M`
  and ages; typing fuzzy-searches the whole repo.
- **Cards**: "nvim is ready", once after the first nvim pane, with the keys
  as the person's `keys.json` binds them and where keys and settings live.
  Without Neovim, "Neovim 0.10 or newer is needed" with **Not now** and
  **Install Neovim**.

## How it behaves

- **Files = `cmd-e`.** In a tab without them, both open: the panel, and the
  tab's nvim pane with the keyboard. Pressed again, both close. Closing never
  stops the tab's nvim, and its screen is only put away: Files again shows it
  as it was, at once (about 30 ms), with the same buffers, cursor and
  language servers. The first open in a tab, or one in a tab with several
  panes, starts nvim's screen in a new pane (300–500 ms).
- **One nvim per tab, one panel per window.** The panel follows the tab you
  switch to: shown where that tab's nvim pane is open, hidden where it isn't.
  Looking at a tab never starts an nvim. If nvim's pane opens another way
  (the picker, another mod), the panel opens with it; if it closes (`:q`),
  the panel goes too. Switching the panel to Changes leaves nvim open.
- **The tab's folder** is the git repo the tab's terminals work in (the
  focused one's folder, else the first one's), else that folder itself. nvim
  starts there and the tree is rooted there.
- **The tree** gets the keyboard from a click, from the pane keys moving
  right, or from `cmd-shift-e`, which also selects nvim's file. A file opened
  from it shows in the tab's nvim, which takes the keyboard. It refreshes
  when files change (a folder watch); git marks at most every 2 s.
- **The tree key** (`<leader>e` by default) works like LazyVim's explorer:
  in nvim it shows the tree on nvim's file, or hides it with nvim kept open;
  in the tree it hides the tree and goes back to nvim. A key that opens
  Snacks, neo-tree or nvim-tree is taken over; a free key is mapped; one
  mapped to anything else is left alone. LazyVim's `<leader>E`, `<leader>fe`
  and `<leader>fE` show the tree too.

The tree's keys (LazyVim's Snacks explorer defaults; `?` lists them):

| Key | Does |
|---|---|
| `j`/`k`, ↓/↑ | move |
| `g`/`G` | first / last row |
| `l`, ⏎, → | open the file in the tab's nvim (it takes the keyboard), or open/close the folder |
| `h`, ← | close the folder (the open one, else the row's) and stand on it |
| `/` | find: type to fuzzy-search every file under the root (fff); ⏎ opens, Esc returns |
| Backspace | root up a folder |
| `.` | the row's folder becomes the root |
| `~` | root: back to the tab's folder |
| `q`, Esc | back to the tab's nvim (Snacks closes its window; here the pair closes with cmd-e) |
| `a` | new file in the row's folder (a dialog); end with `/` for a folder; a new file opens in nvim |
| `r` | rename (a path in the root; nvim's buffers, unsaved edits and language servers follow) |
| Tab | select the row (●) for `m`, `c`, `d`, `y` |
| `m` | move the selected rows into the row's folder (confirm); with none selected, rename |
| `c` | copy the selected rows into the row's folder (confirm); with none, copy to a new name |
| `d` | delete (confirm); to the trash when the person's Snacks says so; their buffers close |
| `y` / `p` | yank the paths into nvim's register / paste those files into the row's folder |
| `o` | open with the system's app |
| `H` | show hidden files (starts as the person's Snacks explorer has it) |
| `I` | show git-ignored files (starts as the person's Snacks explorer has it) |
| `Z` | close every folder |
| `u` | list again |
| `?` | every key, in a list |
| the tree key (`space` `e`) | hide the tree, back to nvim (nvim stays) |

- **File operations go through the tab's nvim**: a rename or move renames the
  open buffer, so unsaved edits, undo and language servers follow; a delete
  closes its buffers. Snacks' rename and trash are used when present.
  Anything outside the tab's folder, a symlink's target included, asks first.
- **Preview / Source** swap in place, in about 40 ms; nvim's screen waits
  behind the preview, unsaved edits and all. Opening any file in nvim brings nvim back. The preview
  reloads when its file is saved. Files closes a preview with the tree and
  brings it back next time. An image opened any way opens its preview.
- **Picker** reads the tab's agent: its session log (pi or Claude Code) since
  the agent started, then git changes; with no log, the paths on its screen.
  ⏎ opens the file in the tab's nvim, Esc closes. With nothing to list, an
  8-second warning toast says why.
- **Comments to the agent** (nvim, `<leader>a…`): `ac` comments the line or
  selection, `al` lists them, `as` pastes them all into the agent's input,
  `aS` sends them (submitted once the person stops typing for ~3 s), `ai`
  inserts `path:line`. They go to the tab's agent (the space's only one, or
  the tab's); otherwise nvim asks which.
- **No Neovim**: opening nvim shows the card instead. Install opens an
  "Install Neovim" tab that installs it, points herdr-nvim at it, and closes
  on Enter; `cmd-e` then works. `herdr-app do herdr-nvim.welcome` shows the
  welcome again.
- **Closing** a tab stops its nvim and its language servers; closing a space
  stops all of its tabs'. Their unsaved buffers are lost.
- **Upgrades** stop no running nvim. The install reports the ones still on
  the old version; reopening a tab's pair restarts a clean one, and one with
  unsaved work is kept, with a toast saying to save and reopen.
- **Settings** (`~/.config/herdr-nvim/config.toml`): `[app] tree = "nvim"`
  takes the tab bar's Files button out (the app's `workspace.panels`; only
  what herdr-nvim took out goes back, on `"native"` or uninstall) and makes
  cmd-e toggle only the nvim pane, with nvim's own explorer inside (default
  `"native"`, the tree above); `[app] tree_key` is any lhs, or `""`
  for none (read when nvim starts); `[sidebar] nvim_bin` is the Neovim to
  run; `[sidebar] position = "bottom"` puts nvim below instead of beside. A
  bad value warns and falls back to the default.
- **Other mods** open a file in a tab's nvim with
  `<appHome>/mods/herdr-nvim/bin/herdr-nvim open --tab <tab id> <file>[:line[:col]]`
  (prints `{"terminal": "tN"}`); the panel opens with it.

## Built from

- **The `herdr-nvim` binary** (Rust and Lua, in this repo; the same one the
  herdr TUI plugin uses), run through the wrapper `mods/herdr-nvim/bin/herdr-nvim`,
  which sets `HERDR_NVIM_HOST=app`. It makes every nvim and every pane: one
  headless nvim per tab, shown in the pane with `nvim --remote-ui`. A rebuild
  reuses it; its commands are the mods' whole interface:
  - `toggle --state open|close [--preview F]`: opens or closes the tab's nvim
    pane (`pane.split` right of the tab, labelled `herdr-nvim.sidebar=<tab>`),
    or puts a preview of F in its place;
  - `open [--no-focus] <file>[:line[:col]]`; `fs '<json>'` (the tree's file
    operations, done by nvim); `pick-file` (prints `{"terminal": "tN"}`, a
    picker in a hidden core terminal, or `{"empty": "<why>"}`);
    `app-preview`; `app-current` (nvim's file); `app-nvim-check`;
    `app-install-nvim`;
  - each takes `--ctx` (the focused space, tab, terminal), exits 3 with
    `{"nvim_missing": …}` when there's no usable Neovim, and ends within 30 s.
- **Mod `herdr-nvim`** (`place: none`, focusable), by where it is shown:
  - the layout's `files` slot: the tree (`tree-view.js`; listing by `sh`,
    marks from `git status --porcelain -z`, `watchFolder`);
  - a floating 0×0 block (`role: "sync"`), always rendered: the controller
    (`sync.js`) that keeps the panel and each tab's nvim pane together, and
    takes the commands `herdr-nvim.pick`, `.reveal`, `.toggle`,
    `.swap:<tab>:<file>`, `.files`, `.welcome` and `.uninstall`;
  - an overlay (`mod.open:herdr-nvim`, `cmd-e`): presses the Files button,
    then closes;
  - props `{preview}`: the preview pane; `{bar}`: the picker's title bar;
    `{card}` and `{dialog}`: the cards and the tree's prompts.
- **Parked screens** (`sync.js` `openFast`, `previewFast`): closing a tab's
  nvim pane is `terminal.park` (the screen keeps running in no pane); Files
  again is `terminal.keep {place: beside}` with the tab's one own pane
  focused; Preview is `pane.split {mod, props: {preview, tab}}` across nvim's
  pane, then `terminal.park`; Source is `terminal.keep {place: across}` with
  the preview focused, then `pane.close` of the preview. No herdr-nvim
  process runs for any of them. "nvim is open" is a labelled terminal that
  a pane shows (`panel.js` `nvimPanes`). The first open, a tab with several
  panes (`keep` can only go beside the focused pane) and a screen from
  before the mod's build go through `toggle`, which ends the tab's parked
  screen first.
- **Mod `herdr-nvim-header`**: the app's shipped pane header (`pane-base.js`,
  copied from `mods/pane/main.js` at install) plus the Preview/Source button.
- **Keys** (`keys.json`, merged into `global` only where free): `cmd-e` →
  `mod.open:herdr-nvim`, `cmd-o` → `herdr-nvim.pick`, `cmd-shift-e` →
  `herdr-nvim.reveal`. An install or uninstall also removes keys bound to
  actions herdr-nvim no longer has (`setup.rs` `RETIRED`).

### Layout footprint

The install edits the one layout on screen and nothing else
(`src/app/layout_patch.rs`); uninstall takes exactly this out:

- `files` (the `panel-body` stack's Files child) gets `{mod: "herdr-nvim",
  props: {replaces: <old>}}`; a layout without one gets Herdr's side panel
  right of its tab block, recorded in `props.added`;
- `floating` gets the sync block `{id: "herdr-nvim-sync", content: {mod:
  "herdr-nvim", props: {role: "sync"}}}`;
- each of the app's own pane headers becomes `{mod: "herdr-nvim-header",
  props: {replaces: <old>}}`;
- the panel becomes 300px wide, only while it is still the shipped 368.

Herdr's own layouts can't be changed: on one, the install uses the person's
copy of it, else clones it once (`<name>-copy`), and switches to it.

## Watch out

- **It edits the person's layout**, against AGENTS.md's "the person's own
  layout stays as it was": no API lets a mod take the tab bar's Files button
  or add a tab-bar button of its own (herdrdev/herdr-app#246). The edit is
  the footprint above, and uninstall reverses it exactly.
- The copy (`main-copy`) stops following Herdr's updates to `main`: install
  again after an app update.
- A hidden block's mod copy isn't rendered, so the tree can't see the panel
  close: that's what the always-rendered sync block is for.
- Don't put the nvim terminal itself in the `files` slot: a terminal block is
  removed from the layout file when its terminal ends.
- `terminals()` lags a command by a moment: the controller must not read its
  own just-closed pane as still open.
- Never `setShared` a `null`: the app can't read it back, and every view
  stops re-rendering.
- A mod reads files only inside the app home: the preview `cat`s files from
  elsewhere, and copies images into `mods/herdr-nvim/.cache` for `image()`.
- An overlay's terminal can't be focused by command; only a dialog focuses
  its first terminal when it opens, so the picker is a dialog.
- A process started from a terminal that keeps using `hc` after the terminal
  ends must drop `HC_SCOPE` and `HC_TERMINAL_ID`, or herdr-core ends it too.
- The tree reaches nvim and back with `pane.go` left and right, so the panel
  must be right of nvim.
- A parked screen may run the herdr-nvim from before an install: the install
  ends every parked one, and the controller closes (not parks) a screen that
  ran before its build loaded, so the next Files makes a new one and
  restarts a stale clean nvim.
- `terminal.keep` goes beside the focused pane, so focus is set first and
  waited for (the app applies commands a moment later); without the wait it
  lands beside the wrong pane.
- Cmd+click on a path in a terminal opens the system's app, not nvim (no
  link handlers yet, herdrdev/herdr-app#228).
- In 0.1.12 the tree can show the focus ring without getting keys: it takes
  the keyboard again for 2 s after each arrival (herdrdev/herdr-app#362,
  fixed upstream, not released yet).
- Remote POSIX hosts need Neovim 0.10+ and hc. `cmd-e` and `open --tab
  <machine/tab>` use their own Neovim config/explorer; the native tree,
  picker, annotations and rendered previews remain local-only.

## Done when

- `herdr-app check` passes for both mods, `herdr-app status` shows no
  errors, and installing again reports "already uses herdr-nvim".
- Files in a tab opens the 300px tree and the tab's nvim, focused; a file
  clicked in the tree opens in it; Files again closes both, and nvim keeps
  running.
- A second tab gets its own nvim; switching tabs shows each tab's own state
  and opens nothing.
- `cmd-o` over an agent whose log read `package.json`, then edited
  `src/notes.md`, lists `src/notes.md`, `package.json`, then the git change;
  ⏎ opens the file in nvim.
- `r` in the tree renames a file, and nvim's buffer follows with its unsaved
  edits.
- Preview on a markdown file replaces nvim at its size; Source brings the
  same nvim back.
- `<leader>ac` then `<leader>aS`: the agent receives the comment with its
  `file:line`, submitted.
- With no Neovim, `cmd-e` shows the card; after Install, `cmd-e` opens nvim
  on the installed Neovim.
- Closing a tab stops its nvim; uninstall leaves the layout exactly as it
  was.
- The key table above matches `node mods/herdr-nvim/tree-keys.js`.

`herdr-app/e2e/run.sh` checks these in a private app and core in about a
minute; `--only nvim-install` adds the Neovim install, recorded as a video.
