local dispatch = require("herdr-nvim.dispatch")

local function recorder(fail_on)
  local calls = {}
  return calls, function(argv)
    table.insert(calls, argv)
    if fail_on and #calls == fail_on then return { code = 1, stdout = "", stderr = "boom" } end
    return { code = 0, stdout = "", stderr = "" }
  end
end

T.test("dispatch: paste mode sends text only, no Return", function()
  local calls, exec = recorder()
  local ok = dispatch.send("wA:p1", "line1\nline2", { submit = false }, exec)
  T.ok(ok)
  T.eq(#calls, 1)
  T.eq(calls[1], { "herdr", "agent", "send", "wA:p1", "line1\nline2" })
end)

T.test("dispatch: auto-send appends Return keypress", function()
  local calls, exec = recorder()
  local ok = dispatch.send("wA:p1", "hi", { submit = true }, exec)
  T.ok(ok)
  T.eq(#calls, 2)
  T.eq(calls[2], { "herdr", "pane", "send-keys", "wA:p1", "Return" })
end)

T.test("dispatch: failed send returns err and skips Return", function()
  local calls, exec = recorder(1)
  local ok, err = dispatch.send("wA:p1", "hi", { submit = true }, exec)
  T.eq(ok, false)
  T.ok(err:match("boom"))
  T.eq(#calls, 1)
end)
