local M = {}
local exec_mod = require("herdr-nvim.exec")

function M.send(pane_id, text, opts, exec)
  opts = opts or {}
  exec = exec or exec_mod.default_exec
  local r = exec({ "herdr", "agent", "send", pane_id, text })
  if r.code ~= 0 then
    return false, "herdr agent send failed: " .. (r.stderr ~= "" and r.stderr or ("exit " .. r.code))
  end
  if opts.submit then
    local k = exec({ "herdr", "pane", "send-keys", pane_id, "Return" })
    if k.code ~= 0 then
      return false, "submit keypress failed: " .. (k.stderr ~= "" and k.stderr or ("exit " .. k.code))
    end
  end
  return true
end

return M
