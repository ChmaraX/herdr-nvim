-- The Herdr desktop app's side of nvim (only when herdr-nvim started this nvim
-- for the app: HERDR_NVIM_HOST=app; never in the TUI plugin or a nvim the
-- person starts themselves).
--
-- * The current file, written to $HERDR_NVIM_CURRENT_FILE on every buffer
--   switch, so the app's native tree can follow it.
-- * The explorer keys (`<leader>e`, and LazyVim's `<leader>E`, `<leader>fe`,
--   `<leader>fE`) open the native tree on the current file instead, when the
--   tree setting is "native" and those keys open a known explorer (Snacks,
--   neo-tree, nvim-tree). Any other mapping there, or none, is left alone.
-- * `tree = "nvim"`: the person's own explorer opens with the window.
local exec = require("herdr-nvim.exec")

local M = {}

M.EXPLORER_KEYS = { "<leader>e", "<leader>E", "<leader>fe", "<leader>fE" }

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

-- "native" or "nvim", asked at the moment it matters (a key press), so a
-- changed setting applies without restarting nvim.
function M.tree_mode(run)
  run = run or exec.default_exec
  local r = run({ exec.self(), "app-config" })
  local ok, decoded = pcall(vim.json.decode, r.stdout or "")
  return (ok and type(decoded) == "table" and decoded.tree) or "native"
end

-- Run what a saved mapping did.
local function call_original(map)
  if map.callback then return map.callback() end
  local keys = vim.api.nvim_replace_termcodes(map.rhs or "", true, true, true)
  vim.api.nvim_feedkeys(keys, map.noremap == 1 and "n" or "m", false)
end

M.originals = {}

function M.reveal()
  vim.fn.jobstart({ exec.self(), "app-reveal" }, { detach = true })
end

-- Take over the explorer keys that open a known explorer. Runs after the
-- person's config (VimEnter, and again once LazyVim's VeryLazy maps exist);
-- a key already ours is left as it is.
function M.remap_explorer_keys(get_mode)
  get_mode = get_mode or M.tree_mode
  for _, lhs in ipairs(M.EXPLORER_KEYS) do
    local map = vim.fn.maparg(lhs, "n", false, true)
    if not map.desc or not map.desc:find("^herdr%-nvim") then
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
  pcall(vim.fn.writefile, { vim.fn.fnamemodify(name, ":p") }, path)
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
  M.remap_explorer_keys()
  vim.api.nvim_create_autocmd("User", {
    group = group,
    pattern = "VeryLazy",
    callback = function() M.remap_explorer_keys() end,
  })
end

return M
