local agents = require("herdr-nvim.agents")

local fixture = vim.json.encode({
  id = "cli:agent:list",
  result = { agents = {
    { pane_id = "wA:p1", workspace_id = "wA", agent = "pi",
      agent_status = "idle", cwd = "/tmp/proj-a", terminal_title = "π - proj-a" },
    { pane_id = "wB:p2", workspace_id = "wB", agent = "claude",
      agent_status = "working", cwd = "/tmp/proj-b" },
  } },
})

local function fake_exec(out, code)
  return function(_) return { code = code or 0, stdout = out, stderr = "" } end
end

T.test("agents: parses list and normalizes fields", function()
  local previous = vim.env.HERDR_WORKSPACE_ID
  vim.env.HERDR_WORKSPACE_ID = nil
  local list, err = agents.list(fake_exec(fixture))
  vim.env.HERDR_WORKSPACE_ID = previous
  T.eq(err, nil)
  T.eq(#list, 2)
  local by_pane = {}
  for _, agent in ipairs(list) do by_pane[agent.pane_id] = agent end
  T.eq(by_pane["wA:p1"].kind, "pi")
  T.eq(by_pane["wA:p1"].status, "idle")
  T.eq(by_pane["wA:p1"].title, "π - proj-a")
  T.eq(by_pane["wB:p2"].title, "claude") -- falls back to kind
end)

T.test("agents: no current workspace sorts by title", function()
  vim.env.HERDR_WORKSPACE_ID = nil
  local list = agents.list(fake_exec(fixture))
  T.eq(list[1].pane_id, "wB:p2")
  T.eq(list[1].title, "claude")
  T.eq(list[2].pane_id, "wA:p1")
  T.eq(list[2].title, "π - proj-a")
end)

T.test("agents: current workspace excludes other workspaces", function()
  vim.env.HERDR_WORKSPACE_ID = "wB"
  local list = agents.list(fake_exec(fixture))
  vim.env.HERDR_WORKSPACE_ID = nil
  T.eq(#list, 1)
  T.eq(list[1].pane_id, "wB:p2")
end)

T.test("agents: CLI failure returns err", function()
  local list, err = agents.list(fake_exec("", 1))
  T.eq(list, nil)
  T.ok(err and err:match("herdr"))
end)

T.test("agents: unparseable JSON returns err", function()
  local list, err = agents.list(fake_exec("not json"))
  T.eq(list, nil)
  T.ok(err and err:match("unparseable"))
end)

T.test("agents: resolve returns the lone workspace agent", function()
  local a = agents.resolve({ { pane_id = "wA:p1", tab_id = "wA:t1" } })
  T.eq(a.pane_id, "wA:p1")
end)

T.test("agents: resolve picks the single agent in the current tab", function()
  local previous = vim.env.HERDR_TAB_ID
  vim.env.HERDR_TAB_ID = "wA:t2"
  local a = agents.resolve({
    { pane_id = "wA:p1", tab_id = "wA:t1" },
    { pane_id = "wA:p2", tab_id = "wA:t2" },
  })
  vim.env.HERDR_TAB_ID = previous
  T.eq(a.pane_id, "wA:p2")
end)

T.test("agents: resolve returns nil when the tab is ambiguous", function()
  local previous = vim.env.HERDR_TAB_ID
  vim.env.HERDR_TAB_ID = "wA:t1"
  local a = agents.resolve({
    { pane_id = "wA:p1", tab_id = "wA:t1" },
    { pane_id = "wA:p2", tab_id = "wA:t1" },
  })
  vim.env.HERDR_TAB_ID = previous
  T.eq(a, nil)
end)

T.test("agents: resolve returns nil when no tab context disambiguates", function()
  local previous = vim.env.HERDR_TAB_ID
  vim.env.HERDR_TAB_ID = nil
  local a = agents.resolve({
    { pane_id = "wA:p1", tab_id = "wA:t1" },
    { pane_id = "wB:p2", tab_id = "wB:t1" },
  })
  vim.env.HERDR_TAB_ID = previous
  T.eq(a, nil)
end)

T.test("agents: display row leads with agent kind", function()
  local row = agents.display({ kind = "pi", title = "π - a", status = "idle", cwd = "/x/y/proj" })
  T.eq(row, "pi · idle · proj")
end)
