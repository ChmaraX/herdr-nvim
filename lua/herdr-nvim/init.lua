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
  if M.config.keymaps then
    local p = M.config.prefix
    map("x", p .. "c", function() M.comment_selection() end, "herdr-nvim: comment selection")
    map("n", p .. "c", function() M.comment_line() end, "herdr-nvim: comment line")
    map("n", p .. "l", function() M.list_comments() end, "herdr-nvim: list comments")
    map("n", p .. "s", function() M.send_all({ submit = false }) end, "herdr-nvim: paste comments to agent")
    map("n", p .. "S", function() M.send_all({ submit = true }) end, "herdr-nvim: send comments to agent")
  end
end

local function add_comment(start_line, end_line)
  local bufnr = vim.api.nvim_get_current_buf()
  ui.input_comment(function(text)
    local id = comments.add(bufnr, start_line, end_line, text)
    ui.decorate(id)
  end)
end

function M.comment_selection()
  vim.cmd([[execute "normal! \<esc>"]]) -- materialize '< '> marks
  local s, e = ui.visual_range()
  add_comment(s, e)
end

function M.comment_line()
  local l = vim.api.nvim_win_get_cursor(0)[1]
  add_comment(l, l)
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
  local agent_list, err = agents.list()
  if not agent_list then
    vim.notify("herdr-nvim: " .. err, vim.log.levels.ERROR)
    return
  end
  -- Single funnel for every send (both the resolved and picked paths), so the
  -- "agent is working" warning lives in exactly one place.
  local function deliver(agent)
    if agent.status == "working" then
      vim.notify("herdr-nvim: " .. agents.display(agent) .. " is working — sending anyway", vim.log.levels.WARN)
    end
    local ok, derr = dispatch.send(agent.pane_id, text, opts)
    if not ok then
      vim.notify("herdr-nvim: " .. derr, vim.log.levels.ERROR)
      return
    end
    if M.config.clear_after_send then
      for _, c in ipairs(list) do
        M.delete_comment(c)
      end
    end
    vim.notify(string.format("herdr-nvim: sent %d comment(s) to %s", #list, agent.title))
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

function M.statusline()
  local n = #comments.list()
  return n == 0 and "" or ("● " .. n)
end

return M
