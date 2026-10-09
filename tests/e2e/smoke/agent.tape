# Agent flow: pi (on the mock LLM) edits a file, then herdr-nvim's picker
# (prefix+o) shows the touched file and opens it in the nvim sidebar.
# Run: box up <item> <worktree> --scenario tests/e2e/smoke/agent.scenario.json
#      box record <item> tests/e2e/smoke/agent.tape
# Waits match terminal text, but screenshots are taken from the drawn frame,
# which lags it: keep a short Sleep around each Screenshot.
Output agent.gif

Source tapes/_start.tape
Show

# start pi in the herdr pane and give it a prompt
Type "pi"
Enter
Wait+Screen /mock-1/
Type "Add a farewell function to src/greet.js"
Enter
# turns[1] of the scenario: pi's final reply after the edit
Wait+Screen@20s /Done: src\/greet\.js/
# herdr's agent dot turns from ● (working) to ○ (idle) when pi finishes
Wait+Screen /○ demo/
Sleep 500ms
Screenshot agent-pi.png
Sleep 500ms

# prefix+o: herdr-nvim file picker, newest agent-touched file first
Ctrl+B
Type "o"
Wait+Screen /[1-9]\d* files/
Wait+Screen /esc close/
Sleep 1s
Screenshot agent-picker.png
Sleep 500ms

# open it in the nvim sidebar (wait for nvim's own statusline)
Enter
Wait+Screen /nvim sidebar/
Wait+Screen /src\/greet\.js +\d+,\d+/
# no stable text marks the end of the resize redraw
Sleep 1s
Screenshot agent-sidebar.png
# let VHS flush the last screenshot before the tape ends
Sleep 1s
