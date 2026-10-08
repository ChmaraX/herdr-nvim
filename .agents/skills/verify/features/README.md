# herdr-nvim feature map

This is a behavior-level list of what herdr-nvim does for a user: an nvim sidebar in herdr, a file picker for files the agent touched, Ctrl+click file links, and review comments sent from nvim to an agent. Agents use it to decide what to drive in the box and what counts as proof. Humans use it as a regression checklist. Keep entries short enough to run without reading source.

## Baseline

- Drive the real app in the box. Commands and tape writing are in [scripts/box/README.md](../../../../scripts/box/README.md). Templates live in `scripts/box/smoke/`.
- Every `box record` starts fresh: a new herdr session, no herdr-nvim state or config, no nvim swap files, no pi sessions, and a clean `~/demo` (a small git repo with `src/greet.js` and `src/main.js`).
- herdr's prefix is `ctrl+b`. `prefix+e` toggles the sidebar and `prefix+o` opens the file picker.
- If an agent is needed, start `pi` in the pane. It answers from a mock LLM scenario (`box up … --scenario s.json`, templates `smoke/*.scenario.json`). No other agent is installed.
- The sidebar nvim is vanilla, so `<leader>` is `\`.
- Box limits (Linux only, no mouse, xterm.js, fixed versions) are in the box README's [Limits](../../../../scripts/box/README.md#limits). If a path hits one, name the gap and cover the closest real path.
- Tape mechanics (`Source tapes/_start.tape`, waits, screenshots, final sleeps) are canonical in [scripts/box/README.md#writing-a-tape](../../../../scripts/box/README.md#writing-a-tape).

## Sidebar

- [sidebar](sidebar.md): Toggle, position, one hidden nvim per tab, auto-cleanup, choice of nvim binary and config.

## Files

- [picker](picker.md): Agent-touched file list, repo-wide fuzzy search, open in the sidebar at the line.
- [links](links.md): Ctrl+click on file paths and `file://` links opens them in the sidebar.

## Annotations

- [annotations](annotations.md): Comment on lines, list comments, send them to an agent, `:Herdr ref`, statusline, `:Herdr` command and keymaps.

## Journeys

- [journeys](journeys.md): The review loop across features: agent edit, picker, comment, submit, agent reply.

## Ops

- [ops](ops.md): `herdr-nvim doctor`, `herdr-nvim daemons`, install and build.

## Entry contract

Every area file uses the same four H2s: `Sub-features`, `How to get to it (user POV)`, `Driving it in the box`, `Gotchas`.
