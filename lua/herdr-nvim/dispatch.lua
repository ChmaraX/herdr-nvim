local M = {}
local exec_mod = require("herdr-nvim.exec")

function M.send(pane_id, text, opts, exec)
  opts = opts or {}
  exec = exec or exec_mod.default_exec
  if opts.submit then
    -- agent prompt sends text and auto-submits (presses Enter for you)
    local r = exec({ "herdr", "agent", "prompt", pane_id, text })
    if r.code ~= 0 then
      return false, "herdr agent prompt failed: " .. (r.stderr ~= "" and r.stderr or ("exit " .. r.code))
    end
  else
    -- pane send-text sends text without auto-submitting
    local r = exec({ "herdr", "pane", "send-text", pane_id, text })
    if r.code ~= 0 then
      return false, "herdr pane send-text failed: " .. (r.stderr ~= "" and r.stderr or ("exit " .. r.code))
    end
  end
  return true
end

return M
