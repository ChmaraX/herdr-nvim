-- File operations for the Herdr app's native tree, done by the tab's nvim so
-- its buffers and language servers stay in step, as with LazyVim's explorer:
-- Snacks' own rename (LSP `workspace/willRenameFiles` / `didRenameFiles`),
-- copy, trash and buffer delete when Snacks is there; the same steps done here
-- otherwise. `herdr-nvim fs '<request>'` calls run() with a JSON request
-- `{op, root, ...}` and gets a JSON answer, `{ok = true, ...}` or `{error}`.
--
-- Every path must be inside `root` (the tab's folder) unless the request says
-- `outside = true`, which the app sends only after the person confirmed.
local M = {}
local uv = vim.uv or vim.loop

local function snacks()
  local ok, s = pcall(require, "snacks")
  return ok and type(s) == "table" and s.rename and s or nil
end

local function norm(path)
  return vim.fs.normalize(vim.fn.fnamemodify(path, ":p")):gsub("/$", "")
end

local function exists(path)
  return uv.fs_stat(path) ~= nil
end

local function under(path, dir)
  return path == dir or path:sub(1, #dir + 1) == dir .. "/"
end

local function check_inside(req, ...)
  if req.outside then return end
  local root = norm(req.root or error("no root"))
  for _, path in ipairs({ ... }) do
    if not under(norm(path), root) then error(("%s is outside %s"):format(path, root)) end
  end
end

-- Buffers whose file is `path` or inside it.
local function buffers_under(path)
  local out = {}
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    local name = vim.api.nvim_buf_get_name(buf)
    if name ~= "" and under(norm(name), path) then table.insert(out, buf) end
  end
  return out
end

-- Point every buffer of `from` (a file, or files inside a folder) at `to`,
-- keeping the windows that show them.
local function move_buffers(from, to)
  for _, buf in ipairs(buffers_under(from)) do
    local old = norm(vim.api.nvim_buf_get_name(buf))
    local new = to .. old:sub(#from + 1)
    local nbuf = vim.fn.bufadd(new)
    vim.bo[nbuf].buflisted = true
    for _, win in ipairs(vim.fn.win_findbuf(buf)) do
      vim.api.nvim_win_call(win, function() vim.cmd("buffer " .. nbuf) end)
    end
    vim.api.nvim_buf_delete(buf, { force = true })
  end
end

-- The language servers' part of a rename (what Snacks.rename does).
local function lsp_rename(from, to, rename)
  local changes = { files = { { oldUri = vim.uri_from_fname(from), newUri = vim.uri_from_fname(to) } } }
  local clients = (vim.lsp.get_clients or vim.lsp.get_active_clients)()
  for _, client in ipairs(clients) do
    if client.supports_method("workspace/willRenameFiles") then
      local resp = client.request_sync("workspace/willRenameFiles", changes, 1000, 0)
      if resp and resp.result ~= nil then
        vim.lsp.util.apply_workspace_edit(resp.result, client.offset_encoding)
      end
    end
  end
  rename()
  for _, client in ipairs(clients) do
    if client.supports_method("workspace/didRenameFiles") then
      client.notify("workspace/didRenameFiles", changes)
    end
  end
end

local function copy_path(from, to)
  local s = snacks()
  if s then
    require("snacks.picker.util").copy_path(from, to)
  else
    local out = vim.fn.system({ "cp", "-R", from, to })
    if vim.v.shell_error ~= 0 then error(out) end
  end
  if not exists(to) then error("could not copy to " .. to) end
end

local ops = {}

-- The explorer defaults of the person's nvim (Snacks' `hidden` / `ignored`).
function ops.defaults()
  local s = snacks()
  local ok, opts = pcall(function() return s.picker.config.get({ source = "explorer" }) end)
  opts = ok and type(opts) == "table" and opts or {}
  return { ok = true, hidden = opts.hidden == true, ignored = opts.ignored == true, snacks = s ~= nil }
end

-- {path, dir}: a new file (opened in nvim) or folder.
function ops.create(req)
  local path = norm(req.path)
  check_inside(req, path)
  if exists(path) then error("already exists: " .. path) end
  vim.fn.mkdir(req.dir and path or vim.fs.dirname(path), "p")
  if not req.dir then
    vim.fn.writefile({}, path)
    vim.cmd.edit(vim.fn.fnameescape(path))
  end
  return { ok = true, path = path }
end

-- {path, to}: rename or move, telling the language servers.
function ops.rename(req)
  local from, to = norm(req.path), norm(req.to)
  check_inside(req, from, to)
  if not exists(from) then error("no such file: " .. from) end
  if exists(to) then error("already exists: " .. to) end
  local s = snacks()
  if s and vim.fn.isdirectory(from) == 0 then
    s.rename.rename_file({ from = from, to = to })
  else
    lsp_rename(from, to, function()
      vim.fn.mkdir(vim.fs.dirname(to), "p")
      if vim.fn.rename(from, to) ~= 0 then error("could not rename " .. from) end
      move_buffers(from, to)
    end)
  end
  if not exists(to) then error("could not rename " .. from) end
  return { ok = true, path = to, lsp = true }
end

-- {path, to}: copy a file or folder.
function ops.copy(req)
  local from, to = norm(req.path), norm(req.to)
  check_inside(req, from, to)
  if exists(to) then error("already exists: " .. to) end
  copy_path(from, to)
  return { ok = true, path = to }
end

-- {paths, dir}: move each into `dir` (a rename each, language servers told).
function ops.move(req)
  local moved = {}
  for _, path in ipairs(req.paths or {}) do
    local to = norm(req.dir) .. "/" .. vim.fn.fnamemodify(norm(path), ":t")
    table.insert(moved, ops.rename({ root = req.root, outside = req.outside, path = path, to = to }).path)
  end
  return { ok = true, paths = moved }
end

-- {paths, dir}: copy each into `dir`.
function ops.copy_into(req)
  local copied = {}
  for _, path in ipairs(req.paths or {}) do
    local to = norm(req.dir) .. "/" .. vim.fn.fnamemodify(norm(path), ":t")
    table.insert(copied, ops.copy({ root = req.root, outside = req.outside, path = path, to = to }).path)
  end
  return { ok = true, paths = copied }
end

-- {paths}: to the trash when the person's Snacks says so, else deleted; their
-- buffers go too.
function ops.delete(req)
  local s = snacks()
  for _, raw in ipairs(req.paths or {}) do
    local path = norm(raw)
    check_inside(req, path)
    local trashed = false
    if s then
      local ok, err = require("snacks.explorer.actions").trash(path)
      if not ok then error(err or ("could not delete " .. path)) end
      trashed = true
    elseif vim.fn.delete(path, "rf") ~= 0 then
      error("could not delete " .. path)
    end
    for _, buf in ipairs(buffers_under(path)) do
      if s then s.bufdelete({ buf = buf, force = true }) else vim.api.nvim_buf_delete(buf, { force = true }) end
    end
    if exists(path) and not trashed then error("could not delete " .. path) end
  end
  return { ok = true }
end

-- {paths}: into nvim's register, as Snacks' explorer yanks.
function ops.yank(req)
  local register = vim.v.register ~= "" and vim.v.register or "+"
  vim.fn.setreg(register, table.concat(vim.tbl_map(norm, req.paths or {}), "\n"), "l")
  return { ok = true, register = register, count = #(req.paths or {}) }
end

-- {dir}: copy the files in that register into `dir`.
function ops.paste(req)
  local dir = norm(req.dir)
  check_inside(req, dir)
  local register = vim.v.register ~= "" and vim.v.register or "+"
  local files = vim.tbl_filter(function(f) return f ~= "" and exists(f) end,
    vim.split(vim.fn.getreg(register) or "", "\n", { plain = true }))
  if #files == 0 then error(("the %s register holds no files"):format(register)) end
  local pasted = {}
  for _, from in ipairs(files) do
    local to = dir .. "/" .. vim.fn.fnamemodify(from, ":t")
    if exists(to) then error("already exists: " .. to) end
    copy_path(norm(from), to)
    table.insert(pasted, to)
  end
  return { ok = true, paths = pasted }
end

function M.run(json)
  local ok, req = pcall(vim.json.decode, json)
  if not ok or type(req) ~= "table" or not ops[req.op] then
    return vim.json.encode({ error = "unknown request" })
  end
  local done, result = pcall(ops[req.op], req)
  if not done then
    result = { error = tostring(result):gsub("^[^:]+:%d+: ", "") }
  end
  return vim.json.encode(result)
end

M.ops = ops
return M
