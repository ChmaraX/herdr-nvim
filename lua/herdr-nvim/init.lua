local M = {}
local comments = require("herdr-nvim.comments")
local prompt = require("herdr-nvim.prompt")
local agents = require("herdr-nvim.agents")
local dispatch = require("herdr-nvim.dispatch")
local ui = require("herdr-nvim.ui")

M.config = { prefix = "<leader>a", keymaps = true, clear_after_send = true }

local function map(mode, lhs, rhs, desc)
  if vim.fn.maparg(vim.api.nvim_replace_termcodes(lhs, true, true, true), mode) ~= "" then
    vim.notify("herdr-nvim: not overriding existing map " .. lhs, vim.log.levels.WARN)
    return
  end
  vim.keymap.set(mode, lhs, rhs, { desc = desc })
end

function M.setup(config)
  M.config = vim.tbl_deep_extend("force", M.config, config or {})
  -- Ensure :Herdr is registered (also done from plugin/herdr-nvim.lua).
  require("herdr-nvim.commands").register()
  -- Default keymaps stay opt-out (keymaps = true) for backward compatibility.
  -- Prefer the :Herdr command or the Lua API and set keymaps = false to bind
  -- your own; see :help herdr-nvim.
  if M.config.keymaps then
    local p = M.config.prefix
    map("x", p .. "c", function() M.comment_selection() end, "herdr-nvim: comment selection")
    map("n", p .. "c", function() M.comment_line() end, "herdr-nvim: comment line")
    map("n", p .. "l", function() M.list_comments() end, "herdr-nvim: list comments")
    map("n", p .. "s", function() M.send_all({ submit = false }) end, "herdr-nvim: paste comments to agent")
    map("n", p .. "S", function() M.send_all({ submit = true }) end, "herdr-nvim: send comments to agent")
    map("x", p .. "i", function() M.ref_selection() end, "herdr-nvim: reference selection at agent cursor")
    map("n", p .. "i", function() M.ref_line() end, "herdr-nvim: reference line at agent cursor")
  end
end

-- Range primitive behind comment_line(), comment_selection(), and :Herdr comment.
function M.comment_range(start_line, end_line)
  local bufnr = vim.api.nvim_get_current_buf()
  ui.input_comment(function(text)
    local id = comments.add(bufnr, start_line, end_line, text)
    ui.decorate(id)
  end)
end

function M.comment_selection()
  vim.cmd([[execute "normal! \<esc>"]]) -- materialize '< '> marks
  local s, e = ui.visual_range()
  M.comment_range(s, e)
end

function M.comment_line()
  local l = vim.api.nvim_win_get_cursor(0)[1]
  M.comment_range(l, l)
end

-- Edit a comment's text in place (undecorate → edit → re-decorate so the callout
-- reflects the new text). `on_done` (optional) fires after the input closes.
function M.edit_comment(c, on_done)
  vim.ui.input({ prompt = "Edit comment: ", default = c.text }, function(t)
    if t and t ~= "" and t ~= c.text then
      ui.undecorate(c.id)
      comments.edit(c.id, t)
      ui.decorate(c.id)
    end
    if on_done then
      on_done()
    end
  end)
end

function M.delete_comment(c)
  ui.undecorate(c.id)
  comments.delete(c.id)
end

-- Interactive list: hover auto-jumps to each comment, <CR> edits, `d` deletes,
-- `q`/<Esc> closes. No secondary jump/edit/delete menu.
function M.list_comments()
  ui.comment_list({
    edit = function(c, refresh)
      M.edit_comment(c, refresh)
    end,
    delete = function(c)
      M.delete_comment(c)
    end,
  })
end

function M._git_context(cwd)
  local ok, r = pcall(function()
    return vim.system({ "git", "rev-parse", "--show-toplevel", "--abbrev-ref", "HEAD" },
      { text = true, cwd = cwd }):wait()
  end)
  if not ok or r.code ~= 0 then return nil end
  local root, branch = r.stdout:match("([^\n]*)\n([^\n]*)")
  if not root then return nil end
  return string.format("repo: %s, branch: %s",
    vim.fn.fnamemodify(vim.trim(root), ":t"), vim.trim(branch))
end

-- Single funnel for every send (pending comments and bare references alike), so
-- the agent resolution, the picker fallback, and the "agent is working" warning
-- all live in exactly one place. `text` is either the payload or a
-- function(agent) returning it -- a reference needs the resolved agent's cwd to
-- shorten its path. `on_sent(agent)` runs only after a successful dispatch.
function M._deliver_to_agent(text, opts, on_sent)
  local agent_list, err = agents.list()
  if not agent_list then
    vim.notify("herdr-nvim: " .. err, vim.log.levels.ERROR)
    return
  end
  local function deliver(agent)
    if agent.status == "working" then
      vim.notify("herdr-nvim: " .. agents.display(agent) .. " is working — sending anyway", vim.log.levels.WARN)
    end
    local payload = type(text) == "function" and text(agent) or text
    local ok, derr = dispatch.send(agent.pane_id, payload, opts)
    if not ok then
      vim.notify("herdr-nvim: " .. derr, vim.log.levels.ERROR)
      return
    end
    on_sent(agent)
  end
  -- Skip the picker when the target is unambiguous (the common one-agent case);
  -- fall back to the picker only when 2+ agents could plausibly be meant.
  local agent = agents.resolve(agent_list)
  if agent then
    deliver(agent)
  else
    ui.pick_agent(agent_list, deliver)
  end
end

function M.send_all(opts)
  local list = comments.list()
  if #list == 0 then
    vim.notify("herdr-nvim: no comments to send", vim.log.levels.INFO)
    return
  end
  local items = {}
  for _, c in ipairs(list) do
    table.insert(items, { comment = c, snippet = comments.snippet(c.id) })
  end
  local first_file = list[1].file
  local cwd = first_file ~= "" and vim.fn.fnamemodify(first_file, ":h") or nil
  local text = prompt.format(items, { header_context = M._git_context(cwd) })
  M._deliver_to_agent(text, opts, function(agent)
    if M.config.clear_after_send then
      for _, c in ipairs(list) do
        M.delete_comment(c)
      end
    end
    vim.notify(string.format("herdr-nvim: sent %d comment(s) to %s", #list, agent.title))
  end)
end

-- Range primitive behind ref_line(), ref_selection(), and :Herdr ref. Sends a
-- bare `path:line` citation and nothing else -- no comment, no code, no header,
-- and never a submit -- so it lands in the middle of a message you are still
-- typing. Pending comments are untouched.
function M.ref_range(start_line, end_line)
  local bufnr = vim.api.nvim_get_current_buf()
  local file = vim.api.nvim_buf_get_name(bufnr)
  if file == "" then
    vim.notify("herdr-nvim: buffer has no file to reference", vim.log.levels.WARN)
    return
  end
  if start_line > end_line then
    start_line, end_line = end_line, start_line
  end
  local count = vim.api.nvim_buf_line_count(bufnr)
  start_line = math.max(1, math.min(start_line, count))
  end_line = math.max(1, math.min(end_line, count))
  -- A reference points at the file on disk, so unwritten changes are invisible
  -- to whoever opens it. Worth saying out loud; not worth blocking over.
  if vim.bo[bufnr].modified then
    vim.notify("herdr-nvim: buffer has unsaved changes — the agent reads the file on disk",
      vim.log.levels.WARN)
  end
  local item = { file = file, start_line = start_line, end_line = end_line }
  -- Deferred: the path is shortened against the cwd of whichever agent is
  -- resolved, which the picker may not settle until after this returns.
  local function payload(agent)
    return prompt.format_ref(item, { cwd = agent.cwd })
  end
  M._deliver_to_agent(payload, { submit = false }, function(agent)
    vim.notify("herdr-nvim: referenced " .. vim.trim(prompt.format_ref(item, { cwd = agent.cwd })))
  end)
end

function M.ref_selection()
  vim.cmd([[execute "normal! \<esc>"]]) -- materialize '< '> marks
  local s, e = ui.visual_range()
  M.ref_range(s, e)
end

function M.ref_line()
  local l = vim.api.nvim_win_get_cursor(0)[1]
  M.ref_range(l, l)
end

function M.statusline()
  local n = #comments.list()
  return n == 0 and "" or ("● " .. n)
end

return M
