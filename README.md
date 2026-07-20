# herdr-nvim

Annotate code in nvim, send the annotations to any AI agent running in
[herdr](https://herdr.dev) — without leaving your editor.

## Requirements

- nvim ≥ 0.10, herdr ≥ 0.7.0, running inside a herdr session

## Install (lazy.nvim)

```lua
{ "adamchmara/herdr-nvim", opts = {} }
```

## Usage

| Mapping | Action |
|---|---|
| `<leader>ac` (visual) | comment the selection |
| `<leader>ac` (normal) | comment the current line |
| `<leader>al` | list comments — jump / edit / delete |
| `<leader>as` | paste all comments into a chosen agent's input (you press Enter) |
| `<leader>aS` | send all comments to a chosen agent (auto-submits) |

## Config

```lua
require("herdr-nvim").setup({
  prefix = "<leader>a",     -- keymap prefix
  keymaps = true,           -- set false to define your own
  clear_after_send = true,  -- comments are ephemeral by design
})
```

Comments live in memory only (extmark-tracked, so they follow your edits) and
are cleared after a successful send.

## Tests

`nvim --headless --noplugin -u NONE -l tests/run.lua`
