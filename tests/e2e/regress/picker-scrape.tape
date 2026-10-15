# Regression: pi edits src/greet.js with bash sed and prints src/greet.js:2.
# No read/edit tool call, so the picker falls back to scraping the pane. The
# mention and git's dirty row are one file: one row that opens at line 2
# (main listed it twice and the top row opened at line 1).
# Run: box scenario <item> tests/e2e/regress/picker-scrape.scenario.json
#      box record <item> tests/e2e/regress/picker-scrape.tape
Output picker-scrape.gif
Source tapes/_start.tape
Show
Type "pi"
Enter
Wait+Screen /mock-1/
Type "Change the greeting to Hi"
Enter
Wait+Screen@30s /Done: /
Wait+Screen /○ demo/
Ctrl+B
Type "o"
Wait+Screen /esc close/
Wait+Screen / 1 files/
Sleep 1s
Screenshot picker-scrape.png
Enter
Wait+Screen /nvim sidebar/
Wait+Screen /src\/greet\.js +2,\d+/
Sleep 1s
Screenshot picker-scrape-opened.png
Sleep 1s
