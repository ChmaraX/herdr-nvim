local prompt = require("herdr-nvim.prompt")

T.test("prompt: single comment, no git context", function()
  local s = prompt.format({
    { comment = { file = "/tmp/x.py", start_line = 5, end_line = 5, text = "rename to double" },
      snippet = { "def f(x): return x*2" } },
  }, {})
  local expected = table.concat({
    "Code review comments from my editor:",
    "",
    "1. /tmp/x.py:5-5",
    "   > def f(x): return x*2",
    "   Comment: rename to double",
    "",
    "Please address each comment. Reply with what you changed per item.",
  }, "\n")
  T.eq(s, expected)
end)

T.test("prompt: multiple comments numbered, snippet capped at 3 lines, header context", function()
  local s = prompt.format({
    { comment = { file = "a.rs", start_line = 1, end_line = 9, text = "c1" },
      snippet = { "l1", "l2", "l3", "l4", "l5" } },
    { comment = { file = "b.rs", start_line = 2, end_line = 3, text = "c2" },
      snippet = { "x", "y" } },
  }, { header_context = "repo: demo, branch: main" })
  T.ok(s:find("Code review comments from my editor (repo: demo, branch: main):", 1, true) == 1)
  T.ok(s:find("1. a.rs:1-9", 1, true))
  T.ok(s:find("   > l3", 1, true))
  T.ok(not s:find("> l4", 1, true), "snippet must cap at 3 lines")
  T.ok(s:find("2. b.rs:2-3", 1, true))
  T.ok(s:find("   Comment: c2", 1, true))
end)
