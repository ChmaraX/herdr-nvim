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

local function run_helper(op, ctx)
  local system, stdout = vim.system, io.stdout
  local calls, output = {}, ""
  vim.system = function(argv, opts)
    if argv[1] == "git" then
      calls[#calls+1] = "git"
      return { wait = function() return { code = 1, stderr = "not a repository" } end }
    end
    T.eq(argv, { "hc", "connect" }) -- info and invalid opens must not contact Neovim
    local request = vim.json.decode(opts.stdin:match("[^\n]+\n([^\n]+)"))
    calls[#calls+1] = request.method
    local value = request.method == "doc.get"
      and { version = 1, value = { spaces = { { id = "s1", tabs = { { id = "t1" } } } } } }
      or { terminals = {} }
    return { wait = function() return { code = 0, stdout = vim.json.encode({ id = 2, result = value }) .. "\n" } end }
  end
  io.stdout = { write = function(_, text) output = output .. text end }
  local ok, err = pcall(remote.main, { [0] = "/tmp/worker.lua", op, vim.json.encode(ctx) })
  vim.system, io.stdout = system, stdout
  return ok, err, output, calls
end

T.test("remote: pane inspection reads one document and never probes the editor", function()
  local ok, err, output, calls = run_helper("info", { space = "s1", tab = "t1", key = "test", cwd = "/deleted/folder" })
  T.ok(ok, err)
  T.eq(calls, { "doc.get", "terminal.list" })
  T.eq(vim.json.decode(output).doc.version, 1)
end)

T.test("remote: invalid files fail before starting or probing a daemon", function()
  local ok, err, _, calls = run_helper("open", {
    space = "s1", tab = "t1", key = "test", cwd = vim.uv.os_tmpdir(),
    file = vim.uv.os_tmpdir() .. "/hnvim-missing-" .. vim.uv.hrtime(),
  })
  T.ok(not ok and tostring(err):find("No remote file", 1, true))
  T.eq(calls, { "doc.get", "git" })
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
