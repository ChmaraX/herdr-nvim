-- Minimal test harness. Run: nvim --headless --noplugin -u NONE -l tests/run.lua
local root = vim.fs.dirname(vim.fs.dirname(debug.getinfo(1, "S").source:sub(2)))
package.path = root .. "/lua/?.lua;" .. root .. "/lua/?/init.lua;" .. package.path

_G.T = { _tests = {} }
function T.test(name, fn) table.insert(T._tests, { name = name, fn = fn }) end
function T.eq(actual, expected, msg)
  if not vim.deep_equal(actual, expected) then
    error(string.format("%s\nexpected: %s\ngot:      %s",
      msg or "not equal", vim.inspect(expected), vim.inspect(actual)), 2)
  end
end
function T.ok(cond, msg) if not cond then error(msg or "expected truthy", 2) end end

local files = vim.fn.glob(root .. "/tests/test_*.lua", false, true)
for _, f in ipairs(files) do dofile(f) end

local failed = 0
for _, t in ipairs(T._tests) do
  local ok, err = pcall(t.fn)
  if ok then print("PASS " .. t.name)
  else failed = failed + 1; print("FAIL " .. t.name .. ": " .. tostring(err)) end
end
print(string.format("%d/%d passed", #T._tests - failed, #T._tests))
os.exit(failed == 0 and 0 or 1)
