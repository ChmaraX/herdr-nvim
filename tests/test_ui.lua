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
