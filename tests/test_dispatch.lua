local dispatch = require("herdr-nvim.dispatch")

local function recorder(fail_on)
  local calls = {}
  return calls, function(argv)
    table.insert(calls, argv)
    if fail_on and #calls == fail_on then return { code = 1, stdout = "", stderr = "boom" } end
    return { code = 0, stdout = "", stderr = "" }
  end
end

T.test("dispatch: paste mode sends text only, no Enter", function()
  local calls, exec = recorder()
  local ok = dispatch.send("wA:p1", "line1\nline2", { submit = false }, exec)
  T.ok(ok)
  T.eq(#calls, 1)
  T.eq(calls[1], { "herdr", "pane", "send-text", "wA:p1", "line1\nline2" })
end)

T.test("dispatch: send mode uses agent prompt (auto-submits)", function()
  local calls, exec = recorder()
  local ok = dispatch.send("wA:p1", "hi", { submit = true }, exec)
  T.ok(ok)
  T.eq(#calls, 1)
  T.eq(calls[1], { "herdr", "agent", "prompt", "wA:p1", "hi" })
end)

T.test("dispatch: failed agent prompt returns err", function()
  local calls, exec = recorder(1)
  local ok, err = dispatch.send("wA:p1", "hi", { submit = true }, exec)
  T.eq(ok, false)
  T.ok(err:match("boom"))
  T.eq(#calls, 1)
end)

T.test("dispatch: failed pane send-text returns err", function()
  local calls, exec = recorder(1)
  local ok, err = dispatch.send("wA:p1", "hi", { submit = false }, exec)
  T.eq(ok, false)
  T.ok(err:match("boom"))
  T.eq(#calls, 1)
end)
