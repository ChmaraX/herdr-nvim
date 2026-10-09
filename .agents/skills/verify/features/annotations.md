# Annotations

Review comments on code lines in nvim, sent to an agent as one prompt, plus bare `file:line` references. Works in the sidebar with no install.

## Sub-features

- comment: `<leader>ac` comments the current line (normal mode) or the selection (visual mode). A charwise `v` selection inside one line stores only that selected span and sends it as `path:line:c1-c2` (1-indexed byte columns, inclusive). Multi-line charwise spans send `path:Ls-Le`; linewise `V`, blockwise visual and whole-line charwise selections stay whole-line. `:Herdr comment` and `comment_range(s,e)` are linewise. A `Comment: ` prompt appears. The block gets an amber sign-column rail, a tinted background and a callout above it. Empty input or Esc adds nothing. Comments follow edits and are kept in memory, per nvim instance.
- list: `<leader>al` / `:Herdr list` opens a float. Moving the cursor previews each comment in the code window. `⏎` edits, `d` deletes, `q`/`Esc` closes.
- send: `<leader>as` / `:Herdr send` pastes the comments into the agent's input without submitting. `<leader>aS` / `:Herdr submit` pastes and submits. The prompt starts with "Code review comments from my editor (repo: X, branch: Y):", lists each entry as `path:line[-end]` or `path:line:c1-c2` relative to the agent's cwd, up to 3 snippet lines or the selected span and the comment, and ends with "Please address each comment. Reply with what you changed per item." On success the notify reads "sent N comment(s) to …" and the comments are cleared (`clear_after_send`). On failure they are kept.
- agent choice: the agent is picked automatically if it is the only one in the workspace or the only one in this tab. Otherwise a numbered "Send to agent" list appears (`1: pi · idle · demo`). A busy agent gets the WARN "is working — sending anyway".
- ref: `<leader>ai` / `:Herdr ref` inserts `path:12` or `path:12-20 ` into the agent's input, mid-sentence. It never submits, sends no code and leaves comments alone. It warns on unsaved changes ("the agent reads the file on disk") and refuses a buffer with no name ("buffer has no file to reference").
- statusline: `require("herdr-nvim").statusline()` returns `● N` while N comments are pending, and an empty string otherwise.
- command: `:Herdr {comment,list,send,submit,ref}` completes with Tab. An unknown subcommand gives an error that lists the valid ones. `setup{prefix, keymaps, clear_after_send}` controls the keymaps, which never override a map the user already set (a WARN appears instead).

## How to get to it (user POV)

Open a file in the sidebar, or in the user's own nvim with the plugin installed. Comment on lines, then send them to an agent running in a herdr pane of the same workspace.

## Driving it in the box

- Full loop (edit, picker, comment, `\aS`, pi's reply): `smoke/journey.tape` with `smoke/journey.scenario.json`. See [journeys](journeys.md).
- Setup: start `pi` in `~/demo`, `Wait+Screen /mock-1/`, then `Ctrl+B` `e` and `:e src/greet.js`. pi needs no prompt for send or ref.
- VHS passes `\` through as-is, so `Type "\ac"` sends `<leader>ac`.
- Comment: `Type "2GVj\ac"`, `Wait+Screen /Comment:/`, type the text, `Enter`, then `Wait+Screen /💬 <text>/`.
- List: `Type "\al"`, `Wait+Screen /Comments/`. Then `j`, `Enter` (prompt "Edit comment:"), `d`, `q`. After `Esc` at the edit prompt, `q` the list before `\al` again, or two lists stack.
- Send: `\as`, then screenshot pi's input box. With `\aS`, pi answers with the next scenario turn. To inspect exactly what pi received, run `box exec <item> -- box-agent-input`; it prints the last prompt from `/proof/logs/mock-llm.jsonl`.
- Agent list: `Type "herdr pane split --current --direction down --no-focus >/dev/null && herdr pane run w1:p2 pi >/dev/null && pi"`. `\as` then shows a numbered "Send to agent" prompt; `Type "1"`, `Enter`. A scenario turn with a long `delayMs` keeps one pi busy for the WARN.
- One agent per tab (2 agents in the workspace): `smoke/agent-choice.tape`. A second pi in a new unfocused tab, then `\as` from tab 1 sends to tab 1's pi with no list. The tape waits for `src/greet.js:2-3` and `Comment: Check this` in tab 1's pi input (after "sent", so nvim's own echo is gone). Then prove tab 2's pi got nothing; this prints `w1:t1 1` and `w1:t2 0`:
  `box exec <item> -- sh -c 'for t in w1:t1 w1:t2; do p=$(herdr pane list | jq -r ".result.panes[] | select(.tab_id==\"$t\" and .agent==\"pi\") | .pane_id"); echo "$t $(herdr pane read $p --source visible | grep -c greet.js:2-3)"; done'`
- Ref: `3G\ai`, then screenshot pi's input box. Statusline: `:lua print(require('herdr-nvim').statusline())`. Command: `:Herdr `, then `Tab`. Errors and WARNs end in a hit-enter prompt; press `Enter` before the next key.

## Gotchas

- In the box sidebar, `<leader>` is `\`. The statusline is not shown by default.
- The callout of a comment on line 1 sits above the first line, so it is off-screen; only the rail shows.
- The agent list is vanilla nvim's `inputlist`: type a number, `Enter`. An empty `Enter` cancels.
- Sending needs the `herdr` CLI on PATH inside nvim and an agent in the same workspace.
- Comments belong to one nvim. They survive a sidebar toggle but are lost when that tab's nvim is stopped.
