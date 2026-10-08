# File picker

A popup that lists the files the agent touched. Typing searches the whole repo, and `⏎` opens the file in the sidebar.

## Sub-features

- default list: files the agent touched this session, newest first, with the cursor on the newest. Rows show a `new` badge for created files, green/red `+N -M` stats and an age (`now`, `2m`, `3h`). At most `picker.max_files` (20) rows. Sources are the agent's session, git changes (uncommitted, plus commits since the session started) and recent pane text. Files that were edited and then reverted rank lower.
- fuzzy search: typing searches every repo file (tracked plus untracked, respecting `.gitignore`) by path and name, not by contents. Multi-term and typo-tolerant. Agent-touched files rank first and matches are highlighted.
- keys: `↑/↓`, `Ctrl+P/N`, `PgUp/PgDn`, `Home/End`, `Ctrl+A/E`, `Ctrl+U` (clear), `Backspace`, `Esc`/`Ctrl+C` (close).
- open at line: `⏎` closes the popup and opens the file in this tab's sidebar, at the line if one is known, and focuses the sidebar. A closed sidebar is opened; an open one is never toggled shut.
- frecency: an existing fff.nvim history is reused for ranking. `picker.frecency=false` turns this off.

## How to get to it (user POV)

Press `prefix+o` from the agent pane or from any pane in the same tab, the sidebar included. The popup is titled "open file" and shows "N files" ("N matches" while searching) and the footer `↑↓ move · ⏎ open · ^U clear · esc close`.

## Driving it in the box

- Template: `smoke/agent.tape` with `smoke/agent.scenario.json`. pi edits `src/greet.js`, `Ctrl+B` `o` lists it, and `Enter` opens it in the sidebar. Wait for `/[1-9]\d* files/`, not the title: the frame is empty for a moment after "open file" appears, and `0 files` means the agent's file is missing.
- In a scenario, the `write` tool gives the `new` badge and `edit` gives `+N -M`. Put `delayMs` on the last turn to open the picker while pi is still working; the files touched so far are already listed.
- Search: with the picker open, `Type "main"`. The counter switches to "N matches". `Ctrl+U` clears. For a large repo use `cd /work` instead of `~/demo`.
- `Enter` with the sidebar closed opens it. `Ctrl+B` `o` from inside the sidebar, `Down`, `Enter` swaps the file in the open sidebar.
- No agent: `Ctrl+B` `o` in a plain shell. Nothing appears; check `box exec <item> -- herdr plugin log list` for the reason.

## Gotchas

- The workspace needs a pane that herdr detects as an agent. In a plain shell the key does nothing visible; the plugin log says "no agent panes found". An agent that has touched nothing opens the picker with "0 files", and typing still searches the repo.
- Each touched file can appear twice in the list (once plain, once with `+N -M`), and "N files" counts both. Treat it as a known defect, not a recipe failure.
- When the sidebar is already open but another pane has focus, `Enter` loads the file but focus stays where it was (herdr 0.9.3 refuses to focus a non-agent pane).
- Only pi exists in the box. Claude and agy sessions cannot be tested, and codex sessions are not read at all.
- The box has no fff.nvim history, so frecency cannot be tested unless you seed one.
