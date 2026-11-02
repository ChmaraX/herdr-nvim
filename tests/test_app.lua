-- The Herdr app's nvim side (lua/herdr-nvim/app.lua): which keys it takes
-- over, and what they do in each tree mode.
local app = require("herdr-nvim.app")

local function clear(lhs)
  pcall(vim.keymap.del, "n", lhs)
end

local function reset()
  vim.g.mapleader = " "
  for _, lhs in ipairs(app.EXPLORER_KEYS) do clear(lhs) end
  app.originals = {}
  app.tree_key = nil
end

-- Press `lhs` with the toggle and reveal actions counted, not run.
local function press(lhs)
  local calls = { toggle = 0, reveal = 0 }
  local toggle, reveal = app.toggle, app.reveal
  app.toggle = function() calls.toggle = calls.toggle + 1 end
  app.reveal = function() calls.reveal = calls.reveal + 1 end
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(lhs, true, true, true), "mtx", false)
  app.toggle, app.reveal = toggle, reveal
  return calls
end

T.test("app: known explorers are recognised by their mapping", function()
  T.ok(app.is_explorer({ desc = "Explorer Snacks (root dir)", rhs = "" }))
  T.ok(app.is_explorer({ desc = "", rhs = ":Neotree toggle<CR>" }))
  T.ok(app.is_explorer({ desc = "", rhs = "<cmd>NvimTreeToggle<cr>" }))
  T.ok(not app.is_explorer({ desc = "Find files", rhs = "" }))
  T.ok(not app.is_explorer({}))
end)

T.test("app: an explorer's <leader>e toggles the native tree in native mode", function()
  reset()
  local opened = 0
  vim.keymap.set("n", "<leader>e", function() opened = opened + 1 end, { desc = "Explorer Snacks (root dir)" })
  vim.keymap.set("n", "<leader>fe", function() opened = opened + 1 end, { desc = "Explorer Snacks (root dir)" })
  app.remap_explorer_keys(function() return "native" end)
  T.eq(press("<leader>e"), { toggle = 1, reveal = 0 })
  T.eq(press("<leader>fe"), { toggle = 0, reveal = 1 })
  T.eq(opened, 0)
  T.ok(vim.fn.maparg("<leader>e", "n", false, true).desc:find("^herdr%-nvim"))
end)

T.test("app: a free <leader>e is mapped to the toggle", function()
  reset()
  local mode = "native"
  app.remap_explorer_keys(function() return mode end)
  T.eq(press("<leader>e"), { toggle = 1, reveal = 0 })
  T.eq(vim.fn.maparg("<leader>e", "n", false, true).desc, "herdr-nvim: Files, show/hide")
  -- In nvim mode (asked at the press) a key that was free does nothing.
  mode = "nvim"
  T.eq(press("<leader>e"), { toggle = 0, reveal = 0 })
end)

T.test("app: a <leader>e that starts a longer mapping is not free", function()
  reset()
  vim.keymap.set("n", "<leader>ef", "<nop>", { desc = "Something else" })
  app.remap_explorer_keys(function() return "native" end)
  T.eq(vim.fn.maparg("<leader>e", "n"), "")
  clear("<leader>ef")
end)

T.test("app: tree_key = '' maps nothing; another tree_key is used instead", function()
  reset()
  app.remap_explorer_keys(function() return "native" end, "")
  T.eq(vim.fn.maparg("<leader>e", "n"), "")
  reset()
  app.remap_explorer_keys(function() return "native" end, "<C-n>")
  T.eq(press("<C-n>"), { toggle = 1, reveal = 0 })
  T.eq(vim.fn.maparg("<leader>e", "n"), "")
  clear("<C-n>")
end)

T.test("app: what to do with the tree key's mapping", function()
  T.eq(app.tree_key_plan({}, false), "map")
  T.eq(app.tree_key_plan({}, true), "leave")
  T.eq(app.tree_key_plan({ desc = "Explorer Snacks (root dir)" }), "take")
  T.eq(app.tree_key_plan({ desc = "herdr-nvim: Files, show/hide" }), "ours")
  T.eq(app.tree_key_plan({ desc = "Edit config", rhs = ":e $MYVIMRC<cr>" }), "leave")
end)

T.test("app: the tree key's presses, as the tree names keys", function()
  T.eq(app.key_presses("<leader>e", " "), { "space", "e" })
  T.eq(app.key_presses("<leader>e", nil), { "\\", "e" })
  T.eq(app.key_presses("<Leader>fE", ","), { ",", "f", "E" })
  T.eq(app.key_presses("<C-n>"), { "ctrl-n" })
  T.eq(app.key_presses("<S-e>"), { "E" })
  T.eq(app.key_presses("<C-S-x>"), { "ctrl-shift-x" })
  T.eq(app.key_presses("<Space>t"), { "space", "t" })
  T.eq(app.key_presses("<localleader>t", " ", "-"), { "-", "t" })
end)

T.test("app: the tree key's presses are written next to the current file", function()
  reset()
  local dir = vim.fn.tempname()
  vim.env.HERDR_NVIM_CURRENT_FILE = dir .. "/current"
  app.write_keys("<leader>e")
  T.eq(vim.json.decode(vim.fn.readfile(dir .. "/current.keys")[1]), { tree_key = { "space", "e" } })
  app.write_keys("")
  T.eq(vim.json.decode(vim.fn.readfile(dir .. "/current.keys")[1]), { tree_key = {} })
  vim.env.HERDR_NVIM_CURRENT_FILE = nil
  vim.fn.delete(dir, "rf")
end)

T.test("app: in nvim mode the explorer key keeps doing what it did", function()
  reset()
  local opened = 0
  vim.keymap.set("n", "<leader>e", function() opened = opened + 1 end, { desc = "Explorer Snacks (root dir)" })
  app.remap_explorer_keys(function() return "nvim" end)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<leader>e", true, true, true), "mtx", false)
  T.eq(opened, 1)
end)

T.test("app: keys that open no explorer are left alone", function()
  reset()
  local hit = 0
  vim.keymap.set("n", "<leader>e", function() hit = hit + 1 end, { desc = "Edit config" })
  app.remap_explorer_keys(function() return "native" end)
  T.eq(vim.fn.maparg("<leader>e", "n", false, true).desc, "Edit config")
  T.eq(vim.fn.maparg("<leader>fe", "n"), "")
end)

T.test("app: remapping twice keeps the person's original", function()
  reset()
  local opened = 0
  vim.keymap.set("n", "<leader>e", function() opened = opened + 1 end, { desc = "Explorer NeoTree (Root Dir)" })
  app.remap_explorer_keys(function() return "nvim" end)
  app.remap_explorer_keys(function() return "nvim" end)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<leader>e", true, true, true), "mtx", false)
  T.eq(opened, 1)
end)
