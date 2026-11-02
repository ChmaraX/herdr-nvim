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

-- Where `path` really is, symlinks resolved: a folder all the way, else its
-- deepest existing folder, with the rest appended (a file, or one still to
-- be made; a link itself is the link, not what it points to).
local function real(path)
  if vim.fn.isdirectory(path) == 1 then return uv.fs_realpath(path) or path end
  local dir, rest = vim.fs.dirname(path), { vim.fs.basename(path) }
  while not uv.fs_realpath(dir) and dir ~= vim.fs.dirname(dir) do
    table.insert(rest, 1, vim.fs.basename(dir))
    dir = vim.fs.dirname(dir)
  end
  return ((uv.fs_realpath(dir) or dir) .. "/" .. table.concat(rest, "/")):gsub("//+", "/")
end

-- Every path inside the root, as the file system resolves them (a symlink in
-- the root to elsewhere is elsewhere), unless the person confirmed.
local function check_inside(req, ...)
  if req.outside then return end
  local root = real(norm(req.root or error("no root")))
  for _, path in ipairs({ ... }) do
    if not under(real(norm(path)), root) then error(("%s is outside %s"):format(path, root)) end
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

-- The buffer named exactly `path`, if any.
local function buffer_named(path)
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if norm(vim.api.nvim_buf_get_name(buf)) == path then return buf end
  end
end

-- Point every buffer of `from` (a file, or files inside a folder) at `to`.
-- The buffer itself is renamed, so its windows, unsaved edits, modified flag
-- and undo stay. A renamed buffer is marked "not edited" (`:w` would then say
-- E13 and want `:w!`), so it is reloaded from its new name and any unsaved
-- lines are put back on top: one more undo step, nothing written to disk.
local function move_buffers(from, to)
  for _, buf in ipairs(buffers_under(from)) do
    local old = norm(vim.api.nvim_buf_get_name(buf))
    local new = to .. old:sub(#from + 1)
    local lines = vim.bo[buf].modified and vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    vim.api.nvim_buf_set_name(buf, new)
    -- Renaming leaves an unlisted buffer under the old name; nothing is there now.
    local left = buffer_named(old)
    if left and left ~= buf then pcall(vim.api.nvim_buf_delete, left, { force = true }) end
    vim.api.nvim_buf_call(buf, function()
      vim.cmd("silent keepalt edit!")
      if lines then
        vim.o.undolevels = vim.o.undolevels -- its own undo step
        vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
      end
    end)
  end
end

-- `client:name(...)` on nvim 0.11+ (the `client.name(...)` calls are
-- deprecated there and gone in 0.13); 0.10 has only the dot calls.
local METHOD_CALLS = vim.fn.has("nvim-0.11") == 1
local function call(client, name, ...)
  if METHOD_CALLS then return client[name](client, ...) end
  return client[name](...)
end

-- The language servers' part of a rename (what Snacks.rename does).
local function lsp_rename(from, to, rename)
  local changes = { files = { { oldUri = vim.uri_from_fname(from), newUri = vim.uri_from_fname(to) } } }
  local clients = (vim.lsp.get_clients or vim.lsp.get_active_clients)()
  for _, client in ipairs(clients) do
    if call(client, "supports_method", "workspace/willRenameFiles") then
      local resp = call(client, "request_sync", "workspace/willRenameFiles", changes, 1000, 0)
      if resp and resp.result ~= nil then
        vim.lsp.util.apply_workspace_edit(resp.result, client.offset_encoding)
      end
    end
  end
  rename()
  for _, client in ipairs(clients) do
    if call(client, "supports_method", "workspace/didRenameFiles") then
      call(client, "notify", "workspace/didRenameFiles", changes)
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

-- {path, folder}: a new file (opened in nvim), or a folder when `folder`.
function ops.create(req)
  local path = norm(req.path)
  check_inside(req, path)
  if exists(path) then error("already exists: " .. path) end
  vim.fn.mkdir(req.folder and path or vim.fs.dirname(path), "p")
  if not req.folder then
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
  -- Snacks' own rename swaps in a fresh buffer and force-deletes the old one,
  -- losing unsaved edits, so the buffers are always moved here; Snacks, when
  -- it is there, only tells the language servers.
  local s = snacks()
  local tell = s and s.rename.on_rename_file or lsp_rename
  tell(from, to, function()
    vim.fn.mkdir(vim.fs.dirname(to), "p")
    if vim.fn.rename(from, to) ~= 0 then error("could not rename " .. from) end
    move_buffers(from, to)
  end)
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

-- Runs `op` ({root, outside, path, to}) for each of `req.paths`, to the same
-- name in `req.dir`; the paths it made.
local function each_into(req, op)
  local done = {}
  for _, path in ipairs(req.paths or {}) do
    local to = norm(req.dir) .. "/" .. vim.fn.fnamemodify(norm(path), ":t")
    table.insert(done, op({ root = req.root, outside = req.outside, path = path, to = to }).path)
  end
  return { ok = true, paths = done }
end

-- {paths, dir}: move each into `dir` (a rename each, language servers told).
function ops.move(req) return each_into(req, ops.rename) end

-- {paths, dir}: copy each into `dir`.
function ops.copy_into(req) return each_into(req, ops.copy) end

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
