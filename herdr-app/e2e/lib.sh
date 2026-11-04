# Helpers of run.sh's checks (sourced once the app, the tabs and the install
# are set up; every value they read is run.sh's, read when called).

probe() { "$py" "$here/probe.py" "$app_bin"; }
panel_is() { [ "$(probe | jget panel)" = "$1" ]; }
files_button_off() { H do layout.hide '{"id":"panel"}' >/dev/null; } # the button's other half
no_sidebar_of() { ! sidebar_of "$1" >/dev/null; }
pair_off() { poll 15 no_sidebar_of "$tab_a"; }
# No file explorer window of nvim's own (netrw, snacks, nvim-tree, neo-tree, oil).
no_explorer() {
  ! nvim_eval "$1" 'join(map(nvim_list_wins(), {_,w -> getbufvar(winbufnr(w), "&filetype")}), ",")' |
    grep -Eq 'netrw|snacks_picker_list|NvimTree|neo-tree|oil|minifiles'
}

# The picker's terminal (cmd-o's dialog).
picker_term() {
  for t in $(hc ls --json | "$py" -c 'import json,sys; [print(t["address"]) for t in json.load(sys.stdin)["terminals"]]'); do
    hc get "$t" --json | grep -q '"picker"' && { echo "$t"; return 0; }
  done
  return 1
}

# The tree, driven as a person does: what cmd-shift-e runs, then keys.
open_in_nvim() { "$wrapper" open "$1" --ctx "$ctx" >/dev/null; }
nvim_on() { [ "$(nvim_eval "$sock_a" 'expand("%:t")')" = "$1" ]; }
dialog_open() { H status | tr -d ' \n' | grep -q '"overlays":\["herdr-nvim"'; }
no_dialog() { ! dialog_open; }
gone_buffer() { [ "$(nvim_eval "$sock_a" "bufnr('$1')")" = "-1" ]; }
tree_key() { keys "$@"; sleep 0.4; } # paced like a person typing
into_tree() { sleep 0.5; H do herdr-nvim.reveal >/dev/null; sleep 1; } # what cmd-shift-e runs (keys.json)
# What nvim tells the tree and the pane header (app.lua write_state) names $1.
state_names() { grep -q "$1" "$(nvim_eval "$sock_a" '$HERDR_NVIM_CURRENT_FILE')"; }

# Preview / Source: tab A's split tree with sizes, nvim's pane or the
# preview as SLOT; the file the preview shows.
tab_shape() {
  "$py" - "$tab_a" "$(sidebar_of "$tab_a" || true)" <<'PY'
import json, subprocess, sys
out = subprocess.run(["hc", "connect"], capture_output=True, text=True, timeout=5,
    input='{"jsonrpc":"2.0","id":1,"method":"hello","params":{}}\n{"jsonrpc":"2.0","id":2,"method":"doc.get","params":{"name":"herdr-app"}}\n').stdout
doc = next(json.loads(l) for l in out.splitlines() if json.loads(l).get("id") == 2)["result"]["value"]
tab = next(t for s in doc["spaces"] for t in s["tabs"] if t["id"] == sys.argv[1])
def walk(n):
    c = n.get("content") or {}
    # The slot: the preview, or nvim's terminal (argv 2, found by its label).
    slot = c.get("mod") == "herdr-nvim" or (c.get("terminal") and c.get("terminal") == sys.argv[2])
    kids = [walk(k) for k in n.get("children", [])]
    if n.get("type") in ("row", "column") and len(kids) == 1: return kids[0] | {"size": n.get("size")}
    return {"type": n.get("type"), "what": "SLOT" if slot else c.get("terminal"), "size": n.get("size"), "kids": kids}
print(json.dumps(walk(tab["root"]), sort_keys=True))
PY
}
preview_pane() { "$py" - "$tab_a" <<'PY'
import json, subprocess, sys
out = subprocess.run(["hc", "connect"], capture_output=True, text=True, timeout=5,
    input='{"jsonrpc":"2.0","id":1,"method":"hello","params":{}}\n{"jsonrpc":"2.0","id":2,"method":"doc.get","params":{"name":"herdr-app"}}\n').stdout
doc = next(json.loads(l) for l in out.splitlines() if json.loads(l).get("id") == 2)["result"]["value"]
tab = next(t for s in doc["spaces"] for t in s["tabs"] if t["id"] == sys.argv[1])
def walk(n):
    c = n.get("content") or {}
    if c.get("mod") == "herdr-nvim" and c.get("props", {}).get("preview"): return c["props"]["preview"]
    return next((f for f in map(walk, n.get("children", [])) if f), None)
f = walk(tab["root"]); print(f or ""); sys.exit(0 if f else 1)
PY
}
has_preview() { preview_pane >/dev/null; }
no_preview() { ! preview_pane >/dev/null; }
preview_is() { preview_pane | grep -q "$1"; }
enc() { "$py" -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe="-_.!~*()"))' "$1"; }
