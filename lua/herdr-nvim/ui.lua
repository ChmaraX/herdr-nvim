local M = {}
local comments = require("herdr-nvim.comments")
local agents = require("herdr-nvim.agents")

-- Underline the annotated range (theme-aware undercurl, no background fill, so
-- the code text underneath stays fully readable). Callout marker/text reuse the
-- diagnostic "warn" styling every colorscheme defines.
vim.api.nvim_set_hl(0, "HerdrNvimComment", { default = true, link = "DiagnosticUnderlineWarn" })
vim.api.nvim_set_hl(0, "HerdrNvimCommentSign", { default = true, link = "DiagnosticWarn" })
vim.api.nvim_set_hl(0, "HerdrNvimCommentText", { default = true, link = "DiagnosticVirtualTextWarn" })

local decorations = {} -- comment id -> { hl, callout, bufnr }

function M.visual_range()
  local s = vim.api.nvim_buf_get_mark(0, "<")[1]
  local e = vim.api.nvim_buf_get_mark(0, ">")[1]
  if s > e then
    s, e = e, s
  end
  return s, e
end

function M.input_comment(on_done)
  vim.ui.input({ prompt = "Comment: " }, function(text)
    if text and text ~= "" then
      on_done(text)
    end
  end)
end

-- The virtual lines rendered beneath an annotated block: a callout showing the
-- full comment text on its own line (much more visible than end-of-line text,
-- and it never covers the code).
function M._callout(text)
  return { { { "╰─ ", "HerdrNvimCommentSign" }, { "💬 " .. text, "HerdrNvimCommentText" } } }
end

function M.decorate(id)
  local c = comments.get(id)
  if not c then
    return
  end
  local ns = comments.ns
  -- 1. Undercurl over the annotated range (end_row/end_col exclusive → cover
  --    through the final selected line).
  local hl = vim.api.nvim_buf_set_extmark(c.bufnr, ns, c.start_line - 1, 0, {
    end_row = c.end_line,
    end_col = 0,
    hl_group = "HerdrNvimComment",
  })
  -- 2. Callout line(s) beneath the block.
  local callout = vim.api.nvim_buf_set_extmark(c.bufnr, ns, c.end_line - 1, 0, {
    virt_lines = M._callout(c.text),
    virt_lines_above = false,
  })
  decorations[id] = { hl = hl, callout = callout, bufnr = c.bufnr }
end

function M.undecorate(id)
  local marks = decorations[id]
  if marks and vim.api.nvim_buf_is_valid(marks.bufnr) then
    vim.api.nvim_buf_del_extmark(marks.bufnr, comments.ns, marks.hl)
    vim.api.nvim_buf_del_extmark(marks.bufnr, comments.ns, marks.callout)
  end
  decorations[id] = nil
end

function M.comment_row(c)
  return string.format("%s:%d-%d  %s", vim.fn.fnamemodify(c.file, ":t"), c.start_line, c.end_line, c.text)
end

-- Interactive comment list: a bottom split with one row per comment. Moving the
-- cursor auto-previews (jumps the code window to that comment); <CR> edits, `d`
-- deletes, `q`/<Esc> closes. `handlers.edit(c, refresh)` and
-- `handlers.delete(c)` do the actual work.
function M.comment_list(handlers)
  local code_win = vim.api.nvim_get_current_win()
  if #comments.list() == 0 then
    vim.notify("herdr-nvim: no comments", vim.log.levels.INFO)
    return
  end

  local buf = vim.api.nvim_create_buf(false, true)
  vim.bo[buf].bufhidden = "wipe"
  vim.bo[buf].filetype = "herdr-nvim-comments"

  vim.cmd("botright split")
  local win = vim.api.nvim_get_current_win()
  vim.api.nvim_win_set_buf(win, buf)
  vim.wo[win].number = false
  vim.wo[win].relativenumber = false
  vim.wo[win].signcolumn = "no"
  vim.wo[win].cursorline = true
  vim.wo[win].winfixheight = true

  local rows = {}

  local function render()
    rows = comments.list()
    if #rows == 0 then
      if vim.api.nvim_win_is_valid(win) then
        vim.api.nvim_win_close(win, true)
      end
      return false
    end
    local lines = {}
    for _, c in ipairs(rows) do
      lines[#lines + 1] = M.comment_row(c)
    end
    vim.bo[buf].modifiable = true
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
    vim.bo[buf].modifiable = false
    vim.api.nvim_win_set_height(win, math.max(3, math.min(12, #rows)))
    return true
  end

  local function current()
    if not vim.api.nvim_win_is_valid(win) then
      return nil
    end
    return rows[vim.api.nvim_win_get_cursor(win)[1]]
  end

  local function preview()
    local c = current()
    if not c or not vim.api.nvim_win_is_valid(code_win) or not vim.api.nvim_buf_is_valid(c.bufnr) then
      return
    end
    vim.api.nvim_win_set_buf(code_win, c.bufnr)
    local line = math.min(c.start_line, math.max(1, vim.api.nvim_buf_line_count(c.bufnr)))
    vim.api.nvim_win_set_cursor(code_win, { line, 0 })
    vim.api.nvim_win_call(code_win, function()
      vim.cmd("normal! zz")
    end)
  end

  render()
  preview()

  local grp = vim.api.nvim_create_augroup("HerdrNvimCommentList" .. buf, { clear = true })
  vim.api.nvim_create_autocmd("CursorMoved", { group = grp, buffer = buf, callback = preview })

  local function close()
    if vim.api.nvim_win_is_valid(win) then
      vim.api.nvim_win_close(win, true)
    end
  end
  local function map(lhs, fn)
    vim.keymap.set("n", lhs, fn, { buffer = buf, nowait = true, silent = true })
  end

  map("q", close)
  map("<Esc>", close)
  map("<CR>", function()
    local c = current()
    if c then
      handlers.edit(c, function()
        if render() then
          preview()
        end
      end)
    end
  end)
  local function del()
    local c = current()
    if c then
      handlers.delete(c)
      if render() then
        preview()
      end
    end
  end
  map("d", del)
  map("dd", del)
end

function M.pick_agent(agent_list, on_choice)
  if #agent_list == 0 then
    vim.notify("herdr-nvim: no herdr agents found", vim.log.levels.WARN)
    return
  end
  vim.ui.select(agent_list, { prompt = "Send to agent", format_item = agents.display }, function(a)
    if a then
      if a.status == "working" then
        vim.notify("herdr-nvim: " .. agents.display(a) .. " is working — sending anyway", vim.log.levels.WARN)
      end
      on_choice(a)
    end
  end)
end

return M
