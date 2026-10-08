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

-- Make a real visual selection with `keys` (from line 1, col 0), leave visual
-- mode, and return visual_region() plus the text its span covers.
local function region(lines, keys, selection)
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, lines)
  vim.api.nvim_set_current_buf(b)
  local saved = vim.o.selection
  vim.o.selection = selection or "inclusive"
  vim.cmd("normal! gg0" .. keys .. "\27")
  local span = ui.visual_region()
  vim.o.selection = saved
  local text = span.cols and vim.api.nvim_buf_get_text(b, span.start_line - 1, span.cols[1], span.end_line - 1, span.cols[2], {}) or nil
  return span, text
end

T.test("ui: visual_region returns the byte span of a charwise selection", function()
  local r, text = region({ "the quick brown fox" }, "4lve")
  T.eq(r, { start_line = 1, end_line = 1, cols = { 4, 9 } })
  T.eq(text, { "quick" })
end)

T.test("ui: visual_region normalizes a selection made backwards", function()
  local r, text = region({ "the quick brown fox" }, "8lv4h")
  T.eq(r, { start_line = 1, end_line = 1, cols = { 4, 9 } })
  T.eq(text, { "quick" })
end)

T.test("ui: visual_region keeps a multibyte last character whole", function()
  local r, text = region({ "café au lait" }, "v3l")
  T.eq(r, { start_line = 1, end_line = 1, cols = { 0, 5 } })
  T.eq(text, { "café" })
end)

T.test("ui: visual_region honors selection=exclusive", function()
  local _, text = region({ "the quick brown fox" }, "4lv5l", "exclusive")
  T.eq(text, { "quick" }, "the char under '> is not selected")
  _, text = region({ "a😀b" }, "lvl", "exclusive")
  T.eq(text, { "😀" })
  _, text = region({ "the quick brown fox" }, "4lv", "exclusive")
  T.eq(text, { "q" }, "an empty exclusive selection still covers one char, as in Vim")
  _, text = region({ "the quick brown fox" }, "8lv4h", "exclusive")
  T.eq(text, { "quick" }, "a backwards exclusive selection keeps its starting char")
end)

T.test("ui: visual_region clamps v$ to the end of the line", function()
  local r, text = region({ "the quick brown fox" }, "4lv$")
  T.eq(r, { start_line = 1, end_line = 1, cols = { 4, 19 } })
  T.eq(text, { "quick brown fox" })
end)

T.test("ui: visual_region spans lines with partial first/last line", function()
  local r, text = region({ "alpha beta", "gamma delta" }, "6lvj0e")
  T.eq(r, { start_line = 1, end_line = 2, cols = { 6, 5 } })
  T.eq(text, { "beta", "gamma" })
end)

T.test("ui: visual_region gives no columns for whole-line selections", function()
  T.eq(region({ "the quick brown fox" }, "v$"), { start_line = 1, end_line = 1 }, "charwise over one whole line")
  T.eq(region({ "a", "b", "c" }, "jVj"), { start_line = 2, end_line = 3 }, "linewise")
  T.eq(region({ "abc", "def" }, "l\22jl"), { start_line = 1, end_line = 2 }, "blockwise")
end)

T.test("ui: decorate rails each line in the sign column + a callout, undecorate removes both", function()
  comments.clear()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "x", "y", "z" })
  local id = comments.add(b, { start_line = 1, end_line = 2 }, "needs work here truly") -- 2-line block
  ui.decorate(id)
  local marks = vim.api.nvim_buf_get_extmarks(b, comments.ns, 0, -1, { details = true })
  local bars, tinted, callout = 0, 0, nil
  for _, m in ipairs(marks) do
    if m[4].sign_text then bars = bars + 1 end
    if m[4].line_hl_group then tinted = tinted + 1 end
    if m[4].virt_lines then callout = m end
  end
  T.eq(bars, 2, "one sign-column rail cell per annotated line")
  T.eq(tinted, 2, "one background tint per annotated line")
  T.ok(callout, "expected a callout virt_lines extmark")
  -- The rail must NOT be inline virt_text: that shifts the annotated code
  -- sideways and knocks the block out of alignment with the rest of the file.
  for _, m in ipairs(marks) do
    T.ok(not m[4].virt_text, "rail lives in the sign column, not inline")
  end
  T.eq(callout[2], 0, "callout anchors above the FIRST line, not below the last")
  ui.undecorate(id)
  marks = vim.api.nvim_buf_get_extmarks(b, comments.ns, 0, -1, { details = true })
  for _, m in ipairs(marks) do
    T.ok(not m[4].sign_text, "rail removed")
    T.ok(not m[4].line_hl_group, "tint removed")
    T.ok(not m[4].virt_lines, "callout removed")
  end
end)

T.test("ui: one-line decoration marks a single line", function()
  comments.clear()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "x", "y" })
  local id = comments.add(b, { start_line = 1, end_line = 1 }, "single")
  ui.decorate(id)
  local marks = vim.api.nvim_buf_get_extmarks(b, comments.ns, 0, -1, { details = true })
  local bars = 0
  for _, m in ipairs(marks) do
    if m[4].sign_text then bars = bars + 1 end
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
  comments.add(b1, { start_line = 1, end_line = 1 }, "first")
  comments.add(b2, { start_line = 1, end_line = 1 }, "second")
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
  local id = comments.add(b, { start_line = 1, end_line = 1 }, "only")
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
  comments.add(b, { start_line = 1, end_line = 1 }, "before")
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
