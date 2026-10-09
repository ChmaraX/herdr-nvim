# Journeys

End-to-end loops that cross several features. Run one after any change that touches more than one area.

## Sub-features

- review loop: pi edits a file → `prefix+o` lists it → `⏎` opens it in the sidebar → `<leader>ac` comments on the changed lines → `<leader>aS` submits the comments to pi → pi answers. Afterwards the comment rail is gone and nvim shows "sent 1 comment(s) to …".

## How to get to it (user POV)

Ask the agent for a change, press `prefix+o`, open the file it touched, select the lines you want changed, comment, and submit. The agent gets one prompt with `path:line-range`, the code and your comment, and replies in its pane.

## Driving it in the box

- `box up <item> <worktree> --scenario tests/e2e/smoke/journey.scenario.json`, then `box record <item> tests/e2e/smoke/journey.tape`. On a running box, `box scenario <item> tests/e2e/smoke/journey.scenario.json` first.
- The scenario has three turns: 0 edits `src/greet.js` (adds `farewell` on lines 5-7), 1 says "Done: …", 2 is the reply to the review ("Thanks for the review…").
- Proof: `journey-1-picker.png` (the picker lists `src/greet.js`), `journey-2-comment.png` (callout "💬 Rename to goodbye" over lines 5-7), `journey-3-reply.png` (the review prompt and pi's reply in pi's pane).
- The tape itself checks pi's input: after `\aS` it waits for `src/greet.js:5-7`, `Comment: Rename to goodbye` and "Please address each comment" in pi's pane before the reply. The mock answers turn 2 to any third request, so the reply alone proves nothing about the content.
- `logs/mock-llm.jsonl` shows the request answered by turn 2, with the full review prompt in `lastUser` (a list of content parts; the prompt is the `text` part, not truncated).

## Gotchas

- Wait for pi to go idle (`Wait+Screen /○ demo/`) before `prefix+o`. Its "Done" text appears while pi still shows Working.
- Keep the scenario turns in step with the tape. An extra request moves every later reply to the next turn, and past the last turn pi answers with the `fallback` text.
- The picker lists `src/greet.js` twice (see [picker](picker.md)). `Enter` on the first row is fine.
