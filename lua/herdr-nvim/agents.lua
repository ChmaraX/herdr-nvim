local M = {}
local exec_mod = require("herdr-nvim.exec")

-- Plugin panes (sidebar/picker) only get workspace/tab context bundled as
-- HERDR_PLUGIN_CONTEXT_JSON; plain terminal panes get flat HERDR_WORKSPACE_ID /
-- HERDR_TAB_ID instead. Prefer the flat vars, fall back to decoding the JSON
-- blob so resolve() works from either context.
local function plugin_context()
  local raw = vim.env.HERDR_PLUGIN_CONTEXT_JSON
  if not raw then return {} end
  local ok, decoded = pcall(vim.json.decode, raw)
  if not ok or type(decoded) ~= "table" then return {} end
  return decoded
end

local function current_workspace_id()
  return vim.env.HERDR_WORKSPACE_ID or plugin_context().workspace_id
end

local function current_tab_id()
  return vim.env.HERDR_TAB_ID or plugin_context().tab_id
end

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
  local here = current_workspace_id()
  for _, a in ipairs(raw) do
    if not here or a.workspace_id == here then
      table.insert(out, {
        pane_id = a.pane_id,
        workspace_id = a.workspace_id,
        tab_id = a.tab_id,
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

-- Resolve the one agent to target without a picker, or nil when it's ambiguous.
-- `list` is already workspace-scoped by M.list. Narrowest unambiguous match wins:
--   1. a single agent sharing the current tab (HERDR_TAB_ID) — the sibling pane,
--      same convention the file picker uses to find "the agent in this tab";
--   2. otherwise, a lone agent in the workspace.
-- Anything ambiguous (2+ candidates) returns nil so the caller shows the picker.
function M.resolve(list)
  if #list == 1 then return list[1] end
  local tab = current_tab_id()
  if tab then
    local in_tab = {}
    for _, a in ipairs(list) do
      if a.tab_id == tab then table.insert(in_tab, a) end
    end
    if #in_tab == 1 then return in_tab[1] end
  end
  return nil
end

function M.display(agent)
  local tail = vim.fn.fnamemodify(agent.cwd, ":t")
  -- Lead with the agent kind (pi/claude/codex…) — the actual agent identity —
  -- then its state and where it's running. (The terminal title tended to just
  -- repeat the workspace/repo name shown by the cwd tail.)
  return string.format("%s · %s · %s", agent.kind, agent.status, tail)
end

return M
