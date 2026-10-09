# box: isolated test box for herdr-nvim

A box is one Docker container that runs the real stack, offline:

- **herdr**: the official Linux release binary.
- **herdr-nvim**: built from the worktree you pass in, then linked with `herdr plugin link`.
- **pi**: the coding agent, with herdr's pi integration installed. pi talks to a local **mock LLM** that replays a script. It costs nothing and gives the same result every run.
- **VHS**: types keys into a real terminal and records GIF/PNG proof.

Use it to reproduce a bug and to verify a fix.

**Isolation:** the box runs with `--network none` and holds no credentials. The worktree is mounted read-only at `/work`. You run git on the host. Several boxes can run at once; each has its own herdr server, its own cargo target volume and its own proof directory. Boxes carry the Docker label `hnv.box=1`; `box down`/`ls` only touch containers and volumes with that label.

**Code you did not write (external PRs) never runs on the host.** Build and test it only in the box: `box test <item>` (see below), and record tapes with `box record`. Do not run its `cargo test`, `cargo build` or lua tests on your machine.

## Commands

```sh
B=~/projects/herdr-nvim/scripts/box/box
$B build                                    # build image hnv-box:latest (~8 min cold)
$B up issue-42 <worktree> [--scenario f.json]  # start the box, build herdr-nvim, start herdr
$B record issue-42 my.tape --run before     # run a tape; outputs go to <proof>/before/
$B record issue-42 my.tape --run before --replace  # deliberately overwrite a run
$B test issue-42                            # cargo test + lua tests, inside the box
$B doctor issue-42                          # read-only health check for stale/broken boxes
$B scenario issue-42 other.json             # swap the mock LLM script
$B exec issue-42 -- herdr pane list         # run any command in the box
$B shell issue-42                           # interactive shell in the box
$B reset issue-42                           # fresh container (herdr/pi state, ~/demo); rebuilds herdr-nvim
$B logs issue-42                            # build, herdr, mock-LLM and vhs logs
$B ls                                       # list boxes
$B down issue-42                            # remove the box (proof is kept)
$B versions                                 # versions baked into the image
```

Proof goes to `~/.cache/hnv-box/<item>/` (set `HNV_BOX_PROOF_DIR` to use another root), which is `/proof` inside the box:

- `<run>/` for each `box record … --run <run>` (for example `before/` and `after/`), holding:
  - every output the tape produces (paths in the tape are relative to this dir)
  - `tapes/`: the tape as recorded, plus `_start.tape`
  - `.worktree-rev`: the worktree commit at record time, with `(dirty)` if it had uncommitted changes
  - `versions.txt`: herdr, pi, nvim and tool versions of the image
  - `scenario.json`: the mock LLM scenario used for the run (the default scenario if no custom one was set)
  - `git-diff-head.patch` and `untracked-files.txt` when `--allow-dirty` records dirty `before`/`after` proof
- Without `--run`, the same files land in the proof dir itself (a second record of the same tape overwrites the first).
- Existing run dirs are refused unless `--replace` is passed. Runs are transactional: a failed `--replace` leaves the previous run dir and stamp untouched.
- Proof runs named `before` or `after` require a clean worktree unless `--allow-dirty` is passed. Exploratory run names stay allowed.
- `.worktree-rev` and `versions.txt` at the top level: rewritten by `up`, `reset` and successful `record` runs.
- `logs/`: box-start, cargo build, herdr, mock LLM, `vhs-<run>-<tape>.log`, `test-*.log`
- `scenario.json`: the active scenario for the box; absent means the default scenario applies

After you edit the worktree on the host, run `box reset <item>`, or just `box record`. Both recreate the runtime container and rebuild herdr-nvim; `box reset` uses the same fresh-container path as a default recording. `box record --keep-session` keeps state but rebuilds when the worktree revision changed. Only the herdr-nvim crate is compiled (about 70 s with release LTO); dependencies are precompiled in the image.

## Health checks

Run `box doctor <item>` first whenever a box behaves strangely or proof might be stale. It prints one line per check and exits non-zero for failures:

- container exists and is running;
- `/work` is mounted and the host HEAD matches `<proof>/.worktree-rev` and `<proof>/.last-build-rev`;
- herdr server is running;
- herdr-nvim is linked from `/opt/herdr-nvim`;
- mock LLM `/health` answers;
- container disk has at least 2G free (warning only).

Each failing line includes the command to recover, usually `box reset <item>` or `box exec <item> -- box-start`.

## In-box verification helpers

These commands are on PATH inside the box. Call them from a tape (as the user's shell would), or with `box exec <item> -- …` after a `box record … --keep-session` run: herdr only has panes while a client session is attached, so right after `box up` there is nothing to click.

- Link click: `box-click-link --pane w1:p1 --text 'https://example.com/index.html'` (or `--row ROW --col COL`, 0-indexed viewport cells). With `--text` it clicks the last visible match, skipping its own command line, and fails unless herdr reports exactly that URL. It calls herdr's `pane.link.activate` API and prints compact JSON for `{url, handled}`, the plugin handler/exit code if one ran, and the clicked cell. `handled:false` with `plugin_handler:null` means herdr will use its default browser path.
- Agent input: `box-agent-input` prints the last prompt pi sent to the mock LLM from `/proof/logs/mock-llm.jsonl`.
- Claude stand-in: opt in with `box exec <item> -- box-claude-setup`, then run `claude` in a herdr pane. The stand-in uses herdr's real Claude hook and writes a scripted Claude transcript: a parent session reads `src/greet.js`, delegates to a sub-agent, and the sub-agent writes `~/notes/plan.md` and edits `~/notes/todo.md`. `box-claude-state` dumps the detected Claude session, transcript paths, touched files and picker handoff candidates.

## Running the tests

`box test <item> [cargo|lua]` runs `cargo test --locked --offline` and `nvim --headless --noplugin -u NONE -l tests/run.lua` inside the box. Logs go to `logs/test-cargo.log` and `logs/test-lua.log`; the exit code is non-zero if either fails.

It copies `/work` to a writable snapshot (`~/test-src` in the box, with a fresh one-commit git repo) and builds into `/cargo-target/test`. A plain `box exec <item> -- sh -c 'cd /work && cargo test --offline'` does not work: `/work` is read-only (some tests write under `target/`) and, for a git worktree, `/work/.git` points at a host path, so git-based tests fail. The lua tests alone also run straight from `/work`: `box exec <item> -- sh -c 'cd /work && nvim --headless --noplugin -u NONE -l tests/run.lua'`. The first `box test` compiles the debug dependencies (about 2 min); later runs are incremental.

## Writing a tape

A tape is a [VHS](https://github.com/charmbracelet/vhs) script.

- Paths in `Output`, `Screenshot` and `Source` are relative to the run directory (`<proof>/<run>/`, or the proof dir without `--run`).
- Every `box record` starts from a fresh runtime container with a fresh herdr session, fresh herdr-nvim state, no pi sessions, a clean `$HOME`, and a clean `~/demo`. Only the cargo target volume/build cache is kept. Pass `--keep-session` to continue from the previous state.
- herdr's prefix is `ctrl+b`. herdr-nvim's keys are bound as its README says: `prefix+e` toggles the sidebar, `prefix+o` opens the file picker.
- Start with `Source tapes/_start.tape` right after `Output`. It sets the size and font, then (hidden) starts herdr and runs `cd ~/demo` (a small git repo) in its pane; herdr's first workspace opens in `$HOME`. It ends hidden, so add `Show` after it (and after any extra hidden setup). `box record` copies `smoke/_start.tape` into `<run>/tapes/` next to your tape; a `_*.tape` next to your tape overrides it.
- Wait for screen text (`Wait+Screen /regex/`), not fixed sleeps. Keep a short `Sleep` before each `Screenshot` and at the end: the drawn frame lags the text.

```
Output repro.gif
Source tapes/_start.tape
Show
# prefix+e: toggle the nvim sidebar
Ctrl+B
Type "e"
Wait+Screen /nvim sidebar/
Sleep 500ms
Screenshot repro.png
Sleep 1s
```

Smoke tests in `smoke/` (box health check after a rebuild, and templates for new tapes):

- `_start.tape`: the shared start that every tape sources.
- `sidebar.tape`: toggles the sidebar.
- `agent.tape` with `agent.scenario.json`: pi edits a file, the picker shows it, and the file opens in the sidebar.
- `agent-choice.tape`: two agents in two tabs; sending comments picks this tab's agent without asking. The tape checks the prompt landed in tab 1's pi; its header has the `box exec` command that shows tab 2's pi got nothing.
- `journey.tape` with `journey.scenario.json`: the core loop. pi edits, picker, sidebar, comment, send to pi, pi replies. The tape checks the review prompt (`src/greet.js:5-7`, the comment, the closing line) in pi's pane before the reply.

Use video (GIF) when the bug takes several steps to show. Use a screenshot when one frame shows it.

## Writing a mock LLM scenario

```json
{
  "fallback": "text for any request past the last turn",
  "turns": [
    { "text": "I'll edit it.",
      "tools": [ { "name": "edit", "args": { "path": "src/greet.js",
                   "edits": [ { "oldText": "a", "newText": "b" } ] } } ] },
    { "text": "Done." }
  ]
}
```

- **Turn selection:** turn *N* answers the request that already holds *N* assistant messages. So the first reply to a prompt is `turns[0]`. After pi runs that turn's tool calls, the next reply is `turns[1]`. A new pi session starts again at turn 0.
- **Tools:** pi's built-in tools, with their real argument names:
  - `write {path, content}`
  - `edit {path, edits: [{oldText, newText}]}`
  - `read {path}`
  - `bash {command}`
- **Timing (optional, per turn):** `delayMs` pauses before the reply starts. `chunkMs` sets the streaming speed (default 25).
- The file is re-read on every request.
- Each request is logged to `logs/mock-llm.jsonl`: which turn answered it and the full last user message as `lastUser` (pi sends it as a list of content parts; the review prompt is the `text` part).

## Limits

- **Linux only.** macOS-only bugs (for example the terminal app, or macOS paths and keys) cannot be reproduced here.
- **No mouse.** VHS sends keys only, so a real Ctrl+click cannot be done. Link handlers are driven by invoking the action directly (see `.agents/skills/verify/features/links.md`).
- **The terminal is VHS's ttyd/xterm.js in headless Chromium.** It is not Ghostty, kitty or iTerm, so terminal-specific rendering can differ.
- **pi's replies are scripted.** You test herdr-nvim's reaction to agent activity, not model behaviour. Other agents (claude, codex) are not installed.
- **herdr and pi versions are fixed at `box build` time.** Rebuild to pick up new releases; only the last layers rebuild.
- **The worktree's `Cargo.lock` must use the dependencies the image precompiled.** If it adds a crate, `box up` fails with a clear message; rebuild with `box build --src <worktree>`.
