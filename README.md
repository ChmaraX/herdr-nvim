# herdr-nvim

Annotate code in nvim, send the annotations to any AI agent running in
[herdr](https://herdr.dev) — without leaving your editor.

## Requirements

- nvim ≥ 0.10, herdr ≥ 0.7.4, running inside a herdr session

## Install (lazy.nvim)

```lua
{ "adamchmara/herdr-nvim", opts = {} }
```

## Usage

| Mapping | Action |
|---|---|
| `<leader>ac` (visual) | comment the selection |
| `<leader>ac` (normal) | comment the current line |
| `<leader>al` | comment list (float): hover to jump, `⏎` edit, `d` delete, `q` close |
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

**What it shows:** two sections — **EDITED** (files the agent's session
actually wrote/edited, mined from its session log where herdr tracks one,
plus anything with uncommitted git changes in the pane's worktree) and
**MENTIONED** (files the agent read this session, or — for agents herdr
doesn't track a session for — the same text-scrape of recent pane output
`pick-file` has always used, over the last `picker.scan_lines` lines,
default 300). A session-edited file whose worktree is clean and wasn't
committed during the session (i.e. the edit was rolled back) is demoted
from EDITED to MENTIONED rather than dropped. Either section is omitted
entirely when it's empty. Both sections are newest-first, existence-filtered
to real files on disk, and never show edit counts or git status
letters/deltas — that detail belongs to git tooling, not this picker.

Each row shows a smart, shortened path (relative to the pane's working
directory, `~`-shortened outside it, filename in bold), a `new` badge for
files created this session, and a relative age (`2m`, `1h`, `3d`) from the
last edit when known. Typing filters by **whole path**, not just the
filename (case-insensitive substring, matched span highlighted); the footer
shows `matched/total`. The cursor starts on the newest EDITED entry, so
pressing Enter with no typing opens the file the agent just worked on.

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

## Ctrl+click to open a file

Agent panes also linkify file paths and `file://` links (including agents'
own OSC 8 links, e.g. Claude Code's) directly in their terminal output —
**Ctrl+click** one to open it in the tab's nvim sidebar (opening the sidebar
first if it's closed), at the right line, focused. This is herdr's fixed
link-handler modifier (not configurable by this plugin) and needs herdr ≥
0.7.4.

The path must include at least one directory segment and a file extension
(e.g. `src/main.rs`, `./lib/util.py:42`, `/abs/path/file.rs:10:3`,
`~/project/notes.md`) — a bare filename like `README.md` is intentionally
**not** linkified (`Node.js`, `e.g.`, and similar prose would otherwise become
false positives); use the picker above for those. Relative paths resolve
against the clicked pane's working directory, then that directory's git
toplevel. A path that doesn't resolve to a real file is a silent no-op.

No key to bind — herdr wires Ctrl+click to the plugin's `open-link` action
automatically for every `link_handlers` match.

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
