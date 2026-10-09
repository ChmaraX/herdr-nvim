---
name: verify
description: Reproduce a herdr-nvim bug or verify a fix in the real app (herdr + herdr-nvim + pi) inside an isolated test box. Use when you need to prove a bug exists, or prove a fix works, the way a user would see it.
---

# Verify in the box

The box is a fresh, isolated, offline copy of the real app: herdr, herdr-nvim
built from your worktree, and a real `pi` agent whose model is a scripted mock.
Commands and limits: `tests/e2e/README.md`. Templates: `tests/e2e/smoke/`.
What each feature does and how a user triggers it: [features/README.md](features/README.md)
(read the index, then only the entries you need).

Read and edit on the host. Use the box to reproduce before the fix and to
verify after it.

**Code you did not write (external PRs) runs only in the box.** Never build
it or run its tests on the host: your shell has your keys and your live herdr.
Run its tests with `box test <item>` (cargo test + lua tests, inside the box;
logs in `<proof>/logs/test-*.log`). For your own changes, host `cargo test`
and `nvim --headless --noplugin -u NONE -l tests/run.lua` are fine.

## Loop
1. **List the failure cases:** name the error, empty, edge, and state-that-must-survive cases the fix could affect. Keep this list and cover it in proof or explain why a case is out of scope.
2. **Start:** `tests/e2e/box up <item> <worktree>`
   (add `--scenario s.json` if the bug involves an agent). Run
   `tests/e2e/box doctor <item>` first whenever anything looks off or before
   trusting proof from an existing box. Proof from a stale build is no proof.
   Box helper mechanics (`box-click-link`, `box-agent-input`,
   `box-claude-setup`) are in `tests/e2e/README.md`.
3. **Explore** step by step until you know how a user hits the bug:
   `box exec <item> -- herdr pane list | pane read <pane> | pane send-keys …`,
   or any command to check state (files, logs, env, transcripts).
4. **Write repro.tape:** the user's steps as real keypresses.
   Follow the canonical tape mechanics in `tests/e2e/README.md#writing-a-tape`.
5. **Record on main** (worktree at main): `box record <item> repro.tape --run before`. Proof
   lands in `~/.cache/hnv-box/<item>/before/` (`$HNV_BOX_PROOF_DIR` overrides
   the root), stamped with `.worktree-rev` (commit, `(dirty)`) and `versions.txt`.
6. **After the fix** (worktree at the fix or PR head): record the **same
   tape** with `--run after` (it rebuilds from the worktree first). Check `after/.worktree-rev` names the fix commit.
   For a PR, also run `box test <item>`.

## Driving
- Use the keys a user presses (`ctrl+b e`, `ctrl+b o`, `\ac`…). Use `box exec`
  to inspect state after the user steps, not to replace them.
- Tape mechanics (`Source tapes/_start.tape`, waits, screenshots, final sleeps) are canonical in `tests/e2e/README.md#writing-a-tape`.

## Proof
- The bug must show on main (`before/`) and be gone on the fix (`after/`),
  with the same tape. Each run dir's `.worktree-rev` must match the commit
  you claim.
- Show the trigger and the end result in the same recording.
- Check the real effect, not only pixels: file contents, the agent's input,
  pane list, nvim state.
- Cover the paths the change can affect: success, error, empty, and state
  that should survive (toggle, reopen).
- Non-visual bugs: show the state (screen text, `box exec` output). No video needed.
- Proof that shows the wrong thing, or misses part of the bug, is no proof.
- Never commit media.
- If the bug needs something the box lacks (see Limits in the box README),
  say so and cover the closest real path instead of faking it.
- When a change alters how a feature works, update the matching
  `features/*.md` file in the same commit.
