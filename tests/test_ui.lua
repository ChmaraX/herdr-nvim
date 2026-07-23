local ui = require("herdr-nvim.ui")
local comments = require("herdr-nvim.comments")

local function scratch_named(lines, name)
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, lines)
  if name then vim.api.nvim_buf_set_name(b, name) end
  return b
end

T.test("ui: visual_range normalizes reversed marks", function()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "a", "b", "c", "d" })
  vim.api.nvim_set_current_buf(b)
  vim.api.nvim_buf_set_mark(b, "<", 3, 0, {})
  vim.api.nvim_buf_set_mark(b, ">", 1, 0, {})
  local s, e = ui.visual_range()
  T.eq({ s, e }, { 1, 3 })
end)

T.test("ui: decorate marks each line of the block with a sign bar + a callout, undecorate removes both", function()
  comments.clear()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "x", "y", "z" })
  local id = comments.add(b, 1, 2, "needs work here truly") -- 2-line block
  ui.decorate(id)
  local marks = vim.api.nvim_buf_get_extmarks(b, comments.ns, 0, -1, { details = true })
  local bars, callout = 0, nil
  for _, m in ipairs(marks) do
    if m[4].virt_text then bars = bars + 1 end
    if m[4].virt_lines then callout = m end
  end
  T.eq(bars, 2, "one inline bar per annotated line")
  T.ok(callout, "expected a callout virt_lines extmark")
  ui.undecorate(id)
  marks = vim.api.nvim_buf_get_extmarks(b, comments.ns, 0, -1, { details = true })
  for _, m in ipairs(marks) do
    T.ok(not m[4].virt_text, "bars removed")
    T.ok(not m[4].virt_lines, "callout removed")
  end
end)

T.test("ui: one-line decoration marks a single line", function()
  comments.clear()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "x", "y" })
  local id = comments.add(b, 1, 1, "single")
  ui.decorate(id)
  local marks = vim.api.nvim_buf_get_extmarks(b, comments.ns, 0, -1, { details = true })
  local bars = 0
  for _, m in ipairs(marks) do
    if m[4].virt_text then bars = bars + 1 end
  end
  T.eq(bars, 1)
end)

T.test("ui: comment row format", function()
  T.eq(ui.comment_row({ file = "/a/b/mod.rs", start_line = 3, end_line = 9, text = "tidy" }),
    "mod.rs:3-9  tidy")
end)

local function list_keymap(buf, lhs)
  for _, m in ipairs(vim.api.nvim_buf_get_keymap(buf, "n")) do
    if m.lhs == lhs then return m.callback end
  end
end

T.test("ui: comment_list renders one line per comment", function()
  comments.clear()
  local b1 = scratch_named({ "x" }, "/tmp/hn-ui-a.lua")
  local b2 = scratch_named({ "y" }, "/tmp/hn-ui-b.lua")
  comments.add(b1, 1, 1, "first")
  comments.add(b2, 1, 1, "second")
  ui.comment_list({ edit = function() end, delete = function() end })
  local list_buf = vim.api.nvim_get_current_buf()
  local lines = vim.api.nvim_buf_get_lines(list_buf, 0, -1, false)
  T.eq(lines, {
    ui.comment_row(comments.list()[1]),
    ui.comment_row(comments.list()[2]),
  })
  vim.api.nvim_win_close(0, true)
end)

T.test("ui: deleting the last comment closes the window", function()
  comments.clear()
  local b = scratch_named({ "x" }, "/tmp/hn-ui-c.lua")
  local id = comments.add(b, 1, 1, "only")
  ui.comment_list({
    edit = function() end,
    delete = function(c) comments.delete(c.id) end,
  })
  local win = vim.api.nvim_get_current_win()
  local list_buf = vim.api.nvim_get_current_buf()
  T.ok(comments.get(id) ~= nil)
  local del = list_keymap(list_buf, "d")
  T.ok(del ~= nil, "expected a 'd' keymap in the comment list")
  del()
  T.eq(comments.get(id), nil)
  T.ok(not vim.api.nvim_win_is_valid(win), "window should close once no comments remain")
end)

T.test("ui: editing a comment refreshes its row", function()
  comments.clear()
  local b = scratch_named({ "x" }, "/tmp/hn-ui-d.lua")
  comments.add(b, 1, 1, "before")
  ui.comment_list({
    edit = function(c, refresh)
      comments.edit(c.id, "after")
      refresh()
    end,
    delete = function() end,
  })
  local list_buf = vim.api.nvim_get_current_buf()
  local enter = list_keymap(list_buf, "<CR>")
  T.ok(enter ~= nil, "expected a <CR> keymap in the comment list")
  enter()
  local lines = vim.api.nvim_buf_get_lines(list_buf, 0, -1, false)
  T.eq(lines, { ui.comment_row(comments.list()[1]) })
  T.ok(lines[1]:find("after", 1, true) ~= nil)
  vim.api.nvim_win_close(0, true)
end)
