# Shared start for every tape: settings, then (hidden) start herdr and cd
# into the demo repo in its pane (herdr's first workspace opens in $HOME).
# Use it right after the tape's Output line:  Source tapes/_start.tape
# VHS inlines it at parse time. The path is relative to VHS's cwd, the run
# dir; `box record` copies this file into <run dir>/tapes/ next to the tape.
# Ends still hidden: add any extra hidden setup, then Show.
Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 800
Set TypingSpeed 40ms
Set Padding 10

Hide
Type "herdr"
Enter
Wait+Screen /spaces/
Type "cd ~/demo && clear"
Enter
Wait+Screen /~\/demo \$/
