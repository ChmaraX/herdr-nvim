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
export PI_CODING_AGENT_DIR=$root/pi CLAUDE_CONFIG_DIR=$root/claude # agents' session logs
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
mkdir -p "$demo/src/deep/er/nest" "$demo/build"
echo needle > "$demo/src/deep/er/nest/needle.txt"
echo 'build/' > "$demo/.gitignore"; echo out > "$demo/build/out.js"; echo h > "$demo/.hidden"
# A LazyVim-like explorer mapping in the nvim config: herdr-nvim takes it over.
mkdir -p "$XDG_CONFIG_HOME/nvim"
cat > "$XDG_CONFIG_HOME/nvim/init.lua" <<'LUA'
vim.keymap.set("n", "<leader>fe", function() vim.g.explorer_opened = (vim.g.explorer_opened or 0) + 1 end, { desc = "Explorer Snacks (root dir)" })
vim.keymap.set("n", "<leader>e", "<leader>fe", { desc = "Explorer Snacks (root dir)", remap = true })
LUA
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
# Agent A's pi session log, as pi writes it (after the agent started): it read
# package.json, then edited src/notes.md. The picker must list these first.
"$py" "$here/fake-pi-session.py" "$PI_CODING_AGENT_DIR" "$(cd "$demo" && pwd -P)"
say "agent A's pi session log: read package.json, edited src/notes.md"

# --- install --------------------------------------------------------------
layouts=$HERDR_APP_HOME/layouts
layouts_now() { (cd "$layouts" && ls -- *.json | sort | tr '\n' ' '); }
shown_layout() { H status | "$py" -c 'import json,sys; print(json.load(sys.stdin)["layout"])'; }
shown_is() { [ "$(shown_layout)" = "$1" ]; }
ldiff() { "$py" "$here/layout_diff.py" "$@" | tee -a "$log"; } # ldiff BEFORE AFTER --expect JSON | --same BEFORE AFTER
no_change='{"changed": {}, "added": [], "removed": []}'
in_slot='{"changed": {"files": ["content"], "panel": ["size"], "root": ["floating"]}, "added": [], "removed": []}'
installed=$("$repo/herdr-app/install.sh" --app-bin "$app_bin" 2>&1 | tee -a "$log")
echo "$installed" | grep -q '^layout: cloned main as main-copy' || fail "on shipped main, the install did not clone main-copy"
poll 10 shown_is main-copy || fail "the install did not show main-copy"
wrapper=$HERDR_APP_HOME/mods/herdr-nvim/bin/herdr-nvim
H check "$HERDR_APP_HOME/mods/herdr-nvim" | tee -a "$log"
again=$("$repo/herdr-app/install.sh" --app-bin "$app_bin" 2>&1 | grep '^layout:' || true)
echo "$again" | grep -q 'already uses herdr-nvim' || fail "a second install is not a no-op: $again"
say "second install: $again"
layouts_installed=$(layouts_now)
ldiff "$layouts/main.json" "$layouts/main-copy.json" --expect "$in_slot" ||
  fail "main-copy differs from main elsewhere than the files slot, the panel width and the controller"
"$py" -c 'import json,sys; l=json.load(open(sys.argv[1]))
def find(n,i): return n if n.get("id")==i else next((x for c in n.get("children",[]) for x in [find(c,i)] if x), None)
print("  files:", json.dumps(find(l,"files")["content"]), " panel:", find(l,"panel")["size"], " floating:", json.dumps(l["floating"]))' "$layouts/main-copy.json" | tee -a "$log"
H status | grep -q '"errors": \[\]' || fail "the app reports errors: $(H status)"

# An orphan daemon (its tab never existed) for the startup GC to reap.
mkdir -p "$root/run/app"
orphan_sock=$root/run/app/spdeadbeef000_tbdeadbeef000.sock
nvim --headless --listen "$orphan_sock" >/dev/null 2>&1 &
orphan_pid=$!
poll 10 test -S "$orphan_sock" || fail "orphan daemon did not start"
say "orphan daemon pid $orphan_pid (tab that never existed)"

# --- 1. Files button in tab A: the tree and the tab's nvim, together -------
probe() { "$py" "$here/probe.py" "$app_bin"; }
panel_is() { [ "$(probe | "$py" -c 'import json,sys; print(json.load(sys.stdin)["panel"])')" = "$1" ]; }
no_explorer() { # no file explorer window of nvim's own (netrw, snacks, nvim-tree, neo-tree, oil)
  ! nvim_eval "$1" 'join(map(nvim_list_wins(), {_,w -> getbufvar(winbufnr(w), "&filetype")}), ",")' |
    grep -Eq 'netrw|snacks_picker_list|NvimTree|neo-tree|oil|minifiles'
}
H do tab.select "{\"id\":\"$tab_a\"}" >/dev/null
H do pane.focus "{\"terminal\":\"$agent_a\"}" >/dev/null
files_button
poll 10 panel_is files || fail "Files did not show the side panel"
poll 15 sidebar_of "$tab_a" || fail "Files did not open tab A's nvim pane with the tree"
sidebar_a=$(sidebar_of "$tab_a")
poll 15 sh -c "ls $root/run/app/*${tab_a:0:12}.sock" || fail "no daemon for tab A"
sock_a=$(sock_of "$space" "$tab_a")
pid_a=$(daemon_pid "$sock_a")
poll 10 focus_is "$sidebar_a" || fail "focus is not on the new nvim pane"
width=$(probe | "$py" -c 'import json,sys; print(json.load(sys.stdin)["width"])')
[ "$width" = 300 ] || fail "the panel is $width px wide, not the default 300"
no_explorer "$sock_a" || fail "nvim opened a file explorer of its own"
H status | grep -q '"errors": \[\]' || fail "the mods report errors: $(H status)"
say "Files: panel with the tree (300 px) and tab A's nvim pane $sidebar_a (daemon $pid_a), focused, no nvim explorer"
poll 10 sh -c "! kill -0 $orphan_pid" || fail "the orphan daemon survived the watcher's start"
say "startup GC: orphan daemon $orphan_pid stopped"
sleep 1
shot 01-files-tree-and-nvim
# What a click on src/greet.js in the tree runs (tree-view.js activate -> lib.js nvim):
"$wrapper" open "$demo/src/greet.js" --ctx "{\"space\":\"$space\",\"tab\":\"$tab_a\",\"terminal\":\"$agent_a\",\"cwd\":\"$demo\"}" >/dev/null
poll 10 nvim_at "$sock_a" greet.js:1 || fail "tab A's nvim is not on greet.js"
say "tree pick: src/greet.js opens in tab A's nvim pane"
sleep 0.8
shot 02-tree-opened-greet-in-nvim
files_button_off() { H do layout.hide '{"id":"panel"}' >/dev/null; } # the button's other half
files_button_off
poll 10 panel_is hidden || fail "Files did not hide the panel"
poll 15 sh -c "! hc ls --json --label herdr-nvim.sidebar=$tab_a | grep -q '\"live\"'" || fail "Files off left tab A's nvim pane open"
alive "$pid_a" || fail "closing the pane stopped the daemon"
say "Files again: panel and nvim pane closed together; daemon $pid_a keeps running"

# --- 2. cmd-e in tab B does the same, and tab switches carry the pair --------
H do tab.select "{\"id\":\"$tab_b\"}" >/dev/null
H do pane.focus "{\"terminal\":\"$agent_b\"}" >/dev/null
keys 14:cmd # cmd-e
poll 10 panel_is files || fail "cmd-e did not show the panel"
poll 15 sidebar_of "$tab_b" || fail "cmd-e did not open tab B's nvim pane"
sidebar_b=$(sidebar_of "$tab_b")
poll 15 sh -c "ls $root/run/app/*${tab_b:0:12}.sock" || fail "no daemon for tab B"
sock_b=$(sock_of "$space" "$tab_b")
pid_b=$(daemon_pid "$sock_b")
[ "$pid_a" != "$pid_b" ] || fail "tabs A and B share a daemon"
say "cmd-e: panel and tab B's nvim pane $sidebar_b (its own daemon $pid_b)"
sleep 0.5
shot 03-cmd-e-tab-b
H do tab.select "{\"id\":\"$tab_a\"}" >/dev/null
poll 10 panel_is hidden || fail "switching to tab A (no nvim pane) left the panel open"
H do tab.select "{\"id\":\"$tab_b\"}" >/dev/null
poll 10 panel_is files || fail "switching back to tab B did not bring its panel back"
sidebar_of "$tab_a" && fail "a tab switch opened an nvim pane in tab A"
say "tab switches: the panel follows each tab's nvim pane (A: closed, B: open), no pane opened by looking"
space2=$(H do space.new "{\"name\":\"other\",\"folder\":\"$root/work\"}" | "$py" -c 'import json,sys; print(json.load(sys.stdin)["space"])')
poll 10 panel_is hidden || fail "a new space without an nvim pane kept the panel open"
H do space.select "{\"id\":\"$space\"}" >/dev/null
poll 10 panel_is files || fail "back in the first space, tab B's panel did not come back"
H do space.select "{\"id\":\"$space2\"}" >/dev/null
poll 10 panel_is hidden || fail "the other space showed the panel"
H do space.select "{\"id\":\"$space\"}" >/dev/null
poll 10 panel_is files || fail "the first space lost its panel"
sidebar_of "$tab_a" && fail "a space switch opened an nvim pane"
H do space.close "{\"id\":\"$space2\",\"confirmed\":true}" >/dev/null
say "space switches: the panel follows the shown tab of each space (other: closed, back: open)"
keys 14:cmd # cmd-e again
poll 10 panel_is hidden || fail "cmd-e again did not hide the panel"
poll 15 sh -c "! hc ls --json --label herdr-nvim.sidebar=$tab_b | grep -q '\"live\"'" || fail "cmd-e again left tab B's nvim pane"
say "cmd-e again: both closed"

# --- 3. picker in tab A ---------------------------------------------------
H do tab.select "{\"id\":\"$tab_a\"}" >/dev/null
H do pane.focus "{\"terminal\":\"$agent_a\"}" >/dev/null
keys 31:cmd # cmd-o
poll 15 sh -c "\"$app_bin\" status | tr -d ' \n' | grep -q '\"overlays\":\[\"layout\"'" || fail "the picker dialog did not open"
picker_term() {
  for t in $(hc ls --json | "$py" -c 'import json,sys; [print(t["address"]) for t in json.load(sys.stdin)["terminals"]]'); do
    hc get "$t" --json | grep -q '"picker"' && { echo "$t"; return 0; }
  done
  return 1
}
poll 10 picker_term || fail "no picker terminal"
picker=$(picker_term)
poll 10 sh -c "hc read $picker | grep -q 'notes.md'" || fail "the picker lists nothing"
rows=$(hc read "$picker" | grep -E 'notes.md|package.json|greet.js' | head -3 | sed -E 's/^[^a-z]*//; s/ +.*//' | tr '\n' ' ')
[ "$rows" = "src/notes.md package.json src/greet.js " ] || fail "the agent's session files don't come first: $rows"
say "cmd-o: the picker lists the agent's session files first: $rows"
sleep 0.6
shot 04-picker
keys text:note
sleep 0.4
shot 05-picker-filtered
keys enter
# A session row carries no line (as in the TUI: with a session log the
# screen's path:line mentions are not scraped).
poll 15 nvim_at "$sock_a" notes.md:1 ||
  fail "the pick did not open notes.md (got $(nvim_eval "$sock_a" 'expand("%:t") . ":" . line(".")'))"
say "picker: tab A's nvim shows $(nvim_eval "$sock_a" 'expand("%:p") . ":" . line(".")')"
sidebar_a=$(sidebar_of "$tab_a") || fail "the pick did not open tab A's nvim pane"
poll 10 focus_is "$sidebar_a" || fail "focus did not move to the nvim pane"
say "focus is on tab A's sidebar"
poll 10 panel_is files || fail "opening a picked file did not bring the Files panel with the nvim pane"
say "the pick opened tab A's nvim pane and the Files panel with it"
sleep 0.5
shot 06-picked-notes

# --- 4. annotations to agent A ----------------------------------------------
keys esc 20 5:shift # 3G: to line 3
poll 5 nvim_at "$sock_a" notes.md:3 || fail "3G did not move to line 3"
keys backslash 0 8 # \ac: comment the line
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
keys 14:cmd # cmd-e: close both first
poll 10 panel_is hidden || fail "cmd-e did not close the panel"
poll 15 sh -c "! hc ls --json --label herdr-nvim.sidebar=$tab_a | grep -q '\"live\"'" || fail "cmd-e left tab A's nvim pane"
"$wrapper" open --tab "$tab_a" src/notes.md:3:6 >/dev/null
poll 10 panel_is files || fail "open --tab did not bring the Files panel with the nvim pane"
sidebar_a=$(sidebar_of "$tab_a") || fail "open --tab did not open tab A's nvim pane"
poll 10 nvim_at "$sock_a" notes.md:3 || fail "open --tab did not land on notes.md:3"
[ "$(nvim_eval "$sock_a" 'col(".")')" = 6 ] || fail "open --tab ignored the column"
say "open --tab $tab_a src/notes.md:3:6 (from closed, no key or button) -> nvim pane on notes.md:3:6 and the Files panel"

# --- 6. toggle off and back: the same daemon ---------------------------------
H do mod.open '{"name":"herdr-nvim"}' >/dev/null # what cmd-e runs
poll 10 panel_is hidden || fail "the toggle did not hide the panel"
poll 15 sh -c "! hc ls --json --label herdr-nvim.sidebar=$tab_a | grep -q '\"live\"'" || fail "toggle did not close tab A's sidebar"
alive "$pid_a" || fail "closing the sidebar stopped the daemon"
say "toggled off: sidebar gone, daemon $pid_a still running"
no_overlay() { ! H status | tr -d ' \n' | grep -q '"overlays":\["'; }
poll 5 no_overlay || fail "the toggle's overlay did not close"
H do mod.open '{"name":"herdr-nvim"}' >/dev/null
poll 15 sidebar_of "$tab_a" || fail "the toggle did not reopen tab A's sidebar"
poll 10 panel_is files || fail "the toggle did not show the panel again"
[ "$(daemon_pid "$sock_a")" = "$pid_a" ] || fail "reopened sidebar has a new daemon"
[ "$(nvim_eval "$sock_a" 'expand("%:t")')" = notes.md ] || fail "the daemon lost its buffer"
say "toggle again: reattached to daemon $pid_a, still on $(nvim_eval "$sock_a" 'expand("%:t") . ":" . line(".")')"
sleep 0.8
shot 10-reattached

say "daemons: $("$wrapper" daemons | tr '\n' '|')"

# --- 6b. the native tree: keyboard, LazyVim's keys, nvim doing the work -----
sidebar_a=$(sidebar_of "$tab_a")
ctx_a="{\"space\":\"$space\",\"tab\":\"$tab_a\",\"terminal\":\"$agent_a\",\"cwd\":\"$demo\"}"
open_in_nvim() { "$wrapper" open "$1" --ctx "$ctx_a" >/dev/null; }
nvim_on() { [ "$(nvim_eval "$sock_a" 'expand("%:t")')" = "$1" ]; }
dialog_open() { H status | tr -d ' \n' | grep -q '"overlays":\["herdr-nvim"'; }
gone_buffer() { [ "$(nvim_eval "$sock_a" "bufnr('$1')")" = "-1" ]; }
real_demo=$(cd "$demo" && pwd -P)
tree_key() { keys "$@"; sleep 0.4; } # paced like a person typing
into_tree() { keys 14:cmd,shift; sleep 1; } # cmd-shift-e

open_in_nvim "$demo/src/greet.js"
poll 10 nvim_on greet.js || fail "nvim did not open greet.js"
into_tree
tree_key text:r
poll 5 dialog_open || fail "cmd-shift-e: the tree did not take the keyboard (no rename dialog)"
shot 11-tree-rename-dialog
tree_key 0:cmd text:src/hello.js enter
poll 10 test -f "$demo/src/hello.js" || fail "rename did not happen"
[ ! -e "$demo/src/greet.js" ] || fail "rename left greet.js"
poll 10 nvim_on hello.js || fail "nvim's buffer did not follow the rename"
gone_buffer "$real_demo/src/greet.js" || fail "the old buffer is still there"
say "cmd-shift-e: the tree took the keyboard on greet.js (nvim's file); r renamed it to src/hello.js, nvim's buffer followed"

tree_key text:q # back to nvim
H do pane.go '{"direction":"right"}' >/dev/null
sleep 0.8
tree_key text:a
poll 5 dialog_open || fail "pane.go right did not give the tree the keyboard"
tree_key text:fromgo.txt enter
poll 10 test -f "$demo/src/fromgo.txt" || fail "a: no new file next to hello.js"
poll 10 nvim_on fromgo.txt || fail "the new file did not open in nvim"
say "pane.go right: into the tree on hello.js; a made src/fromgo.txt (next to it) and nvim opened it"

H do pane.focus "{\"terminal\":\"$sidebar_a\"}" >/dev/null
tree_key esc backslash text:fe # \fe: LazyVim's explorer key, in nvim
tree_key text:c
poll 5 dialog_open || fail "<leader>fe did not focus the tree"
tree_key 0:cmd text:copied.txt enter
poll 10 test -f "$demo/src/copied.txt" || fail "c: no copy"
[ "$(nvim_eval "$sock_a" 'get(g:, "explorer_opened", 0)')" = 0 ] || fail "<leader>fe still opened nvim's explorer"
nvim_eval "$sock_a" 'maparg("<leader>e","n",0,1).desc' | grep -q '^herdr-nvim: Files, show/hide' || fail "the explorer's <leader>e was not taken over"
say "<leader>fe (an 'Explorer Snacks' mapping) in nvim: the tree took the keyboard on fromgo.txt, not nvim's explorer; c copied it to src/copied.txt; <leader>e became the tree key"
sleep 0.5
shot 12-leader-e-tree-copied

open_in_nvim "$demo/src/notes.md"
into_tree
tree_key tab text:g text:j text:m # mark notes.md, stand on src/deep, move
poll 5 dialog_open || fail "m: no confirm"
shot 13-move-confirm
tree_key enter
poll 10 test -f "$demo/src/deep/notes.md" || fail "m: notes.md not moved into src/deep"
[ ! -e "$demo/src/notes.md" ] || fail "m left src/notes.md"
poll 10 sh -c "nvim --headless --server $sock_a --remote-expr 'expand(\"%:p\")' | grep -q 'src/deep/notes.md'" || fail "nvim's buffer did not follow the move"
say "Tab marked notes.md, m moved it into src/deep after the confirm; nvim's buffer followed"

open_in_nvim "$demo/src/copied.txt"
into_tree
tree_key text:d
poll 5 dialog_open || fail "d: no confirm"
shot 14-delete-confirm
tree_key enter
poll 10 sh -c "! test -e $demo/src/copied.txt" || fail "d did not delete"
poll 5 gone_buffer "$real_demo/src/copied.txt" || fail "the deleted file's buffer is still open"
say "d deleted src/copied.txt after the confirm; its buffer is gone"

open_in_nvim "$demo/package.json"
poll 10 nvim_on package.json || fail "nvim did not open package.json"
into_tree
tree_key text:a
poll 5 dialog_open || fail "a: no dialog"
tree_key text:newdir/ enter
poll 10 test -d "$demo/newdir" || fail "a: no folder"
into_tree # back on package.json: a works in its folder
tree_key text:a
poll 5 dialog_open || fail "a: no dialog"
tree_key text:newdir/made.lua enter
poll 10 test -f "$demo/newdir/made.lua" || fail "a: no file in the new folder"
poll 10 nvim_on made.lua || fail "the new file did not open in nvim"
say "a made the folder newdir/ (trailing /) and newdir/made.lua, opened in nvim"

into_tree
tree_key text:/ text:needle
sleep 1.5
shot 15-filter-needle
tree_key enter
poll 10 nvim_on needle.txt || fail "/needle did not find and open src/deep/er/nest/needle.txt"
say "/ needle + ⏎: found src/deep/er/nest/needle.txt and opened it in nvim"

into_tree
tree_key text:Z text:I text:H
sleep 0.8
shot 16-hidden-ignored-shown
tree_key text:g text:r
poll 5 dialog_open || fail "r on the first row: no dialog"
tree_key 0:cmd text:build2 enter
poll 10 test -d "$demo/build2" || fail "I: the ignored folder build/ was not the first row"
tree_key text:G text:k text:r
poll 5 dialog_open || fail "r on .hidden: no dialog"
tree_key 0:cmd text:.hidden2 enter
poll 10 test -f "$demo/.hidden2" || fail "H: .hidden was not shown"
tree_key text:H text:I
say "I showed the git-ignored build/ (renamed to build2 from its row), H showed .hidden (renamed to .hidden2)"

# --- 6b'. one highlight at a time; the root and its way back; the key list ---
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
into_tree; focus_shot 18-focus-tree tree
tree_key text:q; sleep 0.6; focus_shot 18-focus-q-nvim nvim
into_tree; tree_key esc; sleep 0.6; focus_shot 18-focus-esc-nvim nvim
H do pane.go '{"direction":"right"}' >/dev/null; sleep 0.8; focus_shot 18-focus-go-right-tree tree
H do pane.go '{"direction":"left"}' >/dev/null; sleep 0.8; focus_shot 18-focus-go-left-nvim nvim
into_tree; tree_key enter; sleep 1; focus_shot 18-focus-open-nvim nvim
say "one highlight at a time: tree after cmd-shift-e and pane.go right; nvim after q, Esc, pane.go left and opening a file"
# A click into the tree while nvim has the keyboard. Posted mouse events don't
# reach the app: herdr-nvim.click runs the tree's own mouse-down handler,
# after focusing the tree as GPUI does on a press.
H do pane.focus "{\"terminal\":\"$sidebar_a\"}" >/dev/null; sleep 0.8; focus_shot 18-focus-before-click-nvim nvim
H do herdr-nvim.click >/dev/null; sleep 1; focus_shot 18-focus-click-tree tree
tree_key text:?
poll 5 dialog_open || fail "after the click the tree does not have the keyboard"
tree_key esc; poll 5 no_overlay || fail "the key list did not close"
sleep 0.5; focus_shot 18-focus-click-list-closed-tree tree
H do pane.focus "{\"terminal\":\"$sidebar_a\"}" >/dev/null; sleep 0.8; focus_shot 18-focus-click-back-nvim nvim
say "a click into the tree (its mouse-down handler): only the tree highlighted, and it has the keyboard; back to nvim: only nvim"

open_in_nvim "$demo/src/deep/er/nest/needle.txt"
into_tree
tree_key text:h text:. # stand on src/deep/er/nest's folder, make it the root
sleep 0.8
shot 19-breadcrumb-after-dot
tree_key text:g text:r
poll 5 dialog_open || fail "r: no dialog in the new root"
tree_key 0:cmd text:found.txt enter
poll 10 test -f "$demo/src/deep/er/nest/found.txt" || fail ". did not make src/deep/er/nest the root"
tree_key text:~
sleep 0.8
tree_key text:g text:r
poll 5 dialog_open || fail "r: no dialog after ~"
tree_key esc
sleep 0.5
say ". made src/deep/er/nest the root (its first row was needle.txt, renamed found.txt; breadcrumb shot 19); ~ went back"
tree_key text:?
poll 5 dialog_open || fail "? did not open the key list"
shot 19-key-list
tree_key text:?
poll 5 no_overlay || fail "? did not close the key list"
sleep 0.5
focus_shot 19-key-list-closed-tree tree
say "? opened the key list and closed it; the tree has the keyboard again"

# An nvim whose <leader>e opens no explorer keeps its own mapping.
cat > "$XDG_CONFIG_HOME/nvim/init.lua" <<'LUA'
vim.keymap.set("n", "<leader>e", function() vim.g.edit_hit = 1 end, { desc = "Edit something" })
LUA
keys 14:cmd
poll 15 sh -c "! hc ls --json --label herdr-nvim.sidebar=$tab_a | grep -q '\"live\"'" || fail "cmd-e did not close the pair"
"$wrapper" daemons stop "$space:$tab_a" --force >/dev/null
keys 14:cmd
poll 15 sidebar_of "$tab_a" || fail "cmd-e did not reopen nvim"
sidebar_a=$(sidebar_of "$tab_a"); sock_a=$(sock_of "$space" "$tab_a")
poll 10 sh -c "nvim --headless --server $sock_a --remote-expr 'maparg(\"<leader>e\",\"n\",0,1).desc' | grep -qx 'Edit something'" ||
  fail "a <leader>e that opens no explorer was taken over"
say "an nvim whose <leader>e is 'Edit something' keeps it"

# --- 6b''. the tree key: <leader>e shows and hides the tree, nvim stays -------
pair_off() { poll 15 sh -c "! hc ls --json --label herdr-nvim.sidebar=$tab_a | grep -q '\"live\"'"; }
restart_pair() { # close the pair, stop tab A's nvim, open the pair: nvim reads its config again
  keys 14:cmd; pair_off || fail "cmd-e did not close the pair"
  "$wrapper" daemons stop "$space:$tab_a" --force >/dev/null
  keys 14:cmd; poll 15 sidebar_of "$tab_a" || fail "cmd-e did not reopen nvim"
  sidebar_a=$(sidebar_of "$tab_a"); sock_a=$(sock_of "$space" "$tab_a")
  poll 10 test -S "$sock_a" || fail "no daemon socket"
}
leader_e_desc() { nvim_eval "$sock_a" 'get(maparg("<leader>e","n",0,1), "desc", "")'; }
leader_e_is() { [ "$(leader_e_desc)" = "$1" ]; }
# A plain nvim: no plugins, nothing on <leader>e. herdr-nvim maps it.
: > "$XDG_CONFIG_HOME/nvim/init.lua"
restart_pair
poll 10 leader_e_is "herdr-nvim: Files, show/hide" || fail "a free <leader>e was not mapped: $(leader_e_desc)"
say "plain nvim: <leader>e was free, herdr-nvim mapped it ($(leader_e_desc))"
echo leader > "$demo/src/leader.txt"
open_in_nvim "$demo/src/leader.txt"
poll 10 nvim_on leader.txt || fail "nvim did not open leader.txt"
H do pane.focus "{\"terminal\":\"$sidebar_a\"}" >/dev/null; sleep 0.8
tree_key esc backslash 14 # \e with the tree showing: hidden, nvim stays and keeps the keyboard
poll 10 panel_is hidden || fail "<leader>e did not hide the tree"
sleep 1.5; sidebar_of "$tab_a" >/dev/null || fail "<leader>e closed nvim with the tree"
focus_is "$sidebar_a" || fail "<leader>e: nvim does not have the keyboard"
focus_shot 21-leader-e-hid-tree wide
tree_key esc backslash 14 # \e with the tree hidden: shown, focused on nvim's file
poll 10 panel_is files || fail "<leader>e did not show the tree"
sleep 1; focus_shot 21-leader-e-shows-tree tree
tree_key text:r; poll 5 dialog_open || fail "<leader>e: the tree did not take the keyboard"
tree_key 0:cmd text:src/leader2.txt enter
poll 10 test -f "$demo/src/leader2.txt" || fail "<leader>e: the tree was not on leader.txt (nvim's file)"
sleep 0.8
tree_key backslash 14 # the same keys in the tree: hidden, nvim stays and gets the keyboard
poll 10 panel_is hidden || fail "\\ e in the tree did not hide it"
sleep 1.5; sidebar_of "$tab_a" >/dev/null || fail "\\ e in the tree closed nvim"
focus_is "$sidebar_a" || fail "\\ e in the tree: nvim does not have the keyboard"
focus_shot 21-tree-leader-e-back-to-nvim wide
say "<leader>e: hid the tree (nvim kept the keyboard), showed it on nvim's file (r renamed leader.txt), and \\ e in the tree hid it again, nvim focused"
# Files and cmd-e from "nvim open, tree hidden": show the tree; again: close both.
files_button
poll 10 panel_is files || fail "Files did not show the tree"
sleep 1.5; sidebar_of "$tab_a" >/dev/null || fail "Files closed nvim"
H do layout.hide '{"id":"panel"}' >/dev/null # the Files button again
pair_off || fail "Files again did not close nvim"
panel_is hidden || fail "Files again left the panel"
keys 14:cmd; poll 15 sidebar_of "$tab_a" || fail "cmd-e did not open the pair"
sidebar_a=$(sidebar_of "$tab_a")
H do pane.focus "{\"terminal\":\"$sidebar_a\"}" >/dev/null; sleep 0.8
tree_key esc backslash 14; poll 10 panel_is hidden || fail "<leader>e did not hide the tree"
sleep 1; keys 14:cmd
poll 10 panel_is files || fail "cmd-e did not show the tree"
sleep 1.5; sidebar_of "$tab_a" >/dev/null || fail "cmd-e closed nvim"
keys 14:cmd; pair_off || fail "cmd-e again did not close nvim"
panel_is hidden || fail "cmd-e again left the panel"
say "nvim open, tree hidden: Files and cmd-e each showed the tree (nvim kept), and pressed again closed both"
# Switching back to a tab whose tree <leader>e hid: it stays hidden.
keys 14:cmd; poll 15 sidebar_of "$tab_a" || fail "cmd-e did not open the pair"
sidebar_a=$(sidebar_of "$tab_a")
H do pane.focus "{\"terminal\":\"$sidebar_a\"}" >/dev/null; sleep 0.8
tree_key esc backslash 14; poll 10 panel_is hidden || fail "<leader>e did not hide the tree"
H do tab.select "{\"id\":\"$tab_b\"}" >/dev/null
files_button; poll 15 sidebar_of "$tab_b" || fail "Files in tab B did not open its nvim"
H do tab.select "{\"id\":\"$tab_a\"}" >/dev/null
sleep 1.5; panel_is hidden || fail "back in tab A, the tree <leader>e hid came back"
sidebar_of "$tab_a" >/dev/null || fail "tab A lost its nvim"
shot 21-switch-back-tree-stays-hidden
H do tab.select "{\"id\":\"$tab_b\"}" >/dev/null
poll 10 panel_is files || fail "tab B (tree shown) did not show the panel"
H do layout.hide '{"id":"panel"}' >/dev/null # close B's pair again
poll 15 sh -c "! hc ls --json --label herdr-nvim.sidebar=$tab_b | grep -q '\"live\"'" || fail "Files did not close tab B's nvim"
H do tab.select "{\"id\":\"$tab_a\"}" >/dev/null
sleep 1.5; panel_is hidden || fail "tab A's tree came back"
files_button; poll 10 panel_is files || fail "Files did not show tab A's tree"
say "tab A's tree, hidden by <leader>e, stayed hidden when coming back from tab B (twice); Files showed it again"
# tree_key = "": nothing mapped (the config's other settings kept).
config_toml=$XDG_CONFIG_HOME/herdr-nvim/config.toml
mkdir -p "${config_toml%/*}"; touch "$config_toml"; cp "$config_toml" "$root/config.toml.saved"
"$py" - "$config_toml" <<'EOF'
import sys
path = sys.argv[1]
lines = open(path).read().splitlines()
if "[app]" in lines: lines.insert(lines.index("[app]") + 1, 'tree_key = ""')
else: lines += ["[app]", 'tree_key = ""']
open(path, "w").write("\n".join(lines) + "\n")
EOF
"$wrapper" app-config | grep -q '"tree_key":""' || fail "tree_key = \"\" not read: $("$wrapper" app-config)"
restart_pair
sleep 2; leader_e_is "" || fail "tree_key = \"\": <leader>e is mapped ($(leader_e_desc))"
grep -qx '{"tree_key":\[\]}' "$(nvim_eval "$sock_a" '$HERDR_NVIM_CURRENT_FILE').keys" || fail "tree_key = \"\": the tree still has a key"
cp "$root/config.toml.saved" "$config_toml"
say "tree_key = \"\": <leader>e left unmapped, and the tree has no tree key"

# --- 6c. tree = "nvim": cmd-e and Files open only nvim, with its explorer ---
keys 14:cmd
poll 10 panel_is hidden || fail "cmd-e did not close the pair"
"$wrapper" app-setup tree nvim | tee -a "$log"
"$wrapper" daemons stop "$space:$tab_a" --force >/dev/null
H do pane.focus "{\"terminal\":\"$agent_a\"}" >/dev/null
keys 14:cmd
poll 15 sidebar_of "$tab_a" || fail "tree=nvim: cmd-e did not open nvim"
sock_a=$(sock_of "$space" "$tab_a")
panel_is hidden || fail "tree=nvim: the Files panel showed"
poll 10 sh -c "nvim --headless --server $sock_a --remote-expr 'join(map(nvim_list_wins(), {_,w -> getbufvar(winbufnr(w), \"&filetype\")}), \",\")' | grep -q netrw" ||
  fail "tree=nvim: nvim's own explorer did not open"
sleep 0.8
shot 17-tree-nvim-mode
say "tree=nvim: cmd-e opened only the nvim pane, with nvim's explorer (netrw here) inside"
files_button
poll 15 sh -c "! hc ls --json --label herdr-nvim.sidebar=$tab_a | grep -q '\"live\"'" || fail "tree=nvim: Files did not close nvim"
poll 10 panel_is hidden || fail "tree=nvim: Files left the panel open"
files_button
poll 15 sidebar_of "$tab_a" || fail "tree=nvim: Files did not open nvim"
poll 10 panel_is hidden || fail "tree=nvim: Files showed the panel"
keys 14:cmd
poll 15 sh -c "! hc ls --json --label herdr-nvim.sidebar=$tab_a | grep -q '\"live\"'" || fail "tree=nvim: cmd-e did not close nvim"
say "tree=nvim: Files and cmd-e toggle the nvim pane alone; the panel never stays"
"$wrapper" app-setup tree native | tee -a "$log"
pid_a=$(daemon_pid "$sock_a") # tab A's daemon now (6b/6c restarted it)
alive "$pid_a" || fail "tab A has no daemon after the tree checks"

# --- 6d. stale nvims: an install leaves them, reopening restarts the clean ones -
stamp_file=$XDG_DATA_HOME/herdr-nvim/app/runtime-stamp
real_stamp=$(cat "$stamp_file")
stamp_of() { nvim_eval "$sock_a" '$HERDR_NVIM_RUNTIME_STAMP'; }
[ "$(stamp_of)" = "$real_stamp" ] || fail "tab A's nvim did not record the runtime stamp ($(stamp_of) vs $real_stamp)"
# Tab A's nvim as one started before an install: it loaded runtime "old-e2e".
echo old-e2e > "$stamp_file"
"$wrapper" daemons stop "$space:$tab_a" --force >/dev/null
keys 14:cmd; poll 15 sidebar_of "$tab_a" || fail "cmd-e did not open the pair"
sock_a=$(sock_of "$space" "$tab_a"); poll 10 test -S "$sock_a" || fail "no daemon socket"
poll 10 sh -c "[ \"\$(nvim --headless --server $sock_a --remote-expr '\$HERDR_NVIM_RUNTIME_STAMP' 2>/dev/null)\" = old-e2e ]" ||
  fail "the daemon did not record old-e2e"
keys 14:cmd; pair_off || fail "cmd-e did not close the pair"
old_pid=$(daemon_pid "$sock_a")
installed=$("$repo/herdr-app/install.sh" --app-bin "$app_bin" 2>&1 | tee -a "$log")
[ "$(cat "$stamp_file")" = "$real_stamp" ] || fail "the install wrote another stamp for the same runtime"
echo "$installed" | grep -q '^nvim: 1 of 2 running tab nvim(s) are stale' || fail "the install did not report 1 stale nvim of 2"
alive "$old_pid" || fail "the install stopped a stale nvim"
"$wrapper" daemons | tee -a "$log" | grep -q 'alive · stale' || fail "daemons does not list tab A's nvim as stale"
say "install: reported 1 of 2 nvims stale, stopped none (tab A's $old_pid still runs)"
keys 14:cmd; poll 15 sidebar_of "$tab_a" || fail "cmd-e did not reopen the pair"
sidebar_a=$(sidebar_of "$tab_a")
poll 10 sh -c "! kill -0 $old_pid" || fail "reopening did not restart the stale nvim"
poll 10 sh -c "[ \"\$(nvim --headless --server $sock_a --remote-expr '\$HERDR_NVIM_RUNTIME_STAMP' 2>/dev/null)\" = $real_stamp ]" ||
  fail "the restarted nvim is not on the installed runtime"
pid_a=$(daemon_pid "$sock_a")
say "reopening the pair restarted the stale clean nvim ($old_pid -> $pid_a) on runtime $real_stamp"
# A stale nvim with an unsaved buffer is kept, with a toast and a message in nvim.
nvim_eval "$sock_a" 'execute("enew") . setline(1, "unsaved")' >/dev/null
[ "$(nvim_eval "$sock_a" "len(filter(getbufinfo({'bufmodified':1}),'v:val.listed'))")" = 1 ] || fail "no unsaved buffer"
keys 14:cmd; pair_off || fail "cmd-e did not close the pair"
echo old-e2e > "$stamp_file" # the next install
keys 14:cmd; poll 15 sidebar_of "$tab_a" || fail "cmd-e did not reopen the pair"
sidebar_a=$(sidebar_of "$tab_a")
sleep 1.5
alive "$pid_a" || fail "a stale nvim with an unsaved buffer was restarted"
shot 23-stale-unsaved-kept-toast
poll 10 sh -c "nvim --headless --server $sock_a --remote-expr 'execute(\"messages\")' 2>/dev/null | grep -q 'before the last install'" ||
  fail "nvim did not say why it was kept"
say "a stale nvim with an unsaved buffer was kept ($pid_a), with a toast and a message in nvim (shot 23)"
nvim_eval "$sock_a" 'execute("bwipeout!")' >/dev/null
echo "$real_stamp" > "$stamp_file"

# --- 6e. on a fresh nvim: pane.go left from the tree, and space e (Snacks-like) --
# The person's setup: space leader, LazyVim's Snacks explorer keys, and
# cmd-j h / cmd-j l for pane.go.
cat > "$XDG_CONFIG_HOME/nvim/init.lua" <<'LUA'
vim.g.mapleader = " "
vim.keymap.set("n", "<leader>fe", function() vim.g.explorer_opened = (vim.g.explorer_opened or 0) + 1 end, { desc = "Explorer Snacks (root dir)" })
vim.keymap.set("n", "<leader>e", "<leader>fe", { desc = "Explorer Snacks (root dir)", remap = true })
LUA
"$py" - "$HERDR_APP_HOME/keys.json" <<'PY'
import json, sys
k = json.load(open(sys.argv[1]))
k.setdefault("global", {}).update({"cmd-j h": "pane.go:left", "cmd-j l": "pane.go:right"})
json.dump(k, open(sys.argv[1], "w"), indent=2)
PY
restart_pair
pid_fresh=$(daemon_pid "$sock_a")
[ "$(stamp_of)" = "$real_stamp" ] || fail "the fresh nvim is not on the installed runtime"
poll 10 leader_e_is "herdr-nvim: Files, show/hide (Explorer Snacks (root dir))" ||
  fail "space e (Snacks-like) was not taken over: $(leader_e_desc)"
go_left() { keys 38:cmd text:h; sleep 0.8; } # cmd-j h
into_tree; focus_shot 24-fresh-tree-cmd-shift-e tree
go_left; focus_shot 24-fresh-cmd-j-h-from-tree nvim
H do herdr-nvim.click >/dev/null; sleep 1; focus_shot 24-fresh-click-tree tree
go_left; focus_shot 24-fresh-cmd-j-h-after-click nvim
keys 38:cmd text:l; sleep 0.8; focus_shot 24-fresh-cmd-j-l-tree tree
go_left; focus_shot 24-fresh-cmd-j-h-after-cmd-j-l nvim
say "fresh nvim ($pid_fresh): cmd-j h from the tree (after cmd-shift-e, a click, cmd-j l) left one highlight, nvim's"
H do pane.focus "{\"terminal\":\"$sidebar_a\"}" >/dev/null; sleep 0.8
tree_key "text: e" # space e in nvim, tree showing: hidden, nvim stays focused
poll 10 panel_is hidden || fail "space e did not hide the tree"
sleep 1.5; sidebar_of "$tab_a" >/dev/null || fail "space e closed nvim"
focus_is "$sidebar_a" || fail "space e: nvim does not have the keyboard"
focus_shot 25-fresh-space-e-hid-tree wide
tree_key "text: e" # tree hidden: shown, with the keyboard
poll 10 panel_is files || fail "space e did not show the tree"
sleep 1; focus_shot 25-fresh-space-e-shows-tree tree
go_left; focus_shot 25-fresh-cmd-j-h-after-space-e nvim
H do pane.go '{"direction":"right"}' >/dev/null; sleep 0.8
tree_key "text: e" # the same keys in the tree: hidden, back to nvim
poll 10 panel_is hidden || fail "space e in the tree did not hide it"
sleep 1; focus_shot 25-fresh-tree-space-e-back-to-nvim wide
[ -z "$(nvim_eval "$sock_a" 'get(g:, "explorer_opened", "")')" ] || fail "space e opened the Snacks-like explorer"
say "fresh nvim: space e (Snacks-like <leader>e) toggled the native tree from nvim and from the tree; Snacks' explorer never ran"
keys 14:cmd; poll 10 panel_is files || fail "cmd-e did not show the tree"
keys 14:cmd; pair_off || fail "cmd-e did not close the pair"
pid_a=$pid_fresh

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
[ "$(layouts_now)" = "$layouts_installed" ] || fail "uninstall deleted or added a layout: $(layouts_now)"
shown_is main-copy || fail "uninstall switched the layout away from main-copy"
ldiff "$layouts/main.json" "$layouts/main-copy.json" --expect "$no_change" || fail "main-copy is not back to main"
grep -q herdr-nvim "$HERDR_APP_HOME/keys.json" && fail "uninstall left keys"
[ ! -e "$HERDR_APP_HOME/mods/herdr-nvim" ] || fail "uninstall left the mod"
say "uninstall: main-copy kept and shown, back to main's content; keys and mods gone"
made=$(H do space.new "{\"name\":\"after\",\"folder\":\"$demo\"}")
space_c=$(echo "$made" | "$py" -c 'import json,sys; print(json.load(sys.stdin)["space"])')
tab_c=$(echo "$made" | "$py" -c 'import json,sys; print(json.load(sys.stdin)["tab"])')
files_button
sleep 1.5
shot 20-uninstalled-stock-files
H do layout.hide '{"id":"panel"}' >/dev/null

# --- 10. which layout, and back to exactly what it was ---------------------------
# Each case: install, check where it went and what changed, uninstall, check
# the layout file holds what it held before (content and key order), no layout
# was deleted or made, and the layout on screen stayed.
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

# 10a. On shipped main with a personal copy already there: that copy is used.
H do layout.switch '{"name":"main"}' >/dev/null; poll 10 shown_is main || fail "no switch to main"
cp "$layouts/main-copy.json" "$root/before-main-copy.json"; before=$(layouts_now)
install_into main-copy 'switched to main-copy, your personal copy of main'
[ "$(layouts_now)" = "$before" ] || fail "the install made another copy: $(layouts_now)"
ldiff "$root/before-main-copy.json" "$layouts/main-copy.json" --expect "$in_slot" || fail "main-copy: wrong changes"
uninstall_back main-copy "$root/before-main-copy.json" "$before"
say "10a: on main, the install used the existing main-copy; uninstall left it as it was"

# 10b. An older install's main-mods (cloned from main) is the person's copy.
H do layout.switch '{"name":"main"}' >/dev/null; poll 10 shown_is main || fail "no switch to main"
rm -f "$layouts/main-copy.json" "$layouts/.state/main-copy.json"
H do layout.clone '{"name":"main","as":"main-mods"}' >/dev/null
H do layout.switch '{"name":"main"}' >/dev/null; poll 10 shown_is main || fail "no switch to main"
cp "$layouts/main-mods.json" "$root/before-main-mods.json"; before=$(layouts_now)
install_into main-mods 'switched to main-mods, your personal copy of main'
[ "$(layouts_now)" = "$before" ] || fail "the install cloned main again: $(layouts_now)"
ldiff "$root/before-main-mods.json" "$layouts/main-mods.json" --expect "$in_slot" || fail "main-mods: wrong changes"
again=$("$repo/herdr-app/install.sh" --app-bin "$app_bin" 2>&1 | grep '^layout:' || true)
[ "$again" = "layout: main-mods: already uses herdr-nvim for Files" ] || fail "reinstall into main-mods: $again"
uninstall_back main-mods "$root/before-main-mods.json" "$before"
say "10b: an older install's main-mods is used as it is (no rename, no new clone); uninstall kept it"

# 10c. A personal layout of the person's own, shown: edited in place. Their
# panel width (420) and another mod's Changes slot stay; a key they use is kept.
"$py" - "$layouts/main-mods.json" "$layouts/mine.json" <<'PY'
import json, sys
l = json.load(open(sys.argv[1]))
l.pop("cloned_from", None)
def find(n, i): return n if n.get("id") == i else next((x for c in n.get("children", []) for x in [find(c, i)] if x), None)
find(l, "panel")["size"] = {"px": 420}
find(l, "changes")["content"] = {"mod": "changes", "props": {"by": "another mod"}}
json.dump(l, open(sys.argv[2], "w"), indent=2)
PY
"$py" - "$HERDR_APP_HOME/keys.json" <<'PY'
import json, sys
k = json.load(open(sys.argv[1])) if open(sys.argv[1]).read().strip() else {}
k.setdefault("global", {})["cmd-o"] = "app.palette"
json.dump(k, open(sys.argv[1], "w"), indent=2)
PY
H do layout.switch '{"name":"mine"}' >/dev/null; poll 10 shown_is mine || fail "no switch to mine"
cp "$layouts/mine.json" "$root/before-mine.json"; cp "$HERDR_APP_HOME/keys.json" "$root/before-keys.json"; before=$(layouts_now)
install_into mine '^layout: mine: Files now opens'
grep -q '^keys: kept yours: global cmd-o is "app.palette"' "$log" || fail "the install did not report the key it kept"
grep -q '"cmd-o": "app.palette"' "$HERDR_APP_HOME/keys.json" || fail "the install replaced the person's cmd-o"
[ "$(layouts_now)" = "$before" ] || fail "the install made a layout: $(layouts_now)"
ldiff "$root/before-mine.json" "$layouts/mine.json" --expect '{"changed": {"files": ["content"], "root": ["floating"]}, "added": [], "removed": []}' ||
  fail "mine: wrong changes (its width and Changes are the person's)"
uninstall_back mine "$root/before-mine.json" "$before"
ldiff --same "$root/before-keys.json" "$HERDR_APP_HOME/keys.json" || fail "uninstall did not put keys.json back"
say "10c: a personal layout edited in place, its width and Changes kept, cmd-o kept and reported; uninstall: layout and keys as before"

# 10d. A personal layout without a Files slot (a copy of Terminals): the
# standard side panel is added beside the tab, and works.
H do layout.clone '{"name":"terminals"}' >/dev/null; poll 10 shown_is terminals-copy || fail "no terminals-copy"
cp "$layouts/terminals-copy.json" "$root/before-terminals-copy.json"; before=$(layouts_now)
install_into terminals-copy '^layout: terminals-copy: Files now opens'
ldiff "$root/before-terminals-copy.json" "$layouts/terminals-copy.json" \
  --expect '{"changed": {"root": ["floating"]}, "added": ["files", "herdr-nvim-row", "panel", "panel-body"], "removed": []}' ||
  fail "terminals-copy: wrong changes"
H do space.select "{\"id\":\"$space_c\"}" >/dev/null
keys 14:cmd
poll 10 panel_is files || fail "terminals-copy: cmd-e did not show the added panel"
poll 15 sidebar_of "$tab_c" || fail "terminals-copy: cmd-e did not open the tab's nvim"
sleep 1.5
shot 22-terminals-copy-added-panel
keys 14:cmd
poll 10 panel_is hidden || fail "terminals-copy: cmd-e did not hide the panel"
no_sidebar() { ! sidebar_of "$1"; }
poll 15 no_sidebar "$tab_c" || fail "terminals-copy: cmd-e did not close the tab's nvim"
uninstall_back terminals-copy "$root/before-terminals-copy.json" "$before"
say "10d: no Files slot: the side panel was added beside the tab, cmd-e opened tree and nvim; uninstall took it out"
say "PASS: every step"
