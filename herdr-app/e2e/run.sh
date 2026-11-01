#!/usr/bin/env bash
# End-to-end run of herdr-nvim in a private Herdr app (macOS): its own app home,
# herdr-core session, XDG dirs and daemon folders, so the person's own Herdr,
# spaces and terminals are never touched. It installs the mod with
# herdr-app/install.sh, drives it the way a person does (the Files button's
# commands, keys sent to the private window only, the picker, annotations),
# checks the real effects (daemons, buffers, what the agent received), takes
# screenshots, uninstalls, and removes everything it started.
#
#   herdr-app/e2e/run.sh [--proof DIR] [--keep] [--app-bin PATH]
#
# Needs: the Herdr app (0.1.11+), hc, nvim, cargo, git, and a python3 with
# pyobjc (Quartz) for the key presses. The window is moved on screen once
# (System Events, needs Accessibility) because macOS only draws visible windows.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
proof=/tmp/nvim-port-proof/run-$(date +%Y%m%d-%H%M%S)
keep=0
app_bin=
while [ $# -gt 0 ]; do
  case $1 in
    --proof) proof=$2; shift ;;
    --keep) keep=1 ;;
    --app-bin) app_bin=$2; shift ;;
    *) echo "unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done
if [ -z "$app_bin" ]; then
  for bundle in /Applications/*.app "$HOME"/Applications/*.app; do
    [ "$(plutil -extract CFBundleIdentifier raw "$bundle/Contents/Info.plist" 2>/dev/null)" = dev.herdr.app ] || continue
    app_bin="$bundle/Contents/MacOS/$(plutil -extract CFBundleExecutable raw "$bundle/Contents/Info.plist")"
  done
fi
[ -x "$app_bin" ] || { echo "no Herdr app found; pass --app-bin" >&2; exit 2; }
py=
for candidate in python3 /usr/bin/python3 /opt/homebrew/bin/python3 /opt/homebrew/anaconda3/bin/python3; do
  if "$candidate" -c 'import Quartz' 2>/dev/null; then py=$(command -v "$candidate"); break; fi
done
[ -n "$py" ] || { echo "no python3 with pyobjc (Quartz) for key presses" >&2; exit 2; }

mkdir -p "$proof"
log=$proof/run.log
: > "$log"
say() { echo "[$(date +%H:%M:%S)] $*" | tee -a "$log"; }
fail() { say "FAIL: $*"; exit 1; }

# --- the private instance ---------------------------------------------------
root=$(mktemp -d /tmp/hnv-e2e.XXXX)
for v in $(env | grep -oE '^(HERDR_[A-Z_]+|HC_[A-Z_]+)=' | tr -d =); do unset "$v"; done
export HERDR_APP_HOME=$root/home HC_SESSION=hn${root: -4} XDG_STATE_HOME=$root/state
export XDG_CONFIG_HOME=$root/config XDG_DATA_HOME=$root/data
export HERDR_NVIM_RUNTIME_DIR=$root/run HERDR_NVIM_STATE_DIR=$root/nvstate
export HERDR_APP_NO_SOUND=1 HERDR_TELEMETRY=0 DO_NOT_TRACK=1
export HERDR_RELEASES_URL=http://127.0.0.1:9 # never update the installed app or core
export PATH="$HOME/.local/bin:/opt/homebrew/bin:$PATH"
mkdir -p "$HERDR_APP_HOME" "$root/work"
echo '{"frame.welcomed": true, "layout": "main"}' > "$HERDR_APP_HOME/shared.json"
version=$(plutil -extract CFBundleShortVersionString raw "${app_bin%/MacOS/*}/Info.plist" 2>/dev/null || true)
echo "{\"last_run\": \"$version\", \"presented\": []}" > "$HERDR_APP_HOME/update-notes.json" # no "Updated" toast
H() { "$app_bin" "$@"; }
app_pid=

cleanup() {
  set +e
  say "cleanup (root $root)"
  if [ "$keep" = 1 ]; then say "kept: . with HERDR_APP_HOME=$HERDR_APP_HOME HC_SESSION=$HC_SESSION"; return; fi
  [ -n "$app_pid" ] && kill "$app_pid" 2>/dev/null && wait "$app_pid" 2>/dev/null
  for t in $(hc ls --json 2>/dev/null | "$py" -c 'import json,sys; [print(t["address"]) for t in json.load(sys.stdin).get("terminals", [])]'); do
    hc kill "$t" >/dev/null 2>&1
  done
  hc server stop >/dev/null 2>&1
  # A stopped core's holder keeps running, and any hc call restarts a core:
  # end this run's own cores and holders (matched by this run's state dir).
  for p in $(pgrep -f '(hc|herdr-core) (serve|hold)'); do
    ps eww -o command= -p "$p" 2>/dev/null | grep -q "XDG_STATE_HOME=$root/state " && kill "$p"
  done
  # Daemons of this run only: their sockets are in this run's folder.
  for s in "$root"/run/app/*.sock; do
    [ -S "$s" ] && nvim --headless --server "$s" --remote-send '<cmd>qa!<cr>' >/dev/null 2>&1
  done
  rm -rf "$root"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

poll() { # poll <seconds> <command...>: until it succeeds
  local until=$((SECONDS + $1)); shift
  until "$@" >/dev/null 2>&1; do [ $SECONDS -lt $until ] || return 1; sleep 0.2; done
}
keys() { "$py" "$here/keys.py" "$app_pid" "$@"; }
shot() { H shot "$proof/$1.png" >/dev/null && say "screenshot $1.png"; }
files_button() { # exactly what the tab bar's Files button runs (mods/tabs/main.js)
  H do layout.select '{"stack":"panel-body","child":"files"}' >/dev/null
  H do layout.show '{"id":"panel"}' >/dev/null
}
focused_pane() { H status | "$py" -c 'import json,sys; print(json.load(sys.stdin)["focused_pane"])'; }
focus_is() { [ "$(focused_pane)" = "$(pane_of "$1")" ]; }
pane_of() { # the pane showing a terminal
  H ls | "$py" -c 'import json,sys
for s in json.load(sys.stdin)["spaces"]:
  for t in s["tabs"]:
    for x in t["terminals"]:
      if x["terminal"] == sys.argv[1]: print(x["pane"])' "$1"
}
sidebar_of() { # the live sidebar terminal of a tab id
  hc ls --json --label "herdr-nvim.sidebar=$1" | "$py" -c 'import json,sys; t=[t["address"] for t in json.load(sys.stdin)["terminals"] if t["state"]=="live"]; print(t[0] if t else ""); sys.exit(0 if t else 1)'
}
sock_of() { ls "$root"/run/app/*"${2:0:12}".sock 2>/dev/null | head -1; } # sock_of <space> <tab>
bounded() { perl -e 'alarm shift; exec @ARGV' "$@"; } # bounded <seconds> <command...>
nvim_eval() { bounded 5 nvim --headless --server "$1" --remote-expr "$2" 2>/dev/null; }
daemon_pid() { nvim_eval "$1" 'getpid()'; }
nvim_at() { [ "$(nvim_eval "$1" 'expand("%:t") . ":" . line(".")')" = "$2" ]; } # nvim_at <socket> <file:line>
alive() { kill -0 "$1" 2>/dev/null; }

hc server start >/dev/null
"$app_bin" > "$root/app.log" 2>&1 &
app_pid=$!
poll 30 H status || fail "the private app did not come up"
say "private app pid $app_pid, core session $HC_SESSION, home $HERDR_APP_HOME"
place_window() {
  osascript -e "tell application \"System Events\" to tell (first process whose unix id is $app_pid) to set position of window 1 to {40, 60}" \
    -e "tell application \"System Events\" to tell (first process whose unix id is $app_pid) to set size of window 1 to {1280, 820}"
}
poll 15 place_window || say "note: could not move the window (Accessibility); screenshots may be stale"

# --- a repo and two agent tabs ----------------------------------------------
demo=$root/work/demo
mkdir -p "$demo/src"
printf 'export function greet(name) {\n  return `hello ${name}`;\n}\n\nexport function bye(name) {\n  return `bye ${name}`;\n}\n' > "$demo/src/greet.js"
printf '# notes\n\nline three\n' > "$demo/src/notes.md"
printf '{"name":"demo"}\n' > "$demo/package.json"
git -C "$demo" init -q && git -C "$demo" add -A && git -C "$demo" -c user.email=e2e@x -c user.name=e2e commit -qm init
echo 'console.log(1)' >> "$demo/src/greet.js"
agent=(python3 -u "$here/dummy-agent.py")
argv=$("$py" -c 'import json,sys; print(json.dumps(sys.argv[1:]))' "${agent[@]}")
made=$(H do space.new "{\"name\":\"e2e\",\"folder\":\"$demo\",\"tab\":\"tab A\",\"argv\":$argv}")
space=$(echo "$made" | "$py" -c 'import json,sys; print(json.load(sys.stdin)["space"])')
tab_a=$(echo "$made" | "$py" -c 'import json,sys; print(json.load(sys.stdin)["tab"])')
agent_a=$(echo "$made" | "$py" -c 'import json,sys; print(json.load(sys.stdin)["terminal"])')
made=$(H do tab.new "{\"name\":\"tab B\",\"space\":\"$space\",\"folder\":\"$demo\",\"argv\":$argv}")
tab_b=$(echo "$made" | "$py" -c 'import json,sys; print(json.load(sys.stdin)["tab"])')
agent_b=$(echo "$made" | "$py" -c 'import json,sys; print(json.load(sys.stdin)["terminal"])')
poll 20 sh -c "hc read $agent_a | grep -q 'notes.md:3'" || fail "agent A did not start"
say "space $space: tab A $tab_a (agent $agent_a), tab B $tab_b (agent $agent_b)"

# --- install --------------------------------------------------------------
"$repo/herdr-app/install.sh" --app-bin "$app_bin" 2>&1 | tee -a "$log"
wrapper=$HERDR_APP_HOME/mods/herdr-nvim/bin/herdr-nvim
for m in herdr-nvim herdr-nvim-pick; do H check "$HERDR_APP_HOME/mods/$m" | tee -a "$log"; done
again=$("$repo/herdr-app/install.sh" --app-bin "$app_bin" 2>&1 | grep '^layout:' || true)
echo "$again" | grep -q 'already uses herdr-nvim' || fail "a second install is not a no-op: $again"
say "second install: $again"
"$py" - "$HERDR_APP_HOME" <<'EOF' | tee -a "$log" || fail "the clone differs from main elsewhere than the files slot"
import json, sys
home = sys.argv[1]
clone = json.load(open(f"{home}/layouts/main-mods.json"))
main = json.load(open(f"{home}/layouts/main.json"))
def blocks(node, out):
    out[node["id"]] = {k: v for k, v in node.items() if k != "children"}
    for child in node.get("children", []): blocks(child, out)
    return out
a, b = blocks(clone, {}), blocks(main, {})
a["root"].pop("cloned_from", None)
diff = sorted(k for k in set(a) | set(b) if a.get(k) != b.get(k))
print("layout diff main-mods vs main:", diff, "->", a["files"]["content"])
sys.exit(0 if diff == ["files"] else 1)
EOF
H status | grep -q '"errors": \[\]' || fail "the app reports errors: $(H status)"

# An orphan daemon (its tab never existed) for the startup GC to reap.
mkdir -p "$root/run/app"
orphan_sock=$root/run/app/spdeadbeef000_tbdeadbeef000.sock
nvim --headless --listen "$orphan_sock" >/dev/null 2>&1 &
orphan_pid=$!
poll 10 test -S "$orphan_sock" || fail "orphan daemon did not start"
say "orphan daemon pid $orphan_pid (tab that never existed)"

# --- 1. Files button in tab A: the native tree, then a file into nvim -------
H do tab.select "{\"id\":\"$tab_a\"}" >/dev/null
H do pane.focus "{\"terminal\":\"$agent_a\"}" >/dev/null
files_button
panel_shown() { "$py" -c 'import json,sys
def find(n,i):
    if n.get("id")==i: return n
    for c in n.get("children",[]):
        r=find(c,i)
        if r: return r
d=json.load(open(sys.argv[1]+"/layouts/.state/main-mods.json"))
sys.exit(0 if d.get("nodes",{}).get("panel",{}).get("hidden") is False else 1)' "$HERDR_APP_HOME"; }
poll 10 panel_shown || fail "Files did not show the side panel"
H status | grep -q '"errors": \[\]' || fail "the tree mod reports errors: $(H status)"
sleep 1
shot 01-files-native-tree
say "Files: the side panel shows herdr-nvim's native tree of tab A's folder"
# What a click on src/greet.js in the tree runs (main.js activate -> lib.js nvim):
"$wrapper" open "$demo/src/greet.js" --ctx "{\"space\":\"$space\",\"tab\":\"$tab_a\",\"terminal\":\"$agent_a\",\"cwd\":\"$demo\"}" >/dev/null
poll 15 sidebar_of "$tab_a" || fail "opening a file did not open tab A's sidebar"
sidebar_a=$(sidebar_of "$tab_a")
sock_a=$(sock_of "$space" "$tab_a")
pid_a=$(daemon_pid "$sock_a")
poll 10 nvim_at "$sock_a" greet.js:1 || fail "tab A's nvim is not on greet.js"
poll 10 focus_is "$sidebar_a" || fail "focus is not on the new sidebar"
say "tree pick: tab A sidebar $sidebar_a, daemon pid $pid_a, on greet.js, focused"
poll 10 sh -c "! kill -0 $orphan_pid" || fail "the orphan daemon survived the watcher's start"
say "startup GC: orphan daemon $orphan_pid stopped"
sleep 1
shot 02-tree-opened-greet-in-nvim
H do layout.hide '{"id":"panel"}' >/dev/null

# --- 2. key in tab B ------------------------------------------------------
H do tab.select "{\"id\":\"$tab_b\"}" >/dev/null
H do pane.focus "{\"terminal\":\"$agent_b\"}" >/dev/null
keys 38:cmd 14 # cmd-j e
poll 15 sidebar_of "$tab_b" || fail "cmd-j e did not open the sidebar in tab B"
sidebar_b=$(sidebar_of "$tab_b")
poll 15 sh -c "ls $root/run/app/*${tab_b:0:12}.sock" || fail "no daemon for tab B"
sock_b=$(sock_of "$space" "$tab_b")
pid_b=$(daemon_pid "$sock_b")
[ "$pid_a" != "$pid_b" ] || fail "tabs A and B share a daemon"
say "cmd-j e: tab B sidebar $sidebar_b, its own daemon pid $pid_b"
sleep 0.5
shot 03-key-tab-b

# --- 3. picker in tab A ---------------------------------------------------
H do tab.select "{\"id\":\"$tab_a\"}" >/dev/null
H do pane.focus "{\"terminal\":\"$agent_a\"}" >/dev/null
keys 38:cmd 3 # cmd-j f
poll 15 sh -c "\"$app_bin\" status | tr -d ' \n' | grep -q '\"overlays\":\[\"layout\"'" || fail "the picker dialog did not open"
sleep 0.6
shot 04-picker
keys text:note
sleep 0.4
shot 05-picker-filtered
keys enter
poll 15 nvim_at "$sock_a" notes.md:3 ||
  fail "the pick did not open notes.md at line 3 (got $(nvim_eval "$sock_a" 'expand("%:t") . ":" . line(".")'))"
say "picker: tab A's nvim shows $(nvim_eval "$sock_a" 'expand("%:p") . ":" . line(".")')"
poll 10 focus_is "$sidebar_a" || fail "focus did not move to the sidebar"
say "focus is on tab A's sidebar"
sleep 0.5
shot 06-picked-notes-line-3

# --- 4. annotations to agent A ----------------------------------------------
keys esc backslash 0 8 # \ac: comment the line
sleep 0.8
shot 07-comment-prompt
keys "text:fix this" enter
sleep 0.5
keys backslash 0 1:shift # \aS: send all, submitted
poll 30 sh -c "hc read $agent_a | grep -q 'SUBMITTED: Code review comments.*src/notes.md:3.*fix this'" ||
  fail "agent A did not receive the submitted comment: $(hc read "$agent_a" | tail -3)"
say "annotation submitted: $(hc read "$agent_a" | grep SUBMITTED | tail -1)"
keys backslash 0 8 "text:and this" enter
sleep 0.5
keys backslash 0 1 # \as: paste only
poll 20 sh -c "hc read $agent_a | grep -q 'INPUT: .*Comment: and this'" || fail "the pasted comments did not arrive"
hc read "$agent_a" | tail -1 | grep -q SUBMITTED && fail "paste mode submitted"
poll 5 sh -c "hc read $sidebar_a | grep -q 'Press ENTER'" && keys esc # two messages in a row: a hit-enter prompt
say "annotation pasted without submit: $(hc read "$agent_a" | grep 'INPUT:' | tail -1)"
sleep 0.5
shot 08-annotations-received

# --- 5. open-link with the line read back from the agent ----------------------
ctx="{\"space\":\"$space\",\"tab\":\"$tab_a\",\"terminal\":\"$agent_a\",\"cwd\":\"$demo\"}"
"$wrapper" open-link "$demo/src/greet.js" --ctx "$ctx" >/dev/null
poll 10 nvim_at "$sock_a" greet.js:5 ||
  fail "open-link did not land on greet.js:5 (got $(nvim_eval "$sock_a" 'expand("%:t") . ":" . line(".")'))"
say "open-link $demo/src/greet.js (no line given) -> greet.js:5, the line agent A printed"
sleep 0.5
shot 09-open-link-greet-line-5

# --- 5b. the open contract other mods use (gitview's Changes) --------------
"$wrapper" open --tab "$tab_a" src/notes.md:3:6 >/dev/null
poll 10 nvim_at "$sock_a" notes.md:3 || fail "open --tab did not land on notes.md:3"
[ "$(nvim_eval "$sock_a" 'col(".")')" = 6 ] || fail "open --tab ignored the column"
say "open --tab $tab_a src/notes.md:3:6 (panel hidden, no mod involved) -> notes.md:3:6"

# --- 6. toggle off and back: the same daemon ---------------------------------
H do mod.open '{"name":"herdr-nvim"}' >/dev/null
poll 15 sh -c "! hc ls --json --label herdr-nvim.sidebar=$tab_a | grep -q '\"live\"'" || fail "toggle did not close tab A's sidebar"
alive "$pid_a" || fail "closing the sidebar stopped the daemon"
say "toggled off: sidebar gone, daemon $pid_a still running"
H do mod.open '{"name":"herdr-nvim"}' >/dev/null
poll 15 sidebar_of "$tab_a" || fail "the toggle did not reopen tab A's sidebar"
[ "$(daemon_pid "$sock_a")" = "$pid_a" ] || fail "reopened sidebar has a new daemon"
[ "$(nvim_eval "$sock_a" 'expand("%:t")')" = notes.md ] || fail "the daemon lost its buffer"
say "toggle again: reattached to daemon $pid_a, still on $(nvim_eval "$sock_a" 'expand("%:t") . ":" . line(".")')"
sleep 0.8
shot 10-reattached

say "daemons: $("$wrapper" daemons | tr '\n' '|')"

# --- 7. closing tab B stops its daemon ----------------------------------------
H do tab.close "{\"id\":\"$tab_b\",\"confirmed\":true}" >/dev/null
poll 15 sh -c "! kill -0 $pid_b" || fail "tab B's daemon $pid_b survived its tab"
alive "$pid_a" || fail "closing tab B stopped tab A's daemon"
say "closed tab B: daemon $pid_b stopped, tab A's $pid_a still running"

# --- 8. closing the space stops the rest, then the watcher ends ---------------
H do space.close "{\"id\":\"$space\",\"confirmed\":true}" >/dev/null
poll 15 sh -c "! kill -0 $pid_a" || fail "tab A's daemon survived its space"
poll 15 sh -c "! hc ls --json | grep -q herdr-nvim-watch" || fail "the watcher kept running with no daemons"
say "closed the space: daemon $pid_a stopped, watcher ended"

# --- 9. uninstall --------------------------------------------------------------
"$repo/herdr-app/install.sh" --uninstall --app-bin "$app_bin" 2>&1 | tee -a "$log"
[ ! -e "$HERDR_APP_HOME/layouts/main-mods.json" ] || fail "uninstall left the layout clone"
H status | grep -q '"layout": "main"' || fail "uninstall did not switch back to main"
grep -q herdr-nvim "$HERDR_APP_HOME/keys.json" && fail "uninstall left keys"
[ ! -e "$HERDR_APP_HOME/mods/herdr-nvim" ] || fail "uninstall left the mod"
H do space.new "{\"name\":\"after\",\"folder\":\"$demo\"}" >/dev/null
files_button
sleep 1.5
shot 11-uninstalled-stock-files
say "PASS: every step"
