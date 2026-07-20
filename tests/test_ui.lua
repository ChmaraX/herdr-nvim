local ui = require("herdr-nvim.ui")
local comments = require("herdr-nvim.comments")

T.test("ui: visual_range normalizes reversed marks", function()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "a", "b", "c", "d" })
  vim.api.nvim_set_current_buf(b)
  vim.api.nvim_buf_set_mark(b, "<", 3, 0, {})
  vim.api.nvim_buf_set_mark(b, ">", 1, 0, {})
  local s, e = ui.visual_range()
  T.eq({ s, e }, { 1, 3 })
end)

T.test("ui: decorate adds virtual text extmark, undecorate removes it", function()
  comments.clear()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "x", "y" })
  local id = comments.add(b, 1, 2, "needs work here truly")
  ui.decorate(id)
  local marks = vim.api.nvim_buf_get_extmarks(b, comments.ns, 0, -1, { details = true })
  local found = false
  for _, m in ipairs(marks) do
    if m[4].virt_text then found = true end
  end
  T.ok(found, "expected a virt_text decoration")
  ui.undecorate(id)
  marks = vim.api.nvim_buf_get_extmarks(b, comments.ns, 0, -1, { details = true })
  for _, m in ipairs(marks) do T.ok(not m[4].virt_text) end
end)

T.test("ui: comment row format", function()
  T.eq(ui.comment_row({ file = "/a/b/mod.rs", start_line = 3, end_line = 9, text = "tidy" }),
    "mod.rs:3-9  tidy")
end)
