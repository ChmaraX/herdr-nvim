local M = {}

function M.default_exec(argv)
  local r = vim.system(argv, { text = true }):wait()
  return { code = r.code, stdout = r.stdout or "", stderr = r.stderr or "" }
end

function M.list(exec)
  exec = exec or M.default_exec
  local r = exec({ "herdr", "agent", "list" })
  if r.code ~= 0 then
    return nil, "herdr agent list failed: " .. (r.stderr ~= "" and r.stderr or ("exit " .. r.code))
  end
  local ok, decoded = pcall(vim.json.decode, r.stdout)
  if not ok or type(decoded) ~= "table" then return nil, "herdr agent list: unparseable JSON" end
  local raw = (decoded.result or {}).agents or {}
  local out = {}
  for _, a in ipairs(raw) do
    table.insert(out, {
      pane_id = a.pane_id,
      workspace_id = a.workspace_id,
      kind = a.agent or "unknown",
      status = a.agent_status or "unknown",
      cwd = a.cwd or "",
      title = a.terminal_title or a.agent or "agent",
    })
  end
  local here = vim.env.HERDR_WORKSPACE_ID
  table.sort(out, function(x, y)
    local xh, yh = x.workspace_id == here, y.workspace_id == here
    if xh ~= yh then return xh end
    return x.title < y.title
  end)
  return out
end

function M.display(agent)
  local tail = vim.fn.fnamemodify(agent.cwd, ":t")
  return string.format("%s · %s · %s", agent.title, agent.status, tail)
end

return M
