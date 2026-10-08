# Sidebar smoke test: herdr starts, prefix+e toggles the herdr-nvim sidebar.
# Run: box record <item> scripts/box/smoke/sidebar.tape
# Output paths are relative to the run dir (/proof, or /proof/<run> with --run).
# Waits match terminal text, but screenshots are taken from the drawn frame,
# which lags it: keep a short Sleep around each Screenshot.
Output sidebar.gif

# hidden: start herdr, cd ~/demo (setup is not part of the proof)
Source tapes/_start.tape
Show

# prefix (ctrl+b) then e: toggle the nvim sidebar
Ctrl+B
Type "e"
Wait+Screen /nvim sidebar/

# open a file in the sidebar's nvim so the screenshot shows real content
Type ":e src/greet.js"
Enter
Wait+Screen /export function greet/
Sleep 500ms
Screenshot sidebar.png
# let VHS flush the last screenshot before the tape ends
Sleep 1s
