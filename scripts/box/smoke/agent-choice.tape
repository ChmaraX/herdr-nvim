# Run: box record <item> scripts/box/smoke/agent-choice.tape (no scenario needed)
# Two agents in the workspace, one per tab: \as from tab 1 must send to
# tab 1's pi without asking ("Send to agent" list must NOT appear).
# Target check: the prompt must show up in tab 1's pi input (on screen here).
# After the tape, prove tab 2's pi did NOT get it (expect t1 = 1, t2 = 0):
#   box exec <item> -- sh -c 'for t in w1:t1 w1:t2; do p=$(herdr pane list | jq -r ".result.panes[] | select(.tab_id==\"$t\" and .agent==\"pi\") | .pane_id"); echo "$t $(herdr pane read $p --source visible | grep -c greet.js:2-3)"; done'
Output agent-choice.gif
Source tapes/_start.tape

# second agent in a new, unfocused tab; then pi in this tab
Type "P=$(herdr tab create --cwd ~/demo --no-focus | jq -r .result.root_pane.pane_id) && herdr pane run $P pi >/dev/null && clear && pi"
Enter
Wait+Screen /mock-1/
Show

Ctrl+B
Type "e"
Wait+Screen /nvim sidebar/
Sleep 1s
Type ":e src/greet.js"
Enter
Wait+Screen /src\/greet\.js/
Type "2GVj\ac"
Wait+Screen /Comment:/
Type "Check this"
Enter
Wait+Screen /💬 Check this/
Type "\as"
Wait+Screen@15s /sent 1 comment\(s\) to/
# \as pastes without submitting: the prompt sits in this tab's pi input.
# "sent" replaced nvim's "Comment: …" echo, so these match pi's pane only.
Wait+Screen /src\/greet\.js:2-3/
Wait+Screen /Comment: Check this/
Sleep 1s
Screenshot agent-choice.png
Sleep 1s
