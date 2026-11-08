local remote = require("herdr-nvim.remote")

T.test("remote: nvim -l arguments decode without forwarding assert's message", function()
  local op, ctx, script = remote.parse_arguments({ [0] = "/tmp/worker.lua", "info", '{"space":"s1","tab":"t1"}' })
  T.eq(op, "info"); T.eq(ctx, { space = "s1", tab = "t1" }); T.eq(script, "/tmp/worker.lua")
end)

T.test("remote: documents are validated before deciding a tab was closed", function()
  local unknown = {
    {}, { version = 0 }, { version = 1, value = {} },
    { version = "1", value = { spaces = {} } },
    { version = 1, value = { spaces = { { id = "s1" } } } },
    { version = 1, value = { spaces = { { id = "s1", tabs = { {} } } } } },
  }
  T.eq(remote.find_tab(nil, "s1", "t1"), nil)
  for _, doc in ipairs(unknown) do T.eq(remote.find_tab(doc, "s1", "t1"), nil) end
  T.eq(remote.find_tab({ version = 1, value = { spaces = {} } }, "s1", "t1"), false)
end)

T.test("remote: tabs are matched inside their owning space", function()
  local tab = { id = "t1", root = { id = "p1" } }
  local doc = { version = 2, value = { spaces = { { id = "s1", tabs = { tab } } } } }
  T.eq(remote.find_tab(doc, "s1", "t1"), tab)
  T.eq(remote.find_tab(doc, "s2", "t1"), false)
  T.eq(remote.find_tab(doc, "s1", "t2"), false)
end)

T.test("remote: stale sockets are reaped only for a known dead owner", function()
  T.ok(remote.dead_owner({ pid = 123 }, function(pid) T.eq(pid, 123); return false end))
  T.ok(not remote.dead_owner({ pid = 123 }, function() return true end))
  for _, meta in ipairs({ {}, { pid = 0 }, { pid = "123" } }) do
    T.ok(not remote.dead_owner(meta, function() error("invalid metadata must not probe a PID") end))
  end
  T.ok(not remote.dead_owner(nil, function() return false end))
end)

T.test("remote: daemon startup preserves the user's configuration", function()
  if vim.fn.has("win32") == 1 then return end
  local argv = remote.daemon_argv({ nvim = "/remote/bin/nvim", key = "test-startup" })
  T.eq(argv[1], "/remote/bin/nvim")
  T.eq(argv[2], "--headless")
  T.eq(argv[3], "--listen")
  T.eq(#argv, 4)
  T.ok(argv[4]:match("/test%-startup%.sock$"))
end)
