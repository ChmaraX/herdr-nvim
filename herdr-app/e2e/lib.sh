# Helpers of run.sh used by more than one group of checks (sourced once
# the app, the tabs and the install are set up; every value they read is
# run.sh's, read when called).

probe() { "$py" "$here/probe.py" "$app_bin"; }

panel_is() { [ "$(probe | jget panel)" = "$1" ]; }

no_explorer() { # no file explorer window of nvim's own (netrw, snacks, nvim-tree, neo-tree, oil)
  ! nvim_eval "$1" 'join(map(nvim_list_wins(), {_,w -> getbufvar(winbufnr(w), "&filetype")}), ",")' |
    grep -Eq 'netrw|snacks_picker_list|NvimTree|neo-tree|oil|minifiles'
}

files_button_off() { H do layout.hide '{"id":"panel"}' >/dev/null; } # the button's other half

picker_term() {
  for t in $(hc ls --json | "$py" -c 'import json,sys; [print(t["address"]) for t in json.load(sys.stdin)["terminals"]]'); do
    hc get "$t" --json | grep -q '"picker"' && { echo "$t"; return 0; }
  done
  return 1
}

no_overlay() { ! H status | tr -d ' \n' | grep -q '"overlays":\["'; }

open_in_nvim() { "$wrapper" open "$1" --ctx "$ctx_a" >/dev/null; }

nvim_on() { [ "$(nvim_eval "$sock_a" 'expand("%:t")')" = "$1" ]; }

dialog_open() { H status | tr -d ' \n' | grep -q '"overlays":\["herdr-nvim"'; }

gone_buffer() { [ "$(nvim_eval "$sock_a" "bufnr('$1')")" = "-1" ]; }

tree_key() { keys "$@"; sleep 0.4; } # paced like a person typing

into_tree() { keys 14:cmd,shift; sleep 1; } # cmd-shift-e

focus_shot() { # focus_shot <name> tree|nvim: exactly one highlighted, and the right one
  # In the 1280-wide window the agent pane starts at ~0.26 of the width, nvim
  # at ~0.49 and the 368 px panel (the tree) at ~0.71.
  H shot "$proof/$1.png" >/dev/null
  read -r count at < <("$py" "$here/highlight.py" "$proof/$1.png")
  [ "$count" = 1 ] || fail "$1: $count panes look focused"
  # With the panel hidden (wide), nvim starts at ~0.63.
  if [ "$2" = tree ]; then awk "BEGIN{exit !($at > 0.62)}" || fail "$1: the focused one is not the tree ($at)"
  elif [ "$2" = wide ]; then awk "BEGIN{exit !($at > 0.55 && $at < 0.7)}" || fail "$1: the focused one is not the wide nvim ($at)"
  else awk "BEGIN{exit !($at > 0.38 && $at < 0.62)}" || fail "$1: the focused one is not nvim ($at)"; fi
}

pair_off() { poll 15 sh -c "! hc ls --json --label herdr-nvim.sidebar=$tab_a | grep -q '\"live\"'"; }

restart_pair() { # close the pair, stop tab A's nvim, open the pair: nvim reads its config again
  if sidebar_of "$tab_a" >/dev/null; then cmd_e; pair_off || fail "cmd-e did not close the pair"; fi
  "$wrapper" daemons stop "$space:$tab_a" --force >/dev/null
  cmd_e; poll 15 sidebar_of "$tab_a" || fail "cmd-e did not reopen nvim"
  sidebar_a=$(sidebar_of "$tab_a"); sock_a=$(sock_of "$space" "$tab_a") || fail "no daemon for tab A"
  poll 10 test -S "$sock_a" || fail "no daemon socket"
}

leader_e_desc() { nvim_eval "$sock_a" 'get(maparg("<leader>e","n",0,1), "desc", "")'; }

leader_e_is() { [ "$(leader_e_desc)" = "$1" ]; }

stamp_of() { nvim_eval "$sock_a" '$HERDR_NVIM_RUNTIME_STAMP'; }

go_left() { keys 38:cmd text:h; sleep 0.8; } # cmd-j h

state_names() { grep -q "zz-dirty.txt" "$(nvim_eval "$sock_a" '$HERDR_NVIM_CURRENT_FILE')"; }

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

no_sidebar() { ! sidebar_of "$tab_a" >/dev/null; }

has_preview() { preview_pane >/dev/null; }

no_preview() { ! preview_pane >/dev/null; }

preview_is() { preview_pane | grep -q "$1"; }

enc() { "$py" -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe="-_.!~*()"))' "$1"; }

install_into() { # install_into <expected layout> <expected line>
  local out; out=$("$repo/herdr-app/install.sh" --app-bin "$app_bin" 2>&1 | tee -a "$log")
  echo "$out" | grep -q "$2" || fail "install: expected '$2'"
  poll 10 shown_is "$1" || fail "install did not show $1 (shows $(shown_layout))"
  H status | grep -q '"errors": \[\]' || fail "the app reports errors: $(H status)"
}

uninstall_back() { # uninstall_back <layout> <saved copy> <layouts before>
  "$repo/herdr-app/install.sh" --uninstall --app-bin "$app_bin" 2>&1 | tee -a "$log"
  ldiff --same "$2" "$layouts/$1.json" || fail "uninstall did not put $1 back as it was"
  [ "$(layouts_now)" = "$3" ] || fail "uninstall deleted or added a layout: $(layouts_now), was $3"
  shown_is "$1" || fail "uninstall switched away from $1"
  H status | grep -q '"errors": \[\]' || fail "the app reports errors: $(H status)"
}

no_sidebar_of() { ! sidebar_of "$1" >/dev/null; }
