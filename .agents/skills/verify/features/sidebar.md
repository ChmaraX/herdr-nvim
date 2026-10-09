# Sidebar

A full-height nvim on one side of the herdr tab. One key toggles it, and each tab has its own nvim behind it that stays alive.

## Sub-features

- toggle: `prefix+e` opens a full-height pane about 50% wide and focuses it. Pressing it again closes the pane and restores the previous layout of the other panes.
- position: `[sidebar] position = "right"` (default), `"left"`, `"top"` or `"bottom"` in `~/.config/herdr-nvim/config.toml`.
- per-tab nvim: each tab has its own hidden nvim. After close and reopen, the buffers, cursor and pending comments are still there. Two tabs can show different files.
- auto-cleanup: closing a tab or a workspace stops that tab's nvim and discards unsaved buffers. After a herdr restart, tabs that come back reattach to their nvim, and nvims whose tab is gone are stopped.
- nvim config: `nvim_bin` sets the nvim binary (needs nvim ≥ 0.10). `nvim_env = ["NVIM_APPNAME=myapp"]` loads the user's config; without it the sidebar runs vanilla nvim. `:Herdr` and the default keymaps work either way.

## How to get to it (user POV)

Press `prefix+e` in any pane of a tab. It works with one pane or many. Other panes are squeezed into the other half. To change the side or the nvim used, edit `config.toml`, then toggle.

## Driving it in the box

- Toggle: `Ctrl+B`, `Type "e"`, `Wait+Screen /nvim sidebar/`. The template is `smoke/sidebar.tape`.
- Without keys: `box exec <item> -- herdr plugin action invoke toggle --plugin chmarax.herdr-nvim` toggles in the focused pane.
- Layout: `box exec <item> -- herdr pane list`, then `herdr pane layout --pane <id>`. For several panes, split inside the tape before toggling: `Type "herdr pane split --current --direction down >/dev/null"`, `Enter`.
- Position: write the config in the tape's hidden setup, before `herdr` (every record starts with no config). Use backticks so the quotes survive: ``Type `mkdir -p ~/.config/herdr-nvim && printf '[sidebar]\nposition = "left"\n' > ~/.config/herdr-nvim/config.toml` `` then `Enter`, start herdr, toggle.
- Persistence: in the sidebar `:e src/greet.js`, move the cursor (`Type "2Gw"`), toggle twice, and check the statusline still shows the same file and position.
- Per-tab nvim and cleanup: `Type "herdr tab create --cwd ~/demo --focus >/dev/null"` in the pane, toggle there, then `box exec <item> -- /opt/herdr-nvim/bin/herdr-nvim daemons` lists one nvim per tab. `herdr tab close <tab-id>` or `herdr workspace close <id>` through `box exec` removes that nvim.
- Restart: `box exec <item> -- herdr server stop`, then `sh -c 'cd ~/demo && (setsid nohup herdr server >/dev/null 2>&1 &)'`. Restored tabs keep their nvim. To see orphans reaped, delete `~/.config/herdr/session.json` before restarting.
- NVIM_APPNAME: write `~/.config/myapp/init.lua` (for example `vim.g.mapleader=" "`), set `nvim_env = ["NVIM_APPNAME=myapp"]` under `[sidebar]`, toggle, then `:echo $NVIM_APPNAME g:mapleader`.

## Gotchas

- Look at the first frame. nvim should draw at full size right away, with no shell prompt and no echoed command in the pane.
- A missing or malformed `config.toml` falls back to the defaults without an error. Invalid `nvim_env` entries are skipped; the warning is only in `herdr plugin log list`, never on screen.
- Stopping an nvim with unsaved edits (`daemons stop --force`) leaves a swap file. Within the same session (`--keep-session`) the next `:e` of that file stops at nvim's swap prompt and opens it `[RO]`. A fresh `box record` clears swap files.
- `box exec <item> -- ps aux | grep …` runs the grep on the host. Prefer `herdr-nvim daemons`, or wrap the pipe in `sh -c`.
- The box has no network, so plugin managers in an NVIM_APPNAME config cannot install anything.
