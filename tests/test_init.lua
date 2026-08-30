local hn = require("herdr-nvim")
local comments = require("herdr-nvim.comments")

T.test("init: setup creates guarded keymaps", function()
  vim.g.mapleader = " "
  vim.keymap.set("n", " ac", "<cmd>echo 'user owns this'<cr>") -- simulate user mapping
  hn.setup({})
  T.ok(vim.fn.maparg(" ac", "n"):match("user owns this"), "must not clobber user map")
  T.ok(vim.fn.maparg(" al", "n") ~= "", "free lhs must be mapped")
  vim.keymap.del("n", " ac")
end)

T.test("init: a second setup does not warn about maps it already owns", function()
  vim.g.mapleader = " "
  local warns = {}
  local orig = vim.notify
  vim.notify = function(msg, level)
    if level == vim.log.levels.WARN then table.insert(warns, msg) end
  end
  hn.setup({})
  hn.setup({})
  vim.notify = orig
  T.eq(warns, {}, "second setup must not warn about its own maps")
  T.ok(vim.fn.maparg(" al", "n") ~= "", "maps still present after the second setup")
end)

T.test("init: comment_line adds a decorated comment via stubbed input", function()
  comments.clear()
  local ui = require("herdr-nvim.ui")
  local orig = ui.input_comment
  ui.input_comment = function(cb) cb("stub comment") end
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "one", "two" })
  vim.api.nvim_set_current_buf(b)
  vim.api.nvim_win_set_cursor(0, { 2, 0 })
  hn.comment_line()
  ui.input_comment = orig
  local l = comments.list()
  T.eq(#l, 1)
  T.eq({ l[1].start_line, l[1].text }, { 2, "stub comment" })
end)

T.test("init: edit_comment updates text and refreshes its callout", function()
  comments.clear()
  local ui = require("herdr-nvim.ui")
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "alpha" })
  local id = comments.add(b, 1, 1, "old text")
  ui.decorate(id)
  local c = comments.get(id)

  local original_input = vim.ui.input
  vim.ui.input = function(_, cb) cb("new text") end
  hn.edit_comment(c)
  vim.ui.input = original_input

  T.eq(comments.get(id).text, "new text")
  local marks = vim.api.nvim_buf_get_extmarks(b, comments.ns, 0, -1, { details = true })
  local callout_text
  for _, mark in ipairs(marks) do
    if mark[4].virt_lines then callout_text = mark[4].virt_lines[1][2][1] end
  end
  T.ok(callout_text and callout_text:find("new text", 1, true), "callout shows edited text")
end)

T.test("init: git context returns nil when git cannot spawn", function()
  local original = vim.system
  vim.system = function() error("ENOENT: git") end
  local ok, context = pcall(hn._git_context)
  vim.system = original
  T.ok(ok)
  T.eq(context, nil)
end)

T.test("init: send_all formats, dispatches, clears", function()
  comments.clear()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "alpha" })
  vim.api.nvim_buf_set_name(b, "/tmp/hn-send.lua")
  comments.add(b, 1, 1, "check this")

  local ui = require("herdr-nvim.ui")
  local dispatch = require("herdr-nvim.dispatch")
  local agents = require("herdr-nvim.agents")
  local sent = {}
  local o1, o2, o3 = ui.pick_agent, dispatch.send, agents.list
  ui.pick_agent = function(_, cb) cb({ pane_id = "wZ:p9", title = "π", status = "idle" }) end
  dispatch.send = function(pane, text, opts) sent = { pane, text, opts }; return true end
  agents.list = function() return { { pane_id = "wZ:p9", title = "π", status = "idle" } } end

  hn.send_all({ submit = false })
  ui.pick_agent, dispatch.send, agents.list = o1, o2, o3

  T.eq(sent[1], "wZ:p9")
  T.ok(sent[2]:find("1. " .. vim.api.nvim_buf_get_name(b) .. ":1-1", 1, true))
  T.ok(sent[2]:find("> alpha", 1, true))
  T.eq(sent[3].submit, false)
  T.eq(comments.list(), {}, "clear_after_send default clears comments")
end)

T.test("init: send_all warns once when the resolved agent is working", function()
  comments.clear()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "alpha" })
  vim.api.nvim_buf_set_name(b, "/tmp/hn-send-working.lua")
  comments.add(b, 1, 1, "check this")

  local ui = require("herdr-nvim.ui")
  local dispatch = require("herdr-nvim.dispatch")
  local agents = require("herdr-nvim.agents")
  local warns = {}
  local o1, o2, o3, on = ui.pick_agent, dispatch.send, agents.list, vim.notify
  ui.pick_agent = function() error("picker must not open for a lone agent") end
  dispatch.send = function() return true end
  agents.list = function() return { { pane_id = "wZ:p9", tab_id = "wZ:t1", kind = "pi", title = "pi", status = "working", cwd = "/x/y/z" } } end
  vim.notify = function(msg, level) if level == vim.log.levels.WARN then table.insert(warns, msg) end end

  hn.send_all({ submit = true })
  ui.pick_agent, dispatch.send, agents.list, vim.notify = o1, o2, o3, on

  T.eq(#warns, 1, "working warning must fire exactly once")
  T.ok(warns[1]:find("is working", 1, true), "warning names the working state")
end)

T.test("init: send_all shows the picker when agents are ambiguous", function()
  comments.clear()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "alpha" })
  vim.api.nvim_buf_set_name(b, "/tmp/hn-send-multi.lua")
  comments.add(b, 1, 1, "check this")

  local ui = require("herdr-nvim.ui")
  local dispatch = require("herdr-nvim.dispatch")
  local agents = require("herdr-nvim.agents")
  local previous = vim.env.HERDR_TAB_ID
  vim.env.HERDR_TAB_ID = nil
  local picked, sent = false, {}
  local o1, o2, o3 = ui.pick_agent, dispatch.send, agents.list
  ui.pick_agent = function(l, cb) picked = true; cb(l[1]) end
  dispatch.send = function(pane) sent = { pane }; return true end
  agents.list = function()
    return {
      { pane_id = "wA:p1", tab_id = "wA:t1", title = "pi", status = "idle" },
      { pane_id = "wB:p2", tab_id = "wB:t1", title = "claude", status = "idle" },
    }
  end

  hn.send_all({ submit = false })
  ui.pick_agent, dispatch.send, agents.list = o1, o2, o3
  vim.env.HERDR_TAB_ID = previous

  T.ok(picked, "picker must open when the target is ambiguous")
  T.eq(sent[1], "wA:p1")
end)

T.test("init: send_all skips the picker for a lone agent", function()
  comments.clear()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "alpha" })
  vim.api.nvim_buf_set_name(b, "/tmp/hn-send-solo.lua")
  comments.add(b, 1, 1, "check this")

  local ui = require("herdr-nvim.ui")
  local dispatch = require("herdr-nvim.dispatch")
  local agents = require("herdr-nvim.agents")
  local picked, sent = false, {}
  local o1, o2, o3 = ui.pick_agent, dispatch.send, agents.list
  ui.pick_agent = function() picked = true end
  dispatch.send = function(pane) sent = { pane }; return true end
  agents.list = function() return { { pane_id = "wZ:p9", tab_id = "wZ:t1", title = "pi", status = "idle" } } end

  hn.send_all({ submit = false })
  ui.pick_agent, dispatch.send, agents.list = o1, o2, o3

  T.ok(not picked, "picker must not open for a single unambiguous agent")
  T.eq(sent[1], "wZ:p9")
end)

T.test("init: send_all retains comments when dispatch.send fails", function()
  comments.clear()
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "alpha" })
  vim.api.nvim_buf_set_name(b, "/tmp/hn-send-fail.lua")
  comments.add(b, 1, 1, "check this")

  local ui = require("herdr-nvim.ui")
  local dispatch = require("herdr-nvim.dispatch")
  local agents = require("herdr-nvim.agents")
  local o1, o2, o3 = ui.pick_agent, dispatch.send, agents.list
  ui.pick_agent = function(_, cb) cb({ pane_id = "wZ:p9", title = "π", status = "idle" }) end
  dispatch.send = function() return false, "boom" end
  agents.list = function() return { { pane_id = "wZ:p9", title = "π", status = "idle" } } end

  hn.send_all({ submit = false })
  ui.pick_agent, dispatch.send, agents.list = o1, o2, o3

  T.eq(#comments.list(), 1, "comments must be retained after a failed send")
end)

T.test("init: statusline reflects pending comment count", function()
  comments.clear()
  T.eq(hn.statusline(), "")
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "x" })
  comments.add(b, 1, 1, "a")
  comments.add(b, 1, 1, "b")
  T.eq(hn.statusline(), "● 2")
end)
