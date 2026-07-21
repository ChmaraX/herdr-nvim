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

### Statusline

`require("herdr-nvim").statusline()` returns `""` with no pending comments, or
`"● 3"` with 3 pending — drop it straight into lualine:

```lua
require("lualine").setup({
  sections = {
    lualine_x = { require("herdr-nvim").statusline },
  },
})
```

## Sidebar (herdr plugin)

herdr-nvim also ships as a [herdr](https://herdr.dev) plugin that toggles a
full-height nvim sidebar in your current workspace:

```sh
herdr plugin link /path/to/herdr-nvim   # local checkout (dev)
# or, once published: herdr plugin install adamchmara/herdr-nvim
```

Toggling **on** squeezes the tab's existing panes into the left half and opens
nvim full-height in the right half, focused. Toggling **off** closes the sidebar
and restores the original layout exactly. **Each tab gets its own independent
nvim** — its own buffers, cursor and in-flight comments — backed by a persistent
per-tab headless daemon that survives a toggle off/on. So you can have two tabs
open with two different files in two separate sidebars at the same time; they
don't share state and don't follow you between tabs. (Stale per-tab daemons from
closed tabs are reaped opportunistically on the next toggle.)

```
before toggle                     after toggle
+-----------------+               +--------+--------+
|                 |               |        |        |
|   your panes    |   prefix+e    | your   |  nvim  |
|  (agent, shell,  -------------->| panes  | sidebar|
|   etc.)         |               | (left  | (right,|
|                 |               |  half) | full-  |
|                 |               |        | height,|
|                 |               |        | focused|
|                 |               |        | )      |
+-----------------+               +--------+--------+
```

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
one in the sidebar. It pops a floating picker — newest paths first, type to
filter — and on Enter opens the chosen file in the tab's nvim sidebar (opening
the sidebar first if needed), at the right line, focused.

**What it shows:** file paths found in the last `picker.scan_lines` (default 300)
lines of the focused agent pane's output — anything the agent *mentioned, read,
diffed, edited, or printed* — newest first, filtered to paths that exist on disk.
It is a text-scrape of recent terminal output, **not** a semantic list of files
the agent changed this turn, and it is not turn-scoped. (A real "files the agent
actually edited" source via agent hooks is planned — see the M5 design note.)

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

## herdr-nvim config (Rust side)

The sidebar/picker binary reads its own optional TOML config, separate from
the Lua `setup{}` above — `~/.config/herdr-nvim/config.toml` (override the
path with `HERDR_NVIM_CONFIG`). A missing file uses the defaults below; a
malformed file also falls back to defaults (with one warning on stderr) —
a bad config file never breaks the plugin.

```toml
[sidebar]
nvim_bin = "nvim"     # binary used to spawn the per-tab nvim daemon

[picker]
scan_lines = 300      # trailing pane lines scanned for file paths by pick-file
```

## Doctor / troubleshooting

```sh
herdr-nvim doctor                     # core checks: splits, toggle, daemon, remote-ui attach
herdr-nvim doctor --with-agent claude # also verifies a live agent registers with herdr
```

`doctor` creates its own scratch workspace (`herdr-nvim-doctor`), runs a set of
labelled live checks against it (`OK`/`FAIL` per check), and always cleans up
after itself — closing the workspace and killing any daemon/nvim it spawned —
even on failure or panic. If it reports a failure:

- **`daemon-healthy` FAIL** — `herdr-nvim`'s spawned nvim daemon didn't come up;
  check that `sidebar.nvim_bin` in your config (if set) points at a working
  `nvim` ≥ 0.10 on `PATH`.
- **`remote-ui-attach` FAIL** — `nvim --remote-ui` couldn't reach the daemon
  socket; check for stale sockets under the runtime dir, or that no other
  process is holding the socket.
- **`toggle-roundtrip-restores-rects` FAIL** — pane layout wasn't restored
  exactly after a toggle on/off; run again outside of any other herdr
  automation that might be racing pane state.
- **workspace cleanup FAIL** ("still present: true") — the scratch workspace
  didn't close; check `herdr workspace list` and close it manually with
  `herdr workspace close <id>`.

If everything is fine, doctor ends with `all doctor checks OK` and exit code 0.

## Tests

`nvim --headless --noplugin -u NONE -l tests/run.lua`
