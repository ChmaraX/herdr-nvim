local M = {}

function M.format(items, opts)
  opts = opts or {}
  local header = "Code review comments from my editor"
  if opts.header_context then header = header .. " (" .. opts.header_context .. ")" end
  local lines = { header .. ":", "" }
  for i, item in ipairs(items) do
    local c = item.comment
    table.insert(lines, string.format("%d. %s", i, M.location(c, opts.cwd)))
    for j = 1, math.min(3, #(item.snippet or {})) do
      table.insert(lines, "   > " .. item.snippet[j])
    end
    table.insert(lines, "   Comment: " .. c.text)
    table.insert(lines, "")
  end
  table.insert(lines, "Please address each comment. Reply with what you changed per item.")
  return table.concat(lines, "\n")
end

-- `file` shortened against the agent's cwd, so a reference reads as a path you
-- would type ("lua/herdr-nvim/init.lua"), not a wall of home directory. A file
-- outside the agent's cwd keeps its absolute path -- it is the only spelling
-- that still resolves from there.
function M._relpath(file, cwd)
  if not cwd or cwd == "" then
    return file
  end
  local prefix = cwd:gsub("/$", "") .. "/"
  if file:sub(1, #prefix) == prefix then
    return file:sub(#prefix + 1)
  end
  return file
end

-- The one spelling of a location, shared by comments and references: the path
-- shortened against `cwd`, and a single line collapsed to `path:12`.
function M.location(item, cwd)
  local path = M._relpath(item.file, cwd)
  if item.start_line == item.end_line then
    return string.format("%s:%d", path, item.start_line)
  end
  return string.format("%s:%d-%d", path, item.start_line, item.end_line)
end

-- A bare file:line citation for dropping into a half-typed message: no header,
-- no code, no git context. Ends with a space so the sentence carries on where
-- the reference stops.
function M.format_ref(item, opts)
  opts = opts or {}
  return M.location(item, opts.cwd) .. " "
end

return M
