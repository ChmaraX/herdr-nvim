local M = {}
local comments = require("herdr-nvim.comments")
local agents = require("herdr-nvim.agents")

-- A vertical bar in the sign column spans the annotated block and bends into the
-- callout below it (like an editor scope line, but marking your comment). Uses
-- the diagnostic "warn" color every colorscheme defines; never touches the code
-- text itself, so it stays fully readable.
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

-- The virtual lines rendered beneath an annotated block: the bar bends (╰─) into
-- a callout showing the full comment text on its own line (never covers code).
function M._callout(text)
  return { { { "╰─ ", "HerdrNvimCommentSign" }, { "💬 " .. text, "HerdrNvimCommentText" } } }
end

function M.decorate(id)
  local c = comments.get(id)
  if not c then
    return
  end
  local ns = comments.ns
  -- 1. A vertical bar in the sign column on every line of the block: the top
  --    line rounds in (╭), the rest continue (│); the callout below closes it.
  local signs = {}
  for line = c.start_line, c.end_line do
    local glyph = (line == c.start_line) and "╭" or "│"
    signs[#signs + 1] = vim.api.nvim_buf_set_extmark(c.bufnr, ns, line - 1, 0, {
      sign_text = glyph,
      sign_hl_group = "HerdrNvimCommentSign",
    })
  end
  -- 2. Callout line beneath the block.
  local callout = vim.api.nvim_buf_set_extmark(c.bufnr, ns, c.end_line - 1, 0, {
    virt_lines = M._callout(c.text),
    virt_lines_above = false,
  })
  decorations[id] = { signs = signs, callout = callout, bufnr = c.bufnr }
end

function M.undecorate(id)
  local marks = decorations[id]
  if marks and vim.api.nvim_buf_is_valid(marks.bufnr) then
    for _, sign in ipairs(marks.signs) do
      vim.api.nvim_buf_del_extmark(marks.bufnr, comments.ns, sign)
    end
    vim.api.nvim_buf_del_extmark(marks.bufnr, comments.ns, marks.callout)
  end
  decorations[id] = nil
end

function M.comment_row(c)
  return string.format("%s:%d-%d  %s", vim.fn.fnamemodify(c.file, ":t"), c.start_line, c.end_line, c.text)
end

-- Interactive comment list: a rounded floating box (with a shortcut-hint footer)
-- listing one row per comment. Moving the cursor auto-previews (jumps the code
-- window to that comment); <CR> edits, `d` deletes, `q`/<Esc> closes.
-- `handlers.edit(c, refresh)` and `handlers.delete(c)` do the actual work.
function M.comment_list(handlers)
  local code_win = vim.api.nvim_get_current_win()
  local rows = comments.list()
  if #rows == 0 then
    vim.notify("herdr-nvim: no comments", vim.log.levels.INFO)
    return
  end

  local buf = vim.api.nvim_create_buf(false, true)
  vim.bo[buf].bufhidden = "wipe"
  vim.bo[buf].filetype = "herdr-nvim-comments"

  local function win_config()
    local width = 46
    for _, c in ipairs(rows) do
      width = math.max(width, vim.fn.strdisplaywidth(M.comment_row(c)) + 2)
    end
    width = math.min(width, math.max(46, vim.o.columns - 6))
    local height = math.max(1, math.min(#rows, 12))
    return {
      relative = "editor",
      width = width,
      height = height,
      row = math.max(0, vim.o.lines - height - 4),
      col = math.max(0, math.floor((vim.o.columns - width) / 2)),
      style = "minimal",
      border = "rounded",
      title = { { " 💬 Comments ", "HerdrNvimCommentText" } },
      title_pos = "center",
      footer = { { " ↑↓ jump  ·  ⏎ edit  ·  d delete  ·  q close ", "Comment" } },
      footer_pos = "center",
    }
  end

  local win = vim.api.nvim_open_win(buf, true, win_config())
  vim.wo[win].cursorline = true
  vim.wo[win].wrap = false

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
    if vim.api.nvim_win_is_valid(win) then
      vim.api.nvim_win_set_config(win, win_config())
    end
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
