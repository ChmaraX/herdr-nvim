-- The Herdr desktop app's side of nvim (only when herdr-nvim started this nvim
-- for the app: HERDR_NVIM_HOST=app; never in the TUI plugin or a nvim the
-- person starts themselves).
--
-- * The current file, written to $HERDR_NVIM_CURRENT_FILE on every buffer
--   switch, so the app's native tree can follow it.
-- * The tree key (`[app] tree_key`, default `<leader>e`) shows the native
--   tree on the current file, or hides it when it shows (nvim stays), when
--   the tree setting is "native". It is taken over when it opens a known
--   explorer (Snacks, neo-tree, nvim-tree), mapped when it is free, and left
--   alone when it does anything else; `""` maps nothing. LazyVim's other
--   explorer keys (`<leader>E`, `<leader>fe`, `<leader>fE`) focus the tree
--   instead, when they open a known explorer.
-- * The tree key's presses, with this nvim's leader, written next to the
--   current file for the tree, which takes the same key to hide itself.
-- * `tree = "nvim"`: the person's own explorer opens with the window.
local exec = require("herdr-nvim.exec")

local M = {}

M.EXPLORER_KEYS = { "<leader>e", "<leader>E", "<leader>fe", "<leader>fE" }
-- The ones that only focus the tree (the tree key toggles it).
M.REVEAL_KEYS = { "<leader>E", "<leader>fe", "<leader>fE" }
M.DEFAULT_TREE_KEY = "<leader>e"

-- What a mapping says about itself, as one lowercased string.
local function describe(map)
  return ((map.desc or "") .. " " .. (map.rhs or "")):lower()
end

-- Whether `map` (maparg(..., true)) opens a file explorer.
function M.is_explorer(map)
  if not map or vim.tbl_isempty(map) then return false end
  local text = describe(map)
  for _, pattern in ipairs({ "explorer", "neo%-?tree", "nvim%-?tree", "nvimtree" }) do
    if text:find(pattern) then return true end
  end
  return false
end

-- herdr-nvim's app settings (`herdr-nvim app-config`), {} when unreadable.
function M.settings(run)
  run = run or exec.default_exec
  local r = run({ exec.self(), "app-config" })
  local ok, decoded = pcall(vim.json.decode, r.stdout or "")
  return (ok and type(decoded) == "table") and decoded or {}
end

-- "native" or "nvim", asked at the moment it matters (a key press), so a
-- changed setting applies without restarting nvim.
function M.tree_mode(run)
  return M.settings(run).tree or "native"
end

-- Whether a mapping is one of ours.
local function ours(map)
  return map and map.desc and map.desc:find("^herdr%-nvim") ~= nil
end

-- What to do with the tree key: "ours" (done already), "take" (it opens a
-- known explorer), "map" (free: nothing mapped there, nor a longer or shorter
-- key overlapping it) or "leave" (anything else).
function M.tree_key_plan(map, overlaps)
  if not map or vim.tbl_isempty(map) then return overlaps and "leave" or "map" end
  if ours(map) then return "ours" end
  if M.is_explorer(map) then return "take" end
  return "leave"
end

local NAMED = {
  space = "space", cr = "enter", enter = "enter", ["return"] = "enter", tab = "tab", bs = "backspace",
  esc = "escape", lt = "<", bslash = "\\", bar = "|", up = "up", down = "down", left = "left", right = "right",
}
local MODS = { c = "ctrl", m = "alt", a = "alt", s = "shift", d = "cmd" }
local MOD_ORDER = { "ctrl", "alt", "shift", "cmd" } -- GPUI's order in a keystroke

-- One key as the tree (GPUI) names it: "e", "E", "space", "ctrl-n".
local function key_name(text)
  if text == " " then return "space" end
  if #text == 1 then return text end
  local lower = text:lower()
  if NAMED[lower] then return NAMED[lower] end
  local parts = vim.split(text, "-", { plain = true })
  local key, has = table.remove(parts), {}
  if key == "" then key = "-" end
  for _, m in ipairs(parts) do
    if MODS[m:lower()] then has[MODS[m:lower()]] = true end
  end
  key = NAMED[key:lower()] or (#key == 1 and key:lower()) or key:lower()
  if has.shift and #key == 1 and key:match("%a") and not (has.ctrl or has.alt or has.cmd) then return key:upper() end
  local out = {}
  for _, m in ipairs(MOD_ORDER) do
    if has[m] then out[#out + 1] = m end
  end
  out[#out + 1] = key
  return table.concat(out, "-")
end

-- The presses an lhs stands for, as the tree names keys, with `<leader>` /
-- `<localleader>` spelled out: ("<leader>e", " ") -> { "space", "e" }.
function M.key_presses(lhs, leader, localleader)
  local out, i = {}, 1
  while i <= #lhs do
    local close = lhs:sub(i, i) == "<" and lhs:find(">", i + 1, true)
    if close then
      local name = lhs:sub(i + 1, close - 1)
      local lead = ({ leader = leader or "\\", localleader = localleader or "\\" })[name:lower()]
      if lead then
        for c in lead:gmatch(".") do out[#out + 1] = key_name(c) end
      else
        out[#out + 1] = key_name(name)
      end
      i = close + 1
    else
      out[#out + 1] = key_name(lhs:sub(i, i))
      i = i + 1
    end
  end
  return out
end

-- Run what a saved mapping did.
local function call_original(map)
  if map.callback then return map.callback() end
  local keys = vim.api.nvim_replace_termcodes(map.rhs or "", true, true, true)
  vim.api.nvim_feedkeys(keys, map.noremap == 1 and "n" or "m", false)
end

M.originals = {}

-- Focus the tree on the current file.
function M.reveal()
  vim.fn.jobstart({ exec.self(), "app-reveal" }, { detach = true })
end

-- Show the tree on the current file, or hide it when it shows (nvim stays).
function M.toggle()
  vim.fn.jobstart({ exec.self(), "app-reveal", "--toggle" }, { detach = true })
end

-- The tree key's presses for the tree (it reads them when it takes the
-- keyboard): {"tree_key": ["space", "e"]}, [] for none.
function M.write_keys(tree_key)
  local path = vim.env.HERDR_NVIM_CURRENT_FILE
  if not path then return end
  local presses = tree_key == "" and {} or M.key_presses(tree_key, vim.g.mapleader, vim.g.maplocalleader)
  vim.fn.mkdir(vim.fs.dirname(path), "p")
  pcall(vim.fn.writefile, { vim.json.encode({ tree_key = presses }) }, path .. ".keys")
end

-- The tree key: show or hide the tree (in nvim mode, what the key did).
local function map_tree_key(lhs, map, plan, get_mode)
  if plan == "take" then M.originals[lhs] = map end
  vim.keymap.set("n", lhs, function()
    if get_mode() == "native" then
      M.toggle()
    elseif M.originals[lhs] then
      call_original(M.originals[lhs])
    end
  end, { desc = "herdr-nvim: Files, show/hide" .. (map.desc and (" (" .. map.desc .. ")") or "") })
end

-- Take over the keys: the tree key (see the top), and the other explorer
-- keys that open a known explorer. Runs after the person's config (VimEnter,
-- and again once LazyVim's VeryLazy maps exist); a key already ours is left
-- as it is.
function M.remap_explorer_keys(get_mode, tree_key)
  get_mode = get_mode or M.tree_mode
  tree_key = tree_key or M.tree_key or M.DEFAULT_TREE_KEY
  if tree_key ~= "" then
    local map = vim.fn.maparg(tree_key, "n", false, true)
    local plan = M.tree_key_plan(map, vim.tbl_isempty(map) and vim.fn.mapcheck(tree_key, "n") ~= "")
    if plan == "take" or plan == "map" then map_tree_key(tree_key, map, plan, get_mode) end
  end
  M.write_keys(tree_key)
  for _, lhs in ipairs(M.REVEAL_KEYS) do
    local map = vim.fn.maparg(lhs, "n", false, true)
    if lhs ~= tree_key and not ours(map) then
      if M.is_explorer(map) then
        M.originals[lhs] = map
        vim.keymap.set("n", lhs, function()
          if get_mode() == "native" then
            M.reveal()
          else
            call_original(M.originals[lhs])
          end
        end, { desc = "herdr-nvim: Files (" .. (map.desc or "explorer") .. ")" })
      end
    end
  end
end

-- Write the current file for the tree to follow (real files only).
local function write_current()
  local path = vim.env.HERDR_NVIM_CURRENT_FILE
  local name = vim.api.nvim_buf_get_name(0)
  if not path or name == "" or vim.bo.buftype ~= "" or vim.fn.filereadable(name) == 0 then return end
  vim.fn.mkdir(vim.fs.dirname(path), "p")
  -- Its real path: the tree works with real paths (/private/tmp, not /tmp).
  pcall(vim.fn.writefile, { vim.fn.resolve(vim.fn.fnamemodify(name, ":p")) }, path)
end

-- Whether an explorer window is already on screen.
function M.explorer_shown()
  for _, win in ipairs(vim.api.nvim_list_wins()) do
    local ft = vim.bo[vim.api.nvim_win_get_buf(win)].filetype
    if ft == "netrw" or ft == "snacks_picker_list" or ft == "neo-tree" or ft == "NvimTree" then return true end
  end
  return false
end

-- `tree = "nvim"`: open the explorer the person's keys open, once this
-- nvim's window is attached; nvim's own `:Explore` if there is none.
function M.explore_on_ui()
  vim.api.nvim_create_autocmd("UIEnter", {
    once = true,
    callback = vim.schedule_wrap(function()
      if M.explorer_shown() then return end
      for _, lhs in ipairs(M.EXPLORER_KEYS) do
        local map = M.originals[lhs] or vim.fn.maparg(lhs, "n", false, true)
        if M.is_explorer(map) then return call_original(map) end
      end
      pcall(vim.cmd, "Lexplore")
    end),
  })
  return 1
end

function M.setup()
  local group = vim.api.nvim_create_augroup("herdr-nvim-app", { clear = true })
  vim.api.nvim_create_autocmd("BufEnter", { group = group, callback = write_current })
  -- The window gave this nvim the keyboard (the terminal's focus report):
  -- tell the app's tree, which can't see the keyboard leave it.
  vim.api.nvim_create_autocmd("FocusGained", {
    group = group,
    callback = function()
      local path = vim.env.HERDR_NVIM_CURRENT_FILE
      if path then pcall(vim.fn.writefile, { tostring(vim.uv.hrtime()) }, path .. ".focus") end
    end,
  })
  M.tree_key = M.settings().tree_key or M.DEFAULT_TREE_KEY
  M.remap_explorer_keys()
  vim.api.nvim_create_autocmd("User", {
    group = group,
    pattern = "VeryLazy",
    callback = function() M.remap_explorer_keys() end,
  })
end

return M
