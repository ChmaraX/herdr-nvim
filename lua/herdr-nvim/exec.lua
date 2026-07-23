local M = {}

function M.default_exec(argv)
  local r = vim.system(argv, { text = true }):wait()
  return { code = r.code, stdout = r.stdout or "", stderr = r.stderr or "" }
end

return M
