-- Runs on the tab's machine via `nvim --clean -l`. Only the editor daemon
-- loads the user's configuration; helper and UI clients stay clean.
local M = {}
local uv = vim.uv or vim.loop

local function run(argv, options)
  local r = vim.system(argv, vim.tbl_extend("force", { text = true, timeout = 10000 }, options or {})):wait()
  if r.code ~= 0 then error(vim.trim(r.stderr or r.stdout or "command failed")) end
  return r.stdout or ""
end

local function core(method, params)
  local input = vim.json.encode({ jsonrpc = "2.0", id = 1, method = "hello", params = vim.empty_dict() }) .. "\n"
    .. vim.json.encode({ jsonrpc = "2.0", id = 2, method = method, params = params }) .. "\n"
  local text = run({ "hc", "connect" }, { stdin = input })
  for line in text:gmatch("[^\n]+") do
    local reply = vim.json.decode(line)
    if reply.id == 2 then
      if reply.error then error(reply.error.message) end
      return reply.result
    end
  end
  error("herdr-core gave no reply to " .. method)
end

-- An unavailable or unfamiliar document is not proof that a tab was closed.
function M.find_tab(doc, space, tab)
  if type(doc) ~= "table" or type(doc.version) ~= "number" or doc.version < 1 then return nil end
  local value = doc.value
  if type(value) ~= "table" or type(value.spaces) ~= "table" then return nil end
  for _, s in ipairs(value.spaces) do
    if type(s) ~= "table" or type(s.id) ~= "string" or type(s.tabs) ~= "table" then return nil end
    for _, t in ipairs(s.tabs) do
      if type(t) ~= "table" or type(t.id) ~= "string" then return nil end
    end
  end
  for _, s in ipairs(value.spaces) do
    for _, t in ipairs(s.tabs) do
      if s.id == space and t.id == tab then return t end
    end
  end
  return false
end

local function read(path)
  local f = io.open(path, "rb")
  if not f then return nil end
  local data = f:read("*a"); f:close()
  return data
end

local function write(path, value)
  local f = assert(io.open(path .. ".tmp", "wb"))
  f:write(vim.json.encode(value)); f:close()
  assert(uv.fs_rename(path .. ".tmp", path))
end

local function paths(ctx)
  assert(uv.os_uname().sysname ~= "Windows_NT", "Remote Neovim currently requires a POSIX host")
  local root = uv.os_tmpdir() .. "/herdr-nvim-remote-" .. uv.getuid()
  local stat = uv.fs_lstat(root)
  if not stat then uv.fs_mkdir(root, 448); stat = uv.fs_lstat(root) end
  assert(stat and stat.type == "directory" and stat.uid == uv.getuid(), "Unsafe remote runtime directory")
  assert(uv.fs_chmod(root, 448)) -- 0700
  return root .. "/" .. ctx.key
end

local function alive(pid)
  return pid and uv.kill(pid, 0) ~= nil
end

local function metadata(base)
  local text = read(base .. ".json")
  return text and vim.json.decode(text) or nil
end

local function evaluate(ctx, lua)
  local expr = "luaeval(" .. vim.json.encode(lua) .. ")"
  return vim.json.decode(vim.trim(run({ ctx.nvim, "--clean", "--headless", "--server", paths(ctx) .. ".sock", "--remote-expr", expr })))
end

function M.dead_owner(meta, is_alive)
  return type(meta) == "table" and type(meta.pid) == "number" and meta.pid > 0 and not is_alive(meta.pid)
end

local function snapshot(ctx)
  local base = paths(ctx)
  if not uv.fs_stat(base .. ".sock") then
    local meta = metadata(base)
    return { running = false, starting = meta and alive(meta.pid) or false }
  end
  local ok, result = pcall(evaluate, ctx, [[(function()
    local modified = {}
    for _, b in ipairs(vim.fn.getbufinfo({bufmodified=1})) do modified[#modified+1] = b.name end
    return vim.json.encode({file=vim.api.nvim_buf_get_name(0),cursor=vim.api.nvim_win_get_cursor(0),cwd=vim.fn.getcwd(),modified=modified})
  end)()]])
  if not ok then
    local meta = metadata(base)
    if not M.dead_owner(meta, alive) then error("The remote Neovim is not answering; no replacement was started: " .. tostring(result)) end
    return { running = false, stale = true }
  end
  result.running = true; result.socket = base .. ".sock"; result.nvim = vim.fn.exepath(ctx.nvim)
  local meta = metadata(base)
  if meta and meta.signature ~= ctx.signature then
    result.notice = "This remote Neovim retains its previous runtime/settings. Save and quit it to apply the update."
  end
  return result
end

local function terminal_ids(node, out)
  if type(node) ~= "table" then return end
  if node.content and node.content.terminal then out[#out+1] = node.content.terminal end
  for _, child in ipairs(node.children or {}) do terminal_ids(child, out) end
end

local function prepare(ctx)
  local tab = M.find_tab(core("doc.get", { name = "herdr-app" }), ctx.space, ctx.tab)
  assert(tab, "The remote tab is closed or its document is unavailable")
  local cwd = ctx.cwd
  if not cwd then
    local ids = {}; terminal_ids(tab.root, ids)
    local terminal = ctx.terminal or ids[1]
    if terminal then
      local facts = core("terminal.get", { terminal = terminal })
      cwd = facts.foreground and facts.foreground.cwd or facts.cwd
    end
    cwd = cwd or tab.folder or uv.os_homedir()
  end
  if cwd == "" then cwd = uv.os_homedir() end
  local real = uv.fs_realpath(cwd)
  assert(real and uv.fs_stat(real).type == "directory", "No remote folder: " .. cwd)
  ctx.cwd = real
  local ok, top = pcall(run, { "git", "-C", real, "rev-parse", "--show-toplevel" })
  ctx.root = ok and vim.trim(top) or real
  return ctx
end

local function terminals(ctx)
  return core("terminal.list", { labels = { ["herdr-nvim.remote"] = ctx.key } }).terminals
end

local function ensure(ctx, script)
  local base = paths(ctx)
  local lock = base .. ".lock"
  local fd = uv.fs_open(lock, "wx", 384)
  if not fd then
    local pid = tonumber(read(lock))
    if not pid or alive(pid) then error("Another remote Neovim action is running for this tab") end
    assert(uv.fs_unlink(lock)); fd = assert(uv.fs_open(lock, "wx", 384))
  end
  uv.fs_write(fd, tostring(uv.os_getpid())); uv.fs_close(fd)
  local ok, result = xpcall(function()
    local state = snapshot(ctx)
    if state.running then return state end
    assert(not state.starting, "The existing remote Neovim is still starting; no replacement was started")
    if state.stale then uv.fs_unlink(base .. ".sock"); uv.fs_unlink(base .. ".json") end
    for _, t in ipairs(terminals(ctx)) do
      if t.state == "live" and t.labels["herdr-nvim.remote-role"] == "daemon" then
        error("A remote Neovim daemon is already starting; retry when it is ready")
      end
    end
    core("terminal.run", {
      argv = { ctx.nvim, "--clean", "-l", script, "serve", vim.json.encode(ctx) },
      env = ctx.env, cwd = ctx.root, name = "hnvim-" .. ctx.key, no_parent = true, rm = true,
      labels = { ["herdr-nvim.remote"] = ctx.key, ["herdr-nvim.remote-role"] = "daemon" },
    })
    assert(vim.wait(8000, function() return uv.fs_stat(base .. ".sock") ~= nil end, 100),
      "Remote Neovim did not start; inspect " .. base .. ".log")
    return snapshot(ctx)
  end, debug.traceback)
  uv.fs_unlink(lock)
  if not ok then error(result) end
  return result
end

function M.daemon_argv(ctx)
  return { ctx.nvim, "--headless", "--listen", paths(ctx) .. ".sock" }
end

local function serve(ctx)
  local base = paths(ctx)
  local log = assert(io.open(base .. ".log", "w"))
  local done, stopping = false, false
  local env = vim.fn.environ(); env.HC_SCOPE = nil; env.HC_TERMINAL_ID = nil
  local job = vim.system(M.daemon_argv(ctx), {
    cwd = ctx.root, env = env, clear_env = true,
    stdout = function(_, data) if data then log:write(data); log:flush() end end,
    stderr = function(_, data) if data then log:write(data); log:flush() end end,
  }, function(result) log:write("\nNeovim exit code " .. result.code .. "\n"); done = true end)
  write(base .. ".json", { pid = job.pid, signature = ctx.signature })
  local signals = {}
  for _, name in ipairs({ "sigterm", "sighup" }) do
    local signal = uv.new_signal()
    signal:start(name, function() stopping = true end)
    signals[#signals+1] = signal
  end
  while not done and not stopping do
    local ok, tab = pcall(function() return M.find_tab(core("doc.get", { name = "herdr-app" }), ctx.space, ctx.tab) end)
    if ok and tab == false then break end
    vim.wait(5000, function() return done or stopping end, 100)
  end
  if not done then
    job:kill(15)
    if not vim.wait(2000, function() return done end, 50) then job:kill(9); vim.wait(2000, function() return done end, 50) end
  end
  for _, signal in ipairs(signals) do signal:stop(); signal:close() end
  uv.fs_unlink(base .. ".sock"); uv.fs_unlink(base .. ".json")
  log:close()
end

local function request(op, ctx, script)
  if op == "serve" then serve(ctx); return end
  prepare(ctx)
  if op == "info" then
    local state = snapshot(ctx)
    state.doc = core("doc.get", { name = "herdr-app" })
    state.terminals = terminals(ctx); state.tab_root = ctx.root
    return state
  end
  if op == "open" then
    ensure(ctx, script)
    if ctx.file then
      local path = ctx.file
      if path:sub(1, 2) == "~/" then path = uv.os_homedir() .. path:sub(2) end
      if path:sub(1, 1) ~= "/" then path = ctx.cwd .. "/" .. path end
      assert(uv.fs_stat(path) and uv.fs_stat(path).type == "file", "No remote file: " .. path)
      local lua = "(function() vim.cmd.edit(vim.fn.fnameescape(" .. string.format("%q", path) .. ")); "
      if ctx.line then lua = lua .. string.format("vim.api.nvim_win_set_cursor(0,{math.min(%d,vim.api.nvim_buf_line_count(0)),%d}); ", math.max(1, ctx.line), math.max(0, (ctx.col or 1)-1)) end
      evaluate(ctx, lua .. "return vim.json.encode(true) end)()")
    end
    return snapshot(ctx)
  end
  if op == "stop" then
    local state = snapshot(ctx)
    assert(not state.running or #state.modified == 0, "Save the remote Neovim's unsaved buffers before stopping it")
    if state.running then
      run({ ctx.nvim, "--clean", "--headless", "--server", state.socket, "--remote-send", "<C-\\><C-n><Cmd>qa<CR>" })
      assert(vim.wait(4000, function() return uv.fs_stat(state.socket) == nil end, 100),
        "Remote Neovim did not quit; check for unsaved buffers or a prompt")
    end
    return { running = false }
  end
  error("Unknown remote Neovim operation: " .. op)
end

function M.parse_arguments(argv)
  local text = assert(argv[2], "Missing remote context")
  return argv[1], vim.json.decode(text), argv[0]
end

function M.main(argv)
  local op, ctx, script = M.parse_arguments(argv)
  local result = request(op, ctx, script)
  if result then io.stdout:write(vim.json.encode(result) .. "\n") end
end

if ... == nil then
  local ok, err = pcall(M.main, arg)
  if not ok then io.stderr:write(tostring(err) .. "\n"); os.exit(1) end
end
return M
