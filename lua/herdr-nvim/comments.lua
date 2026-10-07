local M = {}
M.ns = vim.api.nvim_create_namespace("herdr-nvim-comments")
local store = {} -- id -> { bufnr, extmark, text, created_at, cols }
local next_id = 1

-- start_col / end_col (0-indexed bytes, end exclusive) are optional: pass them to
-- annotate a sub-line character span; omit them for a whole-line comment.
function M.add(bufnr, start_line, end_line, text, start_col, end_col)
  local id = next_id
  next_id = next_id + 1
  local cols = start_col ~= nil and end_col ~= nil
  local extmark
  if cols then
    -- Anchor to the exact selected columns so the span tracks edits, not the line.
    -- end_row here is the 0-indexed inclusive end line (not the exclusive-next-line
    -- form the whole-line branch uses), paired with a real end_col.
    extmark = vim.api.nvim_buf_set_extmark(bufnr, M.ns, start_line - 1, start_col, {
      end_row = end_line - 1,
      end_col = end_col,
      right_gravity = false,
      end_right_gravity = true,
    })
  else
    extmark = vim.api.nvim_buf_set_extmark(bufnr, M.ns, start_line - 1, 0, {
      end_row = end_line,
      end_col = 0,
      right_gravity = false,
      end_right_gravity = true,
    })
  end
  store[id] = { bufnr = bufnr, extmark = extmark, text = text, created_at = os.time(), cols = cols }
  return id
end

local function resolve(id)
  local e = store[id]
  if not e or not vim.api.nvim_buf_is_valid(e.bufnr) then return nil end
  local pos = vim.api.nvim_buf_get_extmark_by_id(e.bufnr, M.ns, e.extmark, { details = true })
  if not pos or #pos == 0 then return nil end
  local start_line = pos[1] + 1
  local start_col, end_col, end_line
  if e.cols then
    -- A column span stores end_row as the 0-indexed inclusive end line and a
    -- real end_col; keep both so the snippet is the exact selected text.
    start_col = pos[2]
    end_line = (pos[3] and pos[3].end_row or pos[1]) + 1
    end_col = pos[3] and pos[3].end_col or nil
  else
    -- pos[3].end_row is 0-indexed and exclusive; numerically that equals the
    -- 1-indexed inclusive end line, so no conversion is needed here.
    end_line = pos[3] and pos[3].end_row or (pos[1] + 1)
  end
  if end_line < start_line then end_line = start_line end
  return {
    id = id,
    bufnr = e.bufnr,
    file = vim.api.nvim_buf_get_name(e.bufnr),
    start_line = start_line,
    end_line = end_line,
    start_col = start_col,
    end_col = end_col,
    text = e.text,
    created_at = e.created_at,
  }
end

function M.get(id) return resolve(id) end

function M.list()
  local out = {}
  for id in pairs(store) do
    local c = resolve(id)
    if c then table.insert(out, c) end
  end
  table.sort(out, function(a, b)
    if a.file ~= b.file then return a.file < b.file end
    return a.start_line < b.start_line
  end)
  return out
end

function M.edit(id, new_text)
  if not store[id] then return false end
  store[id].text = new_text
  return true
end

function M.delete(id)
  local e = store[id]
  if not e then return false end
  if vim.api.nvim_buf_is_valid(e.bufnr) then
    vim.api.nvim_buf_del_extmark(e.bufnr, M.ns, e.extmark)
  end
  store[id] = nil
  return true
end

function M.clear()
  for id in pairs(store) do M.delete(id) end
end

function M.snippet(id)
  local c = resolve(id)
  if not c then return nil end
  if c.start_col and c.end_col then
    -- Exact selected text across the span (partial first/last line included).
    return vim.api.nvim_buf_get_text(c.bufnr, c.start_line - 1, c.start_col, c.end_line - 1, c.end_col, {})
  end
  return vim.api.nvim_buf_get_lines(c.bufnr, c.start_line - 1, c.end_line, false)
end

return M
