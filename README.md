# herdr-nvim

Neovim, fully integrated into your [herdr](https://herdr.dev) workspace — a
persistent nvim sidebar one key away, with quick access to the files your
agent is working on.

![herdr-nvim sidebar](docs/assets/screenshot.png)

## Features

- **Full-height nvim sidebar, one key to toggle.** Your panes squeeze into
  the left half, nvim takes the right; toggle off and the original layout is
  restored exactly. Each tab keeps its own persistent nvim — buffers, cursor,
  and pending annotations survive the toggle.
- **File picker for the agent's session.** Every file the agent edited or
  mentioned, newest first, with diff stats — Enter opens it in the sidebar
  at the right line.
- **Code annotations that go back to the agent.** Comment lines or selections
  like a code review, then send them all — with file:line and git context —
  to any agent in the workspace (pi, claude, codex, …).

## Requirements

nvim ≥ 0.10 · herdr ≥ 0.7.4 · runs inside a herdr session

## Install

Both halves come from this repo:

**1. The herdr plugin** (sidebar + picker):

```sh
herdr plugin install adamchmara/herdr-nvim
# or, for a local checkout: herdr plugin link /path/to/herdr-nvim
```

Bind keys to the two actions in `~/.config/herdr/config.toml` (none are bound
by default):

```toml
[[keys.command]]
key = "prefix+e"
type = "plugin_action"
command = "adamchmara.herdr-nvim.toggle"
description = "nvim sidebar"

[[keys.command]]
key = "prefix+o"
type = "plugin_action"
command = "adamchmara.herdr-nvim.pick-file"
description = "open file from agent output"
```

**2. The nvim plugin** (annotations), with lazy.nvim:

```lua
{ "adamchmara/herdr-nvim", opts = {} }
```

## The sidebar

`prefix+e` toggles it. Each tab gets its own independent nvim, backed by a
headless daemon that survives toggling — two tabs can sit on two different
files in two sidebars, and closing/reopening the sidebar loses nothing.
Daemons from closed tabs are cleaned up automatically.

## The file picker

`prefix+o` on an agent pane pops a picker of files touched this session:
edits mined from the agent's session log plus uncommitted git changes, with a
text-scrape of recent pane output as fallback for agents herdr doesn't track.

- newest-touched first; cursor starts on the file the agent just worked on,
  so `⏎` with no typing opens it
- `new` badge for files created this session, green/red `+N -M` diff stats
  for uncommitted edits
- shows the latest 20 by default; typing filters the **full** session list
  (case-insensitive, whole path)
- `Esc`/`q` dismisses; `⏎` opens in the sidebar at the right line, opening
  the sidebar first if needed

## Annotations

| Mapping | Action |
|---|---|
| `<leader>ac` | comment the current line / visual selection |
| `<leader>al` | list comments (float): hover to jump, `⏎` edit, `d` delete |
| `<leader>as` | paste all comments into a chosen agent's input |
| `<leader>aS` | send all comments to a chosen agent (auto-submits) |

Comments are ephemeral by design: in-memory only, extmark-tracked (they follow
your edits), cleared after a successful send. The sent prompt includes each
comment's file:line plus the repo and branch, so the agent has context.

For a pending-comment indicator (`● 3`) in your statusline:
`require("herdr-nvim").statusline()` — returns `""` when there's nothing
pending.

## Config

Two small config surfaces, one per half:

**nvim side** — `setup{}` opts:

```lua
require("herdr-nvim").setup({
  prefix = "<leader>a",     -- keymap prefix
  keymaps = true,           -- set false to define your own
  clear_after_send = true,  -- comments are ephemeral by design
})
```

**herdr side** — `~/.config/herdr-nvim/config.toml` (optional; missing or
malformed files fall back to these defaults):

```toml
[sidebar]
nvim_bin = "nvim"   # binary used to spawn the per-tab nvim daemon

[picker]
scan_lines = 300    # pane lines scanned by the fallback text-scrape
max_files = 20      # entries shown before you type a filter
```

## Troubleshooting

```sh
herdr-nvim doctor                     # live checks: splits, toggle, daemon, remote-ui
herdr-nvim doctor --with-agent claude # also verify agent registration
```

Doctor runs labelled checks in a scratch workspace and always cleans up after
itself. Most common failure: `daemon-healthy` FAIL means the nvim daemon
didn't start — check that `sidebar.nvim_bin` points at a working nvim ≥ 0.10.

## Tests

```sh
just ci    # cargo fmt + cargo test + headless Lua suite
```
