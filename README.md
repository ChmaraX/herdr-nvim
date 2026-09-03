# herdr-nvim

[![CI](https://github.com/ChmaraX/herdr-nvim/actions/workflows/ci.yml/badge.svg)](https://github.com/ChmaraX/herdr-nvim/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/ChmaraX/herdr-nvim)](https://github.com/ChmaraX/herdr-nvim/releases)
[![License](https://img.shields.io/github/license/ChmaraX/herdr-nvim)](LICENSE)

Neovim, built into your [herdr](https://herdr.dev) workspace: a persistent
nvim sidebar one key away, with quick access to the files your agent works on.

<https://github.com/user-attachments/assets/11a41bf0-5b1b-4561-b543-481efa8be09b>

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

## Requirements

nvim ≥ 0.10 · herdr ≥ 0.7.4 · runs inside a herdr session

## Install

Both halves come from this repo:

**1. The herdr plugin** (sidebar + picker):

```sh
herdr plugin install ChmaraX/herdr-nvim
# or, for a local checkout: herdr plugin link /path/to/herdr-nvim
```

Bind keys to the two actions in `~/.config/herdr/config.toml` (herdr binds
none by default):

```toml
[[keys.command]]
key = "prefix+e"
type = "plugin_action"
command = "chmarax.herdr-nvim.toggle"
description = "nvim sidebar"

[[keys.command]]
key = "prefix+o"
type = "plugin_action"
command = "chmarax.herdr-nvim.pick-file"
description = "open file from agent output"
```

**2. The nvim plugin** (annotations), with lazy.nvim:

```lua
{ "ChmaraX/herdr-nvim", opts = {} }
```

## The sidebar

`prefix+e` toggles it. Each tab gets its own nvim, backed by a headless
daemon that survives the toggle. Two tabs can show two different files in two
sidebars. When you close and reopen a sidebar, it loses nothing. herdr
removes the daemons of closed tabs automatically.

## The file picker

`prefix+o` pops a fuzzy file picker. It has two modes:

- **Default view (no query):** the files touched this session, newest first.
  It mines edits from the agent's session log and adds uncommitted git
  changes. For agents that herdr does not track, it scrapes recent pane
  output instead. The cursor starts on the newest file, so `⏎` opens it with
  no typing.
- **Typing:** fuzzy matches across the **whole repo**, ranked best first.
  This includes every file that `git ls-files` reports, and it honors
  `.gitignore`. The match is on the path and filename, not the file contents.

Each row shows:

- the path, relative to the agent's cwd
- a `new` badge for files created this session
- green/red `+N -M` diff stats for uncommitted edits
- a relative touched-age (`2m`, `3h`)

If you start the picker from a non-agent pane (for example, the sidebar
itself), it reads the agent in the same tab. So it searches the repo that
you see.

The default view shows the latest `max_files` entries (20). A typed query is
uncapped.

## Annotations

Each action has a default keymap and a `:Herdr` subcommand (subcommands
tab-complete):

| Keymap | Command | Action |
| --- | --- | --- |
| `<leader>ac` | `:Herdr comment` | comment the current line / selection (the command also takes a range: `:5,10Herdr comment`) |
| `<leader>al` | `:Herdr list` | list comments (float): hover to jump, `⏎` edit, `d` delete |
| `<leader>as` | `:Herdr send` | paste all comments into the agent's input |
| `<leader>aS` | `:Herdr submit` | send all comments to the agent (auto-submits) |

Keymaps are on by default (prefix `<leader>a`) and never override a map you
already set. To bind your own, set `keymaps = false` and map the command:

```lua
require("herdr-nvim").setup({ keymaps = false })
vim.keymap.set({ "n", "x" }, "<leader>ac", "<CMD>Herdr comment<CR>", { desc = "Comment" })
```

Or call the Lua API directly (`comment_line`, `comment_selection`,
`comment_range(s, e)`, `list_comments`, `send_all{ submit = false|true }`).
See `:help herdr-nvim` for the full reference.

Sending skips the picker when the target is obvious: the lone agent in the
workspace, or the single agent sharing this tab (the sibling pane). The picker
only appears when two or more agents could plausibly be meant.

Comments are ephemeral by design: in-memory only, extmark-tracked (they follow
your edits), cleared after a successful send. The sent prompt includes each
comment's file:line plus the repo and branch, so the agent has context.

For a pending-comment indicator (`● 3`) in your statusline:
`require("herdr-nvim").statusline()`.

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
position = "right"   # right (default), left, top, or bottom

[picker]
scan_lines = 300    # pane lines scanned by the fallback text-scrape
max_files = 20      # session entries shown before you type a query
                    # (a typed query fuzzy-searches the whole repo, uncapped)
```

## Troubleshooting

```sh
herdr-nvim doctor                     # live checks: splits, toggle, daemon, remote-ui
herdr-nvim doctor --with-agent claude # also verify agent registration
```

Doctor runs labeled checks in a scratch workspace and always removes them
afterward. The most common failure is `daemon-healthy` FAIL: the nvim daemon
did not start. Make sure that `sidebar.nvim_bin` points at a working nvim ≥
0.10.

## Tests

```sh
just ci    # cargo fmt + cargo test + headless Lua suite
```
test
