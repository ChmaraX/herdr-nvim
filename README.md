# herdr-nvim

[![CI](https://github.com/ChmaraX/herdr-nvim/actions/workflows/ci.yml/badge.svg)](https://github.com/ChmaraX/herdr-nvim/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/ChmaraX/herdr-nvim)](https://github.com/ChmaraX/herdr-nvim/releases)
[![License](https://img.shields.io/github/license/ChmaraX/herdr-nvim)](LICENSE)

Neovim, built into your [herdr](https://herdr.dev) workspace: a persistent
nvim sidebar one key away, with quick access to the files your agent works on.

> [!TIP]
> Using the **herdr TUI** (terminal)? Its guide is [TUI.md](TUI.md).

https://github.com/user-attachments/assets/328ab066-29f3-4c39-84c4-30b98449e55e

> **Pairs well with**
> - [herdr-gitview](https://github.com/ChmaraX/herdr-gitview): review, stage, and commit agent changes
> - [herdr-chat](https://github.com/ChmaraX/herdr-chat): a native chat for Pi, Claude Code and Codex
> - [pi-declutter](https://github.com/ChmaraX/pi-declutter): see what the agent did at a glance

## Features

- **Full-height nvim sidebar, one key to toggle.** Your panes move into the
  left half, and nvim takes the right. Toggle it off, and herdr restores the
  original layout. Each tab keeps its own persistent nvim, so buffers, cursor,
  and pending annotations survive the toggle.
- **Fuzzy file picker.** It opens on the files your agent touched recently
  (newest first, with diff stats). Type to fuzzy-search the whole repo. `⏎`
  opens the file in the sidebar at the right line.
- **Code annotations you send to the agent.** Comment lines or a selection
  like a code review. Then send them all to any agent in the workspace (pi,
  claude, codex), with file:line and git context.
- **Inline file:line references.** Drop `path:12-20` into the agent's input
  mid-sentence.
- **A file tree beside nvim.** The tree comes with the sidebar, rooted at
  the tab's repo, with LazyVim's explorer keys. Picking a file opens it in
  the tab's nvim.
- **Preview.** On a markdown file, an image or an SVG, the **Preview**
  button on nvim's pane shows it rendered in nvim's place. **Source** brings
  nvim back as it was.

## Requirements

The Herdr app 0.1.11+ on macOS or Linux, and nvim ≥ 0.10. Without nvim the
installer offers to install it (Homebrew's, else Neovim's own release into
`~/.local/share/herdr-nvim/nvim`, no sudo).

## Install

With the app open, either ask the agent in your Herdr window to *add the mod
at github.com/ChmaraX/herdr-nvim*, or run:

```sh
curl -fsSL https://raw.githubusercontent.com/ChmaraX/herdr-nvim/main/install.sh | sh -s -- --app
```

It downloads the prebuilt binary (no clone, no Rust), adds the mods and the
keys (never one you already use), and points **Files** at the tree. Run it
again to upgrade; `--install-nvim` says yes to installing Neovim up front.

Options go after `--`:

```sh
curl -fsSL …/install.sh | sh -s -- --app -- --panel-width 340  # the Files panel's width (default 300)
curl -fsSL …/install.sh | sh -s -- --app --uninstall           # back to the stock Files browser
```

## Keys

| Key | Does |
| --- | --- |
| `cmd-e` / Files | open or close the tree with the tab's nvim |
| `cmd-shift-e` | the keyboard to the tree, on nvim's file |
| `cmd-o` | a file the agent worked on, opened in nvim |
| `<leader>e` (in nvim or the tree) | show or hide the tree, nvim staying open |

The first nvim pane shows a welcome with these keys, once
(`herdr-app do herdr-nvim.welcome` shows it again).

## The tree

The tree is rooted at the tab's repo and follows nvim's file. Opening a file
from it shows it in the tab's nvim, which takes the keyboard. It has
LazyVim's Snacks explorer keys (`/` find, `a` new, `r` rename, `d` delete,
Tab to select several, …), and `?` lists them all.

File operations go through the tab's nvim: a rename or a move takes the open
buffer with it (unsaved edits, undo and language servers follow), and a
delete closes it. Anything outside the tab's repo asks first.

Prefer nvim's own explorer (Snacks, neo-tree, nvim-tree)? Set
`tree = "nvim"` (see [Config](#config)): the Files button goes away, and
`cmd-e` opens nvim alone.

## The file picker

`cmd-o` opens on the files the tab's agent touched this session, newest
first. It reads them from the agent's session log (pi or Claude Code), adds
uncommitted git changes, and with no log falls back to the paths on the
agent's screen. The cursor starts on the newest file, so `⏎` opens it with
no typing.

Each row shows the path, a `new` badge for files created this session,
`+N -M` diff stats and how long ago it was touched. Typing fuzzy-matches the
whole repo (what `git ls-files` reports, `.gitignore` honored), served by
[fff-search](https://crates.io/crates/fff-search).

## Preview

On nvim's pane header, **Preview** appears while nvim's file is markdown, an
image or an SVG. The preview takes nvim's exact place: markdown rendered, an
image fitted. **Source** (**nvim** for an image) swaps nvim back, unsaved
edits and all. The preview reloads when its file is saved, and an image
opened any way opens its preview.

```` ```mermaid ```` blocks in markdown are drawn as diagrams, with nothing to
install: herdr-nvim draws them itself with
[merman](https://github.com/Latias94/merman), a Rust port of Mermaid (its
layouts and themes, light or dark following Herdr's). Each diagram is drawn
once per code and theme, so text edits around it redraw nothing; a broken one
shows Mermaid's error above its code.

## Comments

| Keymap | Command | Action |
| --- | --- | --- |
| `<leader>ac` | `:Herdr comment` | comment the current line / selection |
| `<leader>al` | `:Herdr list` | list comments: `⏎` edit, `d` delete |
| `<leader>as` | `:Herdr send` | paste all comments into the agent's input |
| `<leader>aS` | `:Herdr submit` | send all comments to the agent |
| `<leader>ai` | `:Herdr ref` | `path:line` at the agent's cursor |

They go to the tab's agent (or the space's only one); otherwise nvim asks
which. Comments are in memory only, follow your edits, and are cleared after
a send. The sent prompt includes each comment's file:line plus the repo and
branch. Keymaps, the prefix and the Lua API are the same as in the TUI:
[TUI.md › Annotations](TUI.md#annotations).

## Config

`~/.config/herdr-nvim/config.toml` (optional; a bad value warns and falls
back to the default):

```toml
[app]
tree = "native"         # or "nvim": nvim's own explorer, no Files button
tree_key = "<leader>e"  # the tree's show/hide key in nvim; "" for none

[sidebar]
nvim_bin = "nvim"       # the Neovim to run
nvim_env = []           # e.g. ["NVIM_APPNAME=myapp"] for a config in ~/.config/myapp
position = "right"      # or "bottom": nvim below instead of beside
```

The tree key is taken over when it opens a known explorer (Snacks, neo-tree,
nvim-tree), mapped when free, and left alone when it does anything else.
LazyVim's `<leader>E`, `<leader>fe` and `<leader>fE` show the tree too.

## Troubleshooting

- **Neovim missing:** `cmd-e` shows a card with **Install Neovim**.
- **Where the nvims are:** each tab's nvim lives as long as its tab; closing
  the tab (or its space) stops it and its language servers, discarding
  unsaved buffers. To see them with the memory each holds:

  ```sh
  ~/.config/herdr-app/mods/herdr-nvim/bin/herdr-nvim daemons
  ```

  `daemons stop <tab>` stops one (`--force` discards unsaved work).
- **nvim not answering:** a tab's nvim whose config gets stuck in a loop
  stops answering. `cmd-e`, a click in the tree and `cmd-o` then fail with a
  message instead of hanging: after 10 s for one that was already hung, 27 s
  at worst for an open that hangs halfway through (the sum of the steps'
  time limits, not a measured time). One that is only waiting for input (a
  half-typed command, a prompt) is sent `<C-\><C-n>` and carries on. A hung
  one is listed as `unresponsive` and never replaced by a second nvim on its
  socket; only a kill ends it, so `daemons stop <tab>` refuses it and
  `daemons stop <tab> --force` kills it (unsaved work in it is lost), as does
  closing its tab.
- **`nvim` not found:** nvim's pane says so and waits for Enter, naming what
  to fix: set `nvim_bin` under `[sidebar]` to nvim's full path (`command -v
  nvim` prints it) when the PATH the app passed on has no nvim. A
  Homebrew-installed nvim is found either way — the common Homebrew prefixes
  (macOS and Linux) are prepended to the PATH nvim is started with.
- **After an upgrade:** running nvims keep the old version until their pair
  is closed and opened again (one with unsaved work is kept, with a toast).

## Development

From a clone (building herdr-nvim with cargo), with the app open:

```sh
herdr-app/install.sh              # install, or upgrade in place
herdr-app/install.sh --uninstall  # take it out
just ci                           # cargo fmt + cargo test + headless Lua suite
node herdr-app/e2e/js-checks.mjs  # the mods' logic
herdr-app/e2e/run.sh              # the main flows in a private app (--bundle: through the root installer)
```

A release ships an app bundle per platform (`herdr-app/bundle.sh`: the
binary, its Lua and the mods), which the root `install.sh` downloads.

How it's built and how it behaves in detail: [herdr-app/RECIPE.md](herdr-app/RECIPE.md).
Other mods open a file in a tab's nvim with
`<app home>/mods/herdr-nvim/bin/herdr-nvim open --tab <tab id> <file>[:line[:col]]`.
