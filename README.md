# herdr-nvim

Annotate code in nvim, send the annotations to any AI agent running in
[herdr](https://herdr.dev) — without leaving your editor.

## Requirements

- nvim ≥ 0.10, herdr ≥ 0.7.0, running inside a herdr session

## Install (lazy.nvim)

```lua
{ "adamchmara/herdr-nvim", opts = {} }
```

## Usage

| Mapping | Action |
|---|---|
| `<leader>ac` (visual) | comment the selection |
| `<leader>ac` (normal) | comment the current line |
| `<leader>al` | list comments — jump / edit / delete |
| `<leader>as` | paste all comments into a chosen agent's input (you press Enter) |
| `<leader>aS` | send all comments to a chosen agent (auto-submits) |

## Config

```lua
require("herdr-nvim").setup({
  prefix = "<leader>a",     -- keymap prefix
  keymaps = true,           -- set false to define your own
  clear_after_send = true,  -- comments are ephemeral by design
})
```

Comments live in memory only (extmark-tracked, so they follow your edits) and
are cleared after a successful send.

## Sidebar (herdr plugin)

herdr-nvim also ships as a [herdr](https://herdr.dev) plugin that toggles a
full-height nvim sidebar in your current workspace:

```sh
herdr plugin link /path/to/herdr-nvim   # local checkout (dev)
# or, once published: herdr plugin install adamchmara/herdr-nvim
```

Toggling **on** squeezes the tab's existing panes into the left half and opens
nvim full-height in the right half, focused. Toggling **off** closes the sidebar
and restores the original layout exactly. The sidebar follows you across tabs
(closes where it was, opens where you are), and each workspace gets its own
persistent headless nvim daemon — so buffers, cursor and in-flight comments
survive a toggle off/on.

The sidebar is driven by a single plugin action. Trigger it directly:

```sh
herdr plugin action invoke toggle --plugin adamchmara.herdr-nvim
```

No key is bound by default. To toggle with a keypress, add a binding to your
`~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+e"                            # any key you like
type = "plugin_action"
command = "adamchmara.herdr-nvim.toggle"
description = "nvim sidebar"
```

## Open a file from agent output

When an agent mentions or edits files, the file bridge lets you jump straight to
one in the sidebar. It scans the focused (or workspace's) agent pane's recent
output for real file paths (with optional `:line` suffixes), pops an overlay
picker — newest paths first, type to filter — and on Enter opens the chosen file
in the workspace nvim sidebar (opening the sidebar first if needed), at the right
line, focused.

Trigger it directly on the focused agent pane:

```sh
herdr plugin action invoke pick-file --plugin adamchmara.herdr-nvim
```

No key is bound by default. To trigger with a keypress, add a binding to your
`~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+o"                            # any key you like
type = "plugin_action"
command = "adamchmara.herdr-nvim.pick-file"
description = "open file from agent output"
```

Esc (or `q`) in the picker dismisses it without touching the sidebar.

Run `herdr-nvim doctor` to self-test the live herdr + nvim integration.

## Tests

`nvim --headless --noplugin -u NONE -l tests/run.lua`
