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
  local list, err = agents.list(fake_exec(fixture))
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

T.test("agents: current workspace sorts first", function()
  vim.env.HERDR_WORKSPACE_ID = "wB"
  local list = agents.list(fake_exec(fixture))
  T.eq(list[1].pane_id, "wB:p2")
  vim.env.HERDR_WORKSPACE_ID = nil
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

T.test("agents: display row", function()
  local row = agents.display({ title = "π - a", status = "idle", cwd = "/x/y/proj" })
  T.eq(row, "π - a · idle · proj")
end)
