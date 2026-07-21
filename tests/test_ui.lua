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

T.test("ui: decorate underlines the range + adds a callout below, undecorate removes both", function()
  comments.clear()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "x", "y" })
  local id = comments.add(b, 1, 2, "needs work here truly")
  ui.decorate(id)
  local marks = vim.api.nvim_buf_get_extmarks(b, comments.ns, 0, -1, { details = true })
  local hl, callout
  for _, m in ipairs(marks) do
    if m[4].hl_group == "HerdrNvimComment" then hl = m end
    if m[4].virt_lines then callout = m end
  end
  T.ok(hl, "expected an underline hl extmark over the range")
  T.eq({ hl[4].end_row, hl[4].end_col }, { 2, 0 }, "underline should cover through the final selected line")
  T.ok(callout, "expected a callout virt_lines extmark")
  ui.undecorate(id)
  marks = vim.api.nvim_buf_get_extmarks(b, comments.ns, 0, -1, { details = true })
  for _, m in ipairs(marks) do
    T.ok(not m[4].virt_lines, "callout removed")
    T.ok(m[4].hl_group ~= "HerdrNvimComment", "underline removed")
  end
end)

T.test("ui: one-line decoration underline covers that line", function()
  comments.clear()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "x", "y" })
  local id = comments.add(b, 1, 1, "single")
  ui.decorate(id)
  local marks = vim.api.nvim_buf_get_extmarks(b, comments.ns, 0, -1, { details = true })
  local hl
  for _, m in ipairs(marks) do
    if m[4].hl_group == "HerdrNvimComment" then hl = m end
  end
  T.ok(hl, "expected underline extmark")
  T.eq({ hl[2], hl[3], hl[4].end_row, hl[4].end_col }, { 0, 0, 1, 0 })
end)

T.test("ui: comment row format", function()
  T.eq(ui.comment_row({ file = "/a/b/mod.rs", start_line = 3, end_line = 9, text = "tidy" }),
    "mod.rs:3-9  tidy")
end)
