# Run: box record <item> scripts/box/smoke/agent-choice.tape (no scenario needed)
# Two agents in the workspace, one per tab: \as from tab 1 must send to
# tab 1's pi without asking ("Send to agent" list must NOT appear).
# Target check: the prompt must show up in tab 1's pi input (on screen here),
# then the tape itself counts both tabs and waits for t1=1, t2=0.
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

# Count both pi panes in the recording; if the counts are wrong, the final
# Wait+Screen times out and the run fails.
Escape
Type ':!j=$(herdr pane list); p1=$(echo "$j" | jq -r ".result.panes[] | select(.tab_id==\"w1:t1\" and .agent==\"pi\") | .pane_id"); p2=$(echo "$j" | jq -r ".result.panes[] | select(.tab_id==\"w1:t2\" and .agent==\"pi\") | .pane_id"); c1=$(herdr pane read $p1 --source visible | grep -c src/greet[.]js:2-3); c2=$(herdr pane read $p2 --source visible | grep -c src/greet[.]js:2-3); echo agent-choice-counts t1=$c1 t2=$c2; [ "$c1:$c2" = 1:0 ] && echo agent-choice-counts-ok'
Enter
Wait+Screen /agent-choice-counts t1=1 t2=0/
Wait+Screen /agent-choice-counts-ok/
Sleep 1s
