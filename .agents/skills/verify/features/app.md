# Herdr desktop app

herdr-nvim also serves the Herdr desktop app (herdr-app + herdr-core), from the same binary and Lua: `HERDR_NVIM_HOST=app`, set only by the app's wrapper, selects it. The full brief is [herdr-app/RECIPE.md](../../../../herdr-app/RECIPE.md); the TUI plugin is unchanged without that variable.

## Sub-features

- pair: the tab bar's Files button or `cmd-e` opens the native tree (the layout's `files` slot) and the tab's nvim pane (a full-height split of the tab, backed by a per-tab daemon) together, and closes both. The panel follows the tab on tab and space switches.
- tree: LazyVim's Snacks explorer keys (`mods/herdr-nvim/tree-keys.js`); file operations done by the tab's nvim (`lua/herdr-nvim/fs.lua`), so a rename keeps a buffer's unsaved edits; nothing outside the tab's folder (symlinks resolved) without a confirm; `cmd-shift-e` and the tree key (`[app] tree_key`) move between nvim and the tree.
- picker: `cmd-o` (`herdr-nvim.pick`) lists the files the agent's session log shows it touched, then git changes, in a dialog; ⏎ opens one in the tab's nvim.
- preview: the nvim pane's header (`mods/herdr-nvim-header`) has a Preview button for markdown, images and SVGs; the button (and an image opened any way: Enter in the tree, cmd-o, `open`) asks the controller (`sync.js` `startSwap`, `herdr-nvim.swap:<tab>:<file>`), which splits across nvim's slot and closes the old pane once the new one is there, so the preview has nvim's exact size; Source does the reverse on the same daemon; Files closes and reopens it with the tree. No e2e yet.
- open: `<appHome>/mods/herdr-nvim/bin/herdr-nvim open --tab <tab> <file>[:line[:col]]` opens a file in that tab's nvim (the contract other mods use).
- install: `herdr-app/install.sh` (a stamped runtime per install, the mod swapped in by a rename, keys merged where free, the person's layout edited as RECIPE.md's Layout footprint says); `--uninstall [--force]` takes it all out and keeps every layout, and keeps nvims with unsaved work unless `--force`.
- stale nvims: an install stops no running nvim; it reports the stale ones. Reopening a pair restarts a stale clean nvim; one with unsaved work is kept with a toast.

## How to get to it (user POV)

Install into a Herdr app home with `herdr-app/install.sh`, then use Files / `cmd-e` / `cmd-o` in the window.

## Driving it

- Not in the TUI box (macOS app). `herdr-app/e2e/run.sh --app-bin <Herdr binary> --proof DIR` is a smoke test of the main flows (about a minute) against a private app and a private core (its own home, XDG dirs and sockets), posting keys to that app's PID only. It writes `run.log`, `app.log` and screenshots into `DIR`. `--only <flows>` (comma-separated: picker, tree, preview, teardown) runs the setup and the first flow, then just those.
- Never run it against the person's own app home, core or daemons; never foreground a window; drop `HC_SCOPE` for every `hc` call.

## Gotchas

- Shipped layouts can't change content: the install uses or makes the person's copy (`<name>-copy`).
- `cmd-o` and `cmd-shift-e` are controller commands: they need the patched layout on screen.
- `pane.focus` fails on overlay terminals; dialogs take the keyboard.
