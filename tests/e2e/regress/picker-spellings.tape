# Regression: pi reads /home/box/demo/src/greet.js, edits ./src/greet.js and
# reads @README.md (pi drops the @). The picker must list greet.js once with
# its diff stat, plus README.md (main listed greet.js twice and lost README).
# Run: box scenario <item> tests/e2e/regress/picker-spellings.scenario.json
#      box record <item> tests/e2e/regress/picker-spellings.tape
Output picker-spellings.gif
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
Wait+Screen / 2 files/
Wait+Screen /greet\.js +\+4 -0/
Wait+Screen /README\.md/
Sleep 1s
Screenshot picker-spellings.png
Sleep 1s
