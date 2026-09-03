local hn = require("herdr-nvim")
local comments = require("herdr-nvim.comments")
local commands = require("herdr-nvim.commands")

local function clear_cmd()
  pcall(vim.api.nvim_del_user_command, "Herdr")
end

T.test("commands: register creates :Herdr and is idempotent", function()
  commands.register()
  T.eq(vim.fn.exists(":Herdr"), 2)
  commands.register() -- must not error
  T.eq(vim.fn.exists(":Herdr"), 2)
  local info = vim.api.nvim_get_commands({}).Herdr
  T.eq(info.nargs, "?")
  T.ok(info.range, "command accepts a range")
end)

T.test("commands: register replaces a pre-existing placeholder", function()
  clear_cmd()
  vim.api.nvim_create_user_command("Herdr", function() end, { nargs = "*" })
  commands.register()
  T.eq(vim.api.nvim_get_commands({}).Herdr.nargs, "?")
end)

T.test("commands: complete filters subcommands by lead", function()
  T.eq(commands.complete("", "Herdr "), { "comment", "list", "send", "submit" })
  T.eq(commands.complete("s", "Herdr s"), { "send", "submit" })
  T.eq(commands.complete("c", "Herdr c"), { "comment" })
  T.eq(commands.complete("co", "Herdr co"), { "comment" })
  T.eq(commands.complete("x", "Herdr x"), {})
end)

T.test("commands: complete honors a range and stops after a subcommand", function()
  T.eq(commands.complete("s", "'<,'>Herdr s"), { "send", "submit" })
  T.eq(commands.complete("s", "5,10Herdr s"), { "send", "submit" })
  T.eq(commands.complete("", "Herdr send "), {})
end)

T.test("commands: run comment forwards the command range", function()
  local got
  local orig = hn.comment_range
  hn.comment_range = function(s, e) got = { s, e } end
  commands.run({ fargs = { "comment" }, line1 = 2, line2 = 4 })
  hn.comment_range = orig
  T.eq(got, { 2, 4 })
end)

T.test("commands: run send/submit map to the send_all submit flag", function()
  local got
  local orig = hn.send_all
  hn.send_all = function(opts) got = opts end
  commands.run({ fargs = { "send" } })
  T.eq(got, { submit = false })
  commands.run({ fargs = { "submit" } })
  T.eq(got, { submit = true })
  hn.send_all = orig
end)

T.test("commands: run list maps to list_comments", function()
  local got
  local orig = hn.list_comments
  hn.list_comments = function() got = true end
  commands.run({ fargs = { "list" } })
  T.eq(got, true)
  hn.list_comments = orig
end)

T.test("commands: run notifies on missing or unknown subcommand", function()
  local orig = vim.notify
  local msgs = {}
  vim.notify = function(msg, level) msgs[#msgs + 1] = { msg, level } end
  commands.run({ fargs = {} })
  commands.run({ fargs = { "bogus" } })
  vim.notify = orig
  T.eq(#msgs, 2)
  T.eq(msgs[2][2], vim.log.levels.ERROR)
  T.ok(msgs[2][1]:find("unknown subcommand"))
end)

T.test("commands: :Herdr comment annotates the cursor line", function()
  comments.clear()
  commands.register()
  local ui = require("herdr-nvim.ui")
  local orig = ui.input_comment
  ui.input_comment = function(cb) cb("stub") end
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "one", "two", "three" })
  vim.api.nvim_set_current_buf(b)
  vim.api.nvim_win_set_cursor(0, { 2, 0 })
  vim.cmd("Herdr comment") -- no range → cursor line
  ui.input_comment = orig
  local list = comments.list()
  T.eq({ list[1].start_line, list[1].end_line }, { 2, 2 }, "default range is the cursor line")
end)

T.test("commands: :<range>Herdr comment annotates the range", function()
  comments.clear()
  commands.register()
  local ui = require("herdr-nvim.ui")
  local orig = ui.input_comment
  ui.input_comment = function(cb) cb("stub") end
  local b = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "a", "b", "c", "d" })
  vim.api.nvim_set_current_buf(b)
  vim.cmd("1,3Herdr comment")
  ui.input_comment = orig
  local list = comments.list()
  T.eq({ list[1].start_line, list[1].end_line }, { 1, 3 })
end)

T.test("commands: setup() ensures the command exists", function()
  clear_cmd()
  hn.setup({})
  T.eq(vim.fn.exists(":Herdr"), 2)
end)
