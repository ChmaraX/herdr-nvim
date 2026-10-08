# Core loop: pi edits a file -> prefix+o picker -> open in the sidebar ->
# comment on lines (\ac) -> submit to pi (\aS) -> pi's reply.
# Run: box up <item> <worktree> --scenario scripts/box/smoke/journey.scenario.json
#      box record <item> scripts/box/smoke/journey.tape
# Waits match terminal text, but screenshots are taken from the drawn frame,
# which lags it: keep a short Sleep around each Screenshot.
Output journey.gif

Source tapes/_start.tape
Show

# 1. pi edits src/greet.js (scenario turns 0 and 1)
Type "pi"
Enter
Wait+Screen /mock-1/
Type "Add a farewell function to src/greet.js"
Enter
Wait+Screen@20s /Done: src\/greet\.js/
Wait+Screen /○ demo/

# 2. prefix+o: the picker lists the touched file; Enter opens it in the sidebar
Ctrl+B
Type "o"
Wait+Screen /[1-9]\d* files/
Wait+Screen /esc close/
Sleep 1s
Screenshot journey-1-picker.png
Sleep 500ms
Enter
Wait+Screen /src\/greet\.js +\d+,\d+/
Sleep 1s

# 3. select the new function (lines 5-7) and comment on it with \ac
Type "5G"
Type "Vjj"
Type "\ac"
Wait+Screen /Comment:/
Type "Rename to goodbye"
Enter
Wait+Screen /💬 Rename to goodbye/
Sleep 500ms
Screenshot journey-2-comment.png
Sleep 500ms

# 4. \aS pastes the comments into pi's input and submits; pi answers with turns[2].
# First prove what pi received: the review prompt (file:lines and the comment)
# in pi's pane. The mock answers turn 2 to any request, so the reply alone
# proves nothing about the content. "sent" replaces nvim's "Comment: …" echo
# and clears the callout, so after it these texts can only be in pi's pane.
Type "\aS"
Wait+Screen@20s /sent 1 comment\(s\) to/
Wait+Screen /src\/greet\.js:5-7/
Wait+Screen /Comment: Rename to goodbye/
Wait+Screen /Please address each comment/
Wait+Screen@20s /Thanks for the review/
Wait+Screen /○ demo/
Sleep 500ms
Screenshot journey-3-reply.png
# let VHS flush the last screenshot before the tape ends
Sleep 1s
