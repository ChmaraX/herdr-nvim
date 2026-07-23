local M = {}
local exec_mod = require("herdr-nvim.exec")

function M.list(exec)
  exec = exec or exec_mod.default_exec
  local r = exec({ "herdr", "agent", "list" })
  if r.code ~= 0 then
    return nil, "herdr agent list failed: " .. (r.stderr ~= "" and r.stderr or ("exit " .. r.code))
  end
  local ok, decoded = pcall(vim.json.decode, r.stdout)
  if not ok or type(decoded) ~= "table" then return nil, "herdr agent list: unparseable JSON" end
  local raw = (decoded.result or {}).agents or {}
  local out = {}
  local here = vim.env.HERDR_WORKSPACE_ID
  for _, a in ipairs(raw) do
    if not here or a.workspace_id == here then
      table.insert(out, {
        pane_id = a.pane_id,
        workspace_id = a.workspace_id,
        kind = a.agent or "unknown",
        status = a.agent_status or "unknown",
        cwd = a.cwd or "",
        title = a.terminal_title or a.agent or "agent",
      })
    end
  end
  table.sort(out, function(x, y) return x.title < y.title end)
  return out
end

function M.display(agent)
  local tail = vim.fn.fnamemodify(agent.cwd, ":t")
  -- Lead with the agent kind (pi/claude/codex…) — the actual agent identity —
  -- then its state and where it's running. (The terminal title tended to just
  -- repeat the workspace/repo name shown by the cwd tail.)
  return string.format("%s · %s · %s", agent.kind, agent.status, tail)
end

return M
