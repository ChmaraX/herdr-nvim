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
end

T.test("app: known explorers are recognised by their mapping", function()
  T.ok(app.is_explorer({ desc = "Explorer Snacks (root dir)", rhs = "" }))
  T.ok(app.is_explorer({ desc = "", rhs = ":Neotree toggle<CR>" }))
  T.ok(app.is_explorer({ desc = "", rhs = "<cmd>NvimTreeToggle<cr>" }))
  T.ok(not app.is_explorer({ desc = "Find files", rhs = "" }))
  T.ok(not app.is_explorer({}))
end)

T.test("app: an explorer key opens the native tree in native mode", function()
  reset()
  local opened = 0
  vim.keymap.set("n", "<leader>e", function() opened = opened + 1 end, { desc = "Explorer Snacks (root dir)" })
  local revealed = 0
  local reveal = app.reveal
  app.reveal = function() revealed = revealed + 1 end
  app.remap_explorer_keys(function() return "native" end)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<leader>e", true, true, true), "mtx", false)
  app.reveal = reveal
  T.eq(revealed, 1)
  T.eq(opened, 0)
  T.ok(vim.fn.maparg("<leader>e", "n", false, true).desc:find("^herdr%-nvim"))
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
