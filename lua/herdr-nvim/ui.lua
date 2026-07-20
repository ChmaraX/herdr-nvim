local M = {}
local comments = require("herdr-nvim.comments")
local agents = require("herdr-nvim.agents")

vim.api.nvim_set_hl(0, "HerdrNvimComment", { default = true, link = "DiffText" })

local decorations = {} -- comment id -> decoration extmark id

function M.visual_range()
  local s = vim.api.nvim_buf_get_mark(0, "<")[1]
  local e = vim.api.nvim_buf_get_mark(0, ">")[1]
  if s > e then s, e = e, s end
  return s, e
end

function M.input_comment(on_done)
  vim.ui.input({ prompt = "Comment: " }, function(text)
    if text and text ~= "" then on_done(text) end
  end)
end

function M.decorate(id)
  local c = comments.get(id)
  if not c then return end
  local label = c.text
  if #label > 30 then label = label:sub(1, 30) .. "…" end
  decorations[id] = vim.api.nvim_buf_set_extmark(c.bufnr, comments.ns, c.start_line - 1, 0, {
    -- end_row/end_col are exclusive. Use the start of the line after the
    -- selected range so one-line comments highlight that line, and multi-line
    -- comments include the final selected line.
    end_row = c.end_line,
    end_col = 0,
    hl_group = "HerdrNvimComment",
    hl_eol = true,
    virt_text = { { "● " .. label, "Comment" } },
    virt_text_pos = "eol",
  })
end

function M.undecorate(id)
  local c = comments.get(id)
  local mark = decorations[id]
  if c and mark and vim.api.nvim_buf_is_valid(c.bufnr) then
    vim.api.nvim_buf_del_extmark(c.bufnr, comments.ns, mark)
  end
  decorations[id] = nil
end

function M.comment_row(c)
  return string.format("%s:%d-%d  %s",
    vim.fn.fnamemodify(c.file, ":t"), c.start_line, c.end_line, c.text)
end

function M.pick_comment(on_choice)
  local list = comments.list()
  if #list == 0 then
    vim.notify("herdr-nvim: no comments", vim.log.levels.INFO)
    return
  end
  vim.ui.select(list, { prompt = "Comments", format_item = M.comment_row }, function(c)
    if c then on_choice(c) end
  end)
end

function M.pick_agent(agent_list, on_choice)
  if #agent_list == 0 then
    vim.notify("herdr-nvim: no herdr agents found", vim.log.levels.WARN)
    return
  end
  vim.ui.select(agent_list, { prompt = "Send to agent", format_item = agents.display }, function(a)
    if a then
      if a.status == "working" then
        vim.notify("herdr-nvim: " .. a.title .. " is working — sending anyway", vim.log.levels.WARN)
      end
      on_choice(a)
    end
  end)
end

return M
