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

-- The Herdr app host: the daemon has HERDR_NVIM_HOST=app and the wrapper's
-- path in HERDR_NVIM_SELF; both modes go through `herdr-nvim app-send`.
local function as_app(fn)
  local host, self = vim.env.HERDR_NVIM_HOST, vim.env.HERDR_NVIM_SELF
  vim.env.HERDR_NVIM_HOST, vim.env.HERDR_NVIM_SELF = "app", "/opt/herdr-nvim/bin/herdr-nvim"
  local ok, err = pcall(fn)
  vim.env.HERDR_NVIM_HOST, vim.env.HERDR_NVIM_SELF = host, self
  if not ok then error(err, 0) end
end

T.test("dispatch (app): submit sends through app-send --submit", function()
  as_app(function()
    local calls, exec = recorder()
    T.ok(dispatch.send("t5", "hi", { submit = true }, exec))
    T.eq(calls[1], { "/opt/herdr-nvim/bin/herdr-nvim", "app-send", "t5", "hi", "--submit" })
  end)
end)

T.test("dispatch (app): paste mode never submits", function()
  as_app(function()
    local calls, exec = recorder()
    T.ok(dispatch.send("t5", "line1\nline2", { submit = false }, exec))
    T.eq(calls[1], { "/opt/herdr-nvim/bin/herdr-nvim", "app-send", "t5", "line1\nline2" })
  end)
end)

T.test("dispatch (app): a failed send names the app command", function()
  as_app(function()
    local _, exec = recorder(1)
    local ok, err = dispatch.send("t5", "hi", { submit = true }, exec)
    T.eq(ok, false)
    T.ok(err:match("app%-send") and err:match("boom"))
  end)
end)
