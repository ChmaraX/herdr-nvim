local M = {}
local exec_mod = require("herdr-nvim.exec")

-- The command that delivers `text` to an agent: submitted (Enter pressed for
-- you) or pasted at the cursor. The TUI has both as herdr commands; the Herdr
-- app's go through the herdr-nvim binary (`app-send`), which uses herdr-core.
local function argv_for(pane_id, text, submit)
  if exec_mod.is_app() then
    local argv = { exec_mod.self(), "app-send", pane_id, text }
    if submit then table.insert(argv, "--submit") end
    return argv, "herdr-nvim app-send"
  end
  if submit then
    return { "herdr", "agent", "prompt", pane_id, text }, "herdr agent prompt"
  end
  return { "herdr", "pane", "send-text", pane_id, text }, "herdr pane send-text"
end

function M.send(pane_id, text, opts, exec)
  opts = opts or {}
  exec = exec or exec_mod.default_exec
  local argv, name = argv_for(pane_id, text, opts.submit)
  local r = exec(argv)
  if r.code ~= 0 then
    return false, name .. " failed: " .. (r.stderr ~= "" and r.stderr or ("exit " .. r.code))
  end
  return true
end

return M
