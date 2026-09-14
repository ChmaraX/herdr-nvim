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

T.test("prompt: format_ref is a bare citation ending in a space", function()
  local s = prompt.format_ref({ file = "/repo/lua/init.lua", start_line = 5, end_line = 10 }, { cwd = "/repo" })
  T.eq(s, "lua/init.lua:5-10 ")
  T.ok(not s:find("`", 1, true), "a ref carries no code")
  T.ok(not s:find("Comment:", 1, true), "a ref carries no comment")
end)

T.test("prompt: format_ref collapses a single line", function()
  T.eq(prompt.format_ref({ file = "/repo/a.rs", start_line = 7, end_line = 7 }, { cwd = "/repo" }), "a.rs:7 ")
end)

T.test("prompt: _relpath shortens against the cwd, keeps outside paths absolute", function()
  T.eq(prompt._relpath("/repo/lua/init.lua", "/repo"), "lua/init.lua")
  T.eq(prompt._relpath("/repo/lua/init.lua", "/repo/"), "lua/init.lua", "trailing slash tolerated")
  T.eq(prompt._relpath("/elsewhere/x.lua", "/repo"), "/elsewhere/x.lua", "outside the cwd stays absolute")
  T.eq(prompt._relpath("/repo-other/x.lua", "/repo"), "/repo-other/x.lua", "prefix must end at a separator")
  T.eq(prompt._relpath("/repo/x.lua", ""), "/repo/x.lua", "no cwd known → unchanged")
end)
