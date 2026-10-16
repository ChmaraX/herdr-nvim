# Regression: pi edits src/greet.js by a relative path. The picker must list
# it once, with git's diff stat (main listed it twice, one row without stat).
# Run: box scenario <item> tests/e2e/regress/picker-relative.scenario.json
#      box record <item> tests/e2e/regress/picker-relative.tape
Output picker-relative.gif
Source tapes/_start.tape
Show
Type "pi"
Enter
Wait+Screen /mock-1/
Type "Add a farewell function to src/greet.js"
Enter
Wait+Screen@30s /Done: /
Wait+Screen /○ demo/
Ctrl+B
Type "o"
Wait+Screen /esc close/
Wait+Screen / 1 files/
Wait+Screen /\+4 -0/
Sleep 1s
Screenshot picker-relative.png
Sleep 1s
