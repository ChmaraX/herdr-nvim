local M = {}

function M.format(items, opts)
  opts = opts or {}
  local header = "Code review comments from my editor"
  if opts.header_context then header = header .. " (" .. opts.header_context .. ")" end
  local lines = { header .. ":", "" }
  for i, item in ipairs(items) do
    local c = item.comment
    table.insert(lines, string.format("%d. %s:%d-%d", i, c.file, c.start_line, c.end_line))
    for j = 1, math.min(3, #(item.snippet or {})) do
      table.insert(lines, "   > " .. item.snippet[j])
    end
    table.insert(lines, "   Comment: " .. c.text)
    table.insert(lines, "")
  end
  table.insert(lines, "Please address each comment. Reply with what you changed per item.")
  return table.concat(lines, "\n")
end

return M
