#!/usr/bin/env bash
# End-to-end run of herdr-nvim in a private Herdr app (macOS): its own app home,
# herdr-core session, XDG dirs and daemon folders, so the person's own Herdr,
# spaces and terminals are never touched. It installs the mod with
# herdr-app/install.sh, drives it the way a person does (the Files button's
# commands, keys sent to the private window only, the picker, annotations),
# checks the real effects (daemons, buffers, what the agent received), takes
# screenshots, uninstalls, and removes everything it started.
#
#   herdr-app/e2e/run.sh [--proof DIR] [--keep] [--app-bin PATH] [--only GROUPS] [--bundle FILE]
#
# A smoke test of the flows a person uses: Files and cmd-e (core), cmd-o
# (picker), the tree's file operations (tree), Preview / Source (preview),
# daemons ending and uninstall (teardown); about 2 minutes. The logic behind
# them is checked by the fast suites (cargo test, tests/run.lua,
# js-checks.mjs). --only picker,preview runs the setup, the install and core,
# then only those flows. --bundle installs and uninstalls the way a person
# does, through the root install.sh with a release's app bundle
# (herdr-app/bundle.sh makes one), instead of this checkout's build.
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
bundle=
only=
groups=" picker tree preview teardown "
while [ $# -gt 0 ]; do
  case $1 in
    --proof) proof=$2; shift ;;
    --keep) keep=1 ;;
    --app-bin) app_bin=$2; shift ;;
    --bundle) bundle=$(cd "$(dirname "$2")" && pwd)/$(basename "$2"); shift ;;
    --only)
      only=$2; shift
      for g in ${only//,/ }; do
        [[ $groups == *" $g "* ]] || { echo "no group $g (groups:$groups)" >&2; exit 2; }
      done ;;
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
# The app as it was at the failure goes with the proof (once the app runs).
fail() { say "FAIL: $*"; type H >/dev/null 2>&1 && H shot "$proof/FAIL.png" >/dev/null 2>&1; exit 1; }

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
# herdr-nvim.debug: the controller's decisions (sync.js, lib.js `trace`) go
# with the proof.
printf '{"frame.welcomed": true, "layout": "main", "herdr-nvim.debug": "%s"}\n' "$root/trace.jsonl" > "$HERDR_APP_HOME/shared.json"
version=$(plutil -extract CFBundleShortVersionString raw "${app_bin%/MacOS/*}/Info.plist" 2>/dev/null || true)
echo "{\"last_run\": \"$version\", \"presented\": []}" > "$HERDR_APP_HOME/update-notes.json" # no "Updated" toast
H() { "$app_bin" "$@"; }
app_pid=

cleanup() {
  set +e
  say "cleanup (root $root)"
  # The app's own log goes with the proof, failed run or not.
  [ -f "$root/app.log" ] && cp "$root/app.log" "$proof/app.log"
  [ -f "$root/trace.jsonl" ] && cp "$root/trace.jsonl" "$proof/trace.jsonl"
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
    [ -S "$s" ] && perl -e 'alarm 5; exec @ARGV' nvim --headless --server "$s" --remote-send '<C-\><C-n><cmd>qa!<cr>' >/dev/null 2>&1
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
# cmd_e: what cmd-e runs (keys.json), after a moment for the last action to
# settle. Not a key press: those are lost now and then when the window isn't
# frontmost, and a person never presses within milliseconds of the last change.
cmd_e() { sleep 0.5; H do mod.open:herdr-nvim >/dev/null; }
files_button() { # exactly what the tab bar's Files button runs (mods/tabs/main.js)
  H do layout.select '{"stack":"panel-body","child":"files"}' >/dev/null
  H do layout.show '{"id":"panel"}' >/dev/null
}
# jget KEY: the KEY field of the JSON object on stdin.
jget() { "$py" -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]])' "$1"; }
focused_pane() { H status | jget focused_pane; }
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
# sock_of <space> <tab>: the tab daemon's socket, waited for (up to 15 s:
# a daemon makes it a moment after the pane opens); fails without one.
sock_of() {
  local s i
  for i in $(seq 1 60); do
    s=$(ls "$root"/run/app/*"${2:0:12}".sock 2>/dev/null | head -1 || true)
    [ -S "$s" ] && { echo "$s"; return 0; }
    sleep 0.25
  done
  return 1
}
bounded() { perl -e 'alarm shift; exec @ARGV' "$@"; } # bounded <seconds> <command...>
nvim_eval() { bounded 5 nvim --headless --server "$1" --remote-expr "$2" 2>/dev/null; }
daemon_pid() { nvim_eval "$1" 'getpid()'; }
eval_has() { nvim_eval "$1" "$2" | grep -q -- "$3"; } # eval_has <socket> <expr> <pattern>
eval_is() { [ "$(nvim_eval "$1" "$2")" = "$3" ]; }    # eval_is <socket> <expr> <value>
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
# The space is made outside the repo and its agents work in it, tab B's in
# src/: a space follows its terminals, so each tab's nvim starts in the repo
# they work in, never in the folder the space was made in.
made=$(H do space.new "{\"name\":\"e2e\",\"folder\":\"$root/work\",\"tab\":\"made\"}")
space=$(echo "$made" | jget space)
tab_made=$(echo "$made" | jget tab)
made=$(H do tab.new "{\"name\":\"tab A\",\"space\":\"$space\",\"folder\":\"$demo\",\"argv\":$argv}")
tab_a=$(echo "$made" | jget tab)
agent_a=$(echo "$made" | jget terminal)
made=$(H do tab.new "{\"name\":\"tab B\",\"space\":\"$space\",\"folder\":\"$demo/src\",\"argv\":$argv}")
tab_b=$(echo "$made" | jget tab)
agent_b=$(echo "$made" | jget terminal)
H do tab.close "{\"id\":\"$tab_made\",\"confirmed\":true}" >/dev/null
H do tab.select "{\"id\":\"$tab_a\"}" >/dev/null
poll 20 sh -c "hc read $agent_a | grep -q 'notes.md:3'" || fail "agent A did not start"
say "space $space: tab A $tab_a (agent $agent_a), tab B $tab_b (agent $agent_b)"
# Agent A's pi session log, as pi writes it (after the agent started): it read
# package.json, then edited src/notes.md. The picker must list these first.
"$py" "$here/fake-pi-session.py" "$PI_CODING_AGENT_DIR" "$(cd "$demo" && pwd -P)"
say "agent A's pi session log: read package.json, edited src/notes.md"

# --- install --------------------------------------------------------------
layouts=$HERDR_APP_HOME/layouts
layouts_now() { (cd "$layouts" && ls -- *.json | sort | tr '\n' ' '); }
shown_layout() { H status | jget layout; }
shown_is() { [ "$(shown_layout)" = "$1" ]; }
ldiff() { "$py" "$here/layout_diff.py" "$@" | tee -a "$log"; } # ldiff BEFORE AFTER --expect JSON | --same BEFORE AFTER
no_change='{"changed": {}, "added": [], "removed": []}'
# The tab block's (`main`) content: its pane header, herdr-nvim's own one.
in_slot='{"changed": {"files": ["content"], "main": ["content"], "panel": ["size"], "root": ["floating"]}, "added": [], "removed": []}'
# Install (or uninstall, with --uninstall): this checkout's build, or the
# bundle through the root installer.
# `runner=detached app_install`: with no terminal to ask on.
app_install() {
  if [ -n "$bundle" ]; then ${runner:-command} sh "$repo/install.sh" --app --bundle "$bundle" ${1:+"$1"} -- --app-bin "$app_bin"
  else ${runner:-command} "$repo/herdr-app/install.sh" ${1:+"$1"} --app-bin "$app_bin"; fi
}
detached() { perl -MPOSIX -e 'POSIX::setsid(); exec @ARGV' "$@"; }
# Without Neovim 0.10+ (a config naming none) and no one to ask (no
# terminal: setsid), the install stops before changing anything.
nvim_conf=$XDG_CONFIG_HOME/herdr-nvim/config.toml
no_nvim() { mkdir -p "${nvim_conf%/*}" && printf '[sidebar]\nnvim_bin = "%s"\n' "$root/no-nvim" > "$nvim_conf"; }
no_nvim
refused=$(runner=detached app_install 2>&1 </dev/null || true)
echo "$refused" | grep -q -- '--install-nvim' || fail "without Neovim the install did not stop and say how: $refused"
[ ! -e "$HERDR_APP_HOME/mods/herdr-nvim" ] && [ ! -e "$XDG_DATA_HOME/herdr-nvim/app" ] ||
  fail "without Neovim the install changed something"
rm "$nvim_conf"
say "without Neovim: the install stops, says how (--install-nvim), changes nothing"
installed=$(app_install 2>&1 | tee -a "$log")
echo "$installed" | grep -q '^herdr-nvim: installed in' || fail "the install did not finish: $installed"
say "installed from ${bundle:-the checkout build}"
echo "$installed" | grep -q '^layout: cloned main as main-copy' || fail "on shipped main, the install did not clone main-copy"
poll 10 shown_is main-copy || fail "the install did not show main-copy"
wrapper=$HERDR_APP_HOME/mods/herdr-nvim/bin/herdr-nvim
H check "$HERDR_APP_HOME/mods/herdr-nvim" | tee -a "$log"
node "$here/js-checks.mjs" | tee -a "$log" || fail "the mod's pure checks failed (node herdr-app/e2e/js-checks.mjs)"
# RECIPE.md's key table is the tree's key table (tree-keys.js).
node "$here/../mods/herdr-nvim/tree-keys.js" > "$root/keys-table.md"
"$py" -c 'import sys; sys.exit(open(sys.argv[2]).read().find(open(sys.argv[1]).read().strip()) < 0)' "$root/keys-table.md" "$here/../RECIPE.md" ||
  fail "RECIPE.md's key table differs from mods/herdr-nvim/tree-keys.js (node mods/herdr-nvim/tree-keys.js prints it)"
again=$(app_install 2>&1 | grep '^layout:' || true)
echo "$again" | grep -q 'already uses herdr-nvim' || fail "a second install is not a no-op: $again"
say "second install: $again"
layouts_installed=$(layouts_now)
ldiff "$layouts/main.json" "$layouts/main-copy.json" --expect "$in_slot" ||
  fail "main-copy differs from main elsewhere than the files slot, the pane header, the panel width and the controller"
"$py" -c 'import json,sys; l=json.load(open(sys.argv[1]))
def find(n,i): return n if n.get("id")==i else next((x for c in n.get("children",[]) for x in [find(c,i)] if x), None)
print("  files:", json.dumps(find(l,"files")["content"]), " panel:", find(l,"panel")["size"], " floating:", json.dumps(l["floating"]))' "$layouts/main-copy.json" | tee -a "$log"
H status | grep -q '"errors": \[\]' || fail "the app reports errors: $(H status)"

# --- the flows ----------------------------------------------------------------
. "$here/lib.sh"
ctx="{\"space\":\"$space\",\"tab\":\"$tab_a\",\"terminal\":\"$agent_a\",\"cwd\":\"$demo\"}"
real_demo=$(cd "$demo" && pwd -P)
# Each flow after `core` starts from the same place (`baseline`), so `--only`
# can run any of them after `core`.
begin() { # begin <flow>: whether it runs; if it does, from the baseline
  [ "$1" = core ] && return 0
  [ -z "$only" ] || [[ ",$only," == *",$1,"* ]] || { say "skipped: $1"; return 1; }
  baseline
}
# Tab A shown, agent A focused, no nvim pane or panel in either tab, both
# tabs' daemons running (sock_a/pid_a, sock_b/pid_b say which).
baseline() {
  H do tab.select "{\"id\":\"$tab_a\"}" >/dev/null
  local t
  for t in "$tab_a" "$tab_b"; do
    if sidebar_of "$t" >/dev/null; then
      "$wrapper" toggle --state close --ctx "{\"space\":\"$space\",\"tab\":\"$t\"}" >/dev/null
    fi
    poll 15 no_sidebar_of "$t" || fail "baseline: tab $t's nvim pane stayed"
  done
  H do layout.hide '{"id":"panel"}' >/dev/null
  poll 10 panel_is hidden || fail "baseline: the panel stayed"
  sock_a=$(sock_of "$space" "$tab_a") || fail "baseline: no daemon for tab A"; pid_a=$(daemon_pid "$sock_a")
  sock_b=$(sock_of "$space" "$tab_b") || fail "baseline: no daemon for tab B"; pid_b=$(daemon_pid "$sock_b")
  H do pane.focus "{\"terminal\":\"$agent_a\"}" >/dev/null
  sleep 0.5
}

# --- 1. core: Files and cmd-e open the tree with the tab's nvim ---------------
H do tab.select "{\"id\":\"$tab_a\"}" >/dev/null
H do pane.focus "{\"terminal\":\"$agent_a\"}" >/dev/null
files_button
poll 10 panel_is files || fail "Files did not show the side panel"
poll 15 sidebar_of "$tab_a" || fail "Files did not open tab A's nvim pane with the tree"
sidebar_a=$(sidebar_of "$tab_a")
sock_a=$(sock_of "$space" "$tab_a") || fail "no daemon for tab A"
pid_a=$(daemon_pid "$sock_a")
poll 10 focus_is "$sidebar_a" || fail "focus is not on the new nvim pane"
# The first nvim pane ever: the welcome, once; Esc gives nvim the keyboard.
poll 5 dialog_open || fail "the first nvim pane brought no welcome card"
sleep 0.5
shot 01-welcome
keys esc
poll 5 no_dialog || fail "Esc left the welcome card open"
poll 5 focus_is "$sidebar_a" || fail "after the welcome, the keyboard is not back in nvim"
say "the first nvim pane: the welcome card, once; Esc hands nvim the keyboard"
[ "$(probe | jget width)" = 300 ] || fail "the panel is not the default 300 px wide"
eval_is "$sock_a" 'getcwd()' "$real_demo" || fail "tab A's nvim is not in the repo its agent works in (the space was made in $root/work)"
no_explorer "$sock_a" || fail "nvim opened a file explorer of its own"
H status | grep -q '"errors": \[\]' || fail "the mods report errors: $(H status)"
say "Files: the tree (300 px) and tab A's nvim pane $sidebar_a (daemon $pid_a) in the tab's folder, focused"
sleep 1
shot 01-files-tree-and-nvim
# What a click on a file in the tree runs (tree-view.js).
"$wrapper" open --no-focus "$demo/src/greet.js" --ctx "$ctx" >/dev/null
poll 10 nvim_at "$sock_a" greet.js:1 || fail "tab A's nvim is not on greet.js"
say "a click in the tree: src/greet.js opens in tab A's nvim"
files_button_off
poll 10 panel_is hidden || fail "Files did not hide the panel"
pair_off || fail "Files off left tab A's nvim pane open"
alive "$pid_a" || fail "closing the pane stopped the daemon"
say "Files again: the tree and nvim's pane closed together; daemon $pid_a keeps running"
# No Neovim when tab B's nvim has to start: the card offering to install
# it, and no nvim pane.
no_nvim
H do tab.select "{\"id\":\"$tab_b\"}" >/dev/null
H do pane.focus "{\"terminal\":\"$agent_b\"}" >/dev/null
cmd_e
poll 10 dialog_open || fail "with no Neovim, cmd-e showed no card"
sleep 0.5
shot 02-no-neovim-card
no_sidebar_of "$tab_b" || fail "with no Neovim, an nvim pane opened in tab B"
keys esc
poll 5 no_dialog || fail "Esc left the Neovim card open"
rm "$nvim_conf"
files_button_off
poll 10 panel_is hidden || fail "the panel stayed after the Neovim card"
say "no Neovim: cmd-e shows the card offering to install it, opens nothing"
# cmd-e in tab B, the real key once (the binding); `cmd_e` runs its command after this.
H do tab.select "{\"id\":\"$tab_b\"}" >/dev/null
H do pane.focus "{\"terminal\":\"$agent_b\"}" >/dev/null
keys 14:cmd
poll 10 panel_is files || fail "cmd-e did not show the panel"
poll 15 sidebar_of "$tab_b" || fail "cmd-e did not open tab B's nvim pane"
sock_b=$(sock_of "$space" "$tab_b") || fail "no daemon for tab B"
pid_b=$(daemon_pid "$sock_b")
[ "$pid_a" != "$pid_b" ] || fail "tabs A and B share a daemon"
eval_is "$sock_b" 'getcwd()' "$real_demo" || fail "tab B's nvim is not in the repo (its agent works in src/)"
say "cmd-e: the tree and tab B's own nvim (daemon $pid_b)"
H do tab.select "{\"id\":\"$tab_a\"}" >/dev/null
poll 10 panel_is hidden || fail "switching to tab A (no nvim pane) left the panel open"
H do tab.select "{\"id\":\"$tab_b\"}" >/dev/null
poll 10 panel_is files || fail "switching back to tab B did not bring its tree back"
no_sidebar_of "$tab_a" || fail "a tab switch opened an nvim pane in tab A"
cmd_e
poll 10 panel_is hidden || fail "cmd-e again did not hide the panel"
poll 15 no_sidebar_of "$tab_b" || fail "cmd-e again left tab B's nvim pane"
say "tab switches carry the pair (A: closed, B: open); cmd-e again closed both"

if begin picker; then
# --- 2. cmd-o: a file from the agent's work, opened in the tab's nvim ---------
sleep 0.5; H do herdr-nvim.pick >/dev/null # what cmd-o runs (keys.json)
poll 10 picker_term || fail "the picker did not open"
picker=$(picker_term)
poll 10 sh -c "hc read $picker | grep -q 'notes.md'" || fail "the picker lists nothing"
rows=$(hc read "$picker" | grep -E 'notes.md|package.json|greet.js' | head -3 | sed -E 's/^[^a-z]*//; s/ +.*//' | tr '\n' ' ')
[ "$rows" = "src/notes.md package.json src/greet.js " ] || fail "the agent's session files don't come first: $rows"
shot 02-picker
keys text:note
sleep 0.4
keys enter
poll 15 nvim_at "$sock_a" notes.md:1 ||
  fail "the pick did not open notes.md (got $(nvim_eval "$sock_a" 'expand("%:t") . ":" . line(".")'))"
sidebar_a=$(sidebar_of "$tab_a") || fail "the pick did not open tab A's nvim pane"
poll 10 focus_is "$sidebar_a" || fail "focus did not move to the nvim pane"
poll 10 panel_is files || fail "the pick did not bring the tree with nvim"
say "cmd-o: the agent's session files first ($rows); the pick opened notes.md in tab A's nvim, focused, with the tree"
fi

if begin tree; then
# --- 3. the tree's file operations, done through nvim -------------------------
files_button
poll 15 sidebar_of "$tab_a" || fail "Files did not open tab A's pair"
sidebar_a=$(sidebar_of "$tab_a")
open_in_nvim "$demo/src/greet.js"
poll 10 nvim_on greet.js || fail "nvim did not open greet.js"
nvim_eval "$sock_a" 'setline(1, "unsaved edit")' >/dev/null
into_tree
tree_key text:r
poll 5 dialog_open || fail "cmd-shift-e: the tree did not take the keyboard (no rename dialog)"
shot 03-tree-rename-dialog
tree_key 0:cmd text:src/hello.js enter
poll 10 test -f "$demo/src/hello.js" || fail "rename did not happen"
[ ! -e "$demo/src/greet.js" ] || fail "rename left greet.js"
poll 10 nvim_on hello.js || fail "nvim's buffer did not follow the rename"
gone_buffer "$real_demo/src/greet.js" || fail "the old buffer is still there"
[ "$(nvim_eval "$sock_a" '&modified . ":" . getline(1)')" = "1:unsaved edit" ] || fail "the rename lost the unsaved edit"
poll 5 state_names hello.js || fail "nvim's state (the tree's and the header's) still names the old file"
say "r on nvim's modified file: renamed to src/hello.js, the buffer followed with its edit, nvim's state too"
into_tree
tree_key text:a
poll 5 dialog_open || fail "a: no dialog"
tree_key text:newdir/made.lua enter
poll 10 test -f "$demo/src/newdir/made.lua" || fail "a: no file in a new folder"
poll 10 nvim_on made.lua || fail "the new file did not open in nvim"
say "a made src/newdir/made.lua (the folder with it) and nvim opened it"
into_tree
tree_key text:d
poll 5 dialog_open || fail "d: no confirm"
tree_key enter
poll 10 sh -c "! test -e $demo/src/newdir/made.lua" || fail "d did not delete"
poll 5 gone_buffer "$real_demo/src/newdir/made.lua" || fail "the deleted file's buffer is still open"
say "d deleted it after the confirm; its buffer is gone"
fi

if begin preview; then
# --- 4. Preview / Source: the rendered file in nvim's exact place -------------
# What the pane header's buttons run (mods/herdr-nvim-header): herdr-nvim.swap.
printf '# Preview\n\nRendered in place of nvim.\n' > "$demo/preview.md"
open_in_nvim "$demo/preview.md"
poll 15 sidebar_of "$tab_a" || fail "open did not bring tab A's nvim"
poll 10 panel_is files || fail "the tree did not come with nvim"
sleep 1
shape=$(tab_shape)
H do "herdr-nvim.swap:$tab_a:$(enc "$demo/preview.md")" >/dev/null
poll 10 has_preview || fail "Preview put no preview in nvim's place"
poll 5 no_sidebar_of "$tab_a" || fail "Preview left nvim's pane beside the preview"
sleep 0.8
[ "$(tab_shape)" = "$shape" ] || fail "Preview changed the layout: $(tab_shape) vs $shape"
panel_is files || fail "Preview closed the tree"
shot 04-preview-in-nvims-place
H do "herdr-nvim.swap:$tab_a:" >/dev/null
poll 15 sidebar_of "$tab_a" || fail "Source did not bring nvim back"
poll 5 no_preview || fail "Source left the preview"
sleep 0.8
[ "$(tab_shape)" = "$shape" ] || fail "Source changed the layout: $(tab_shape) vs $shape"
[ "$(daemon_pid "$sock_a")" = "$pid_a" ] || fail "Source gave nvim a new daemon"
say "Preview / Source: the preview takes nvim's exact place and back, same layout and daemon, the tree shown"
# Files with a preview: closes it with the tree, and opens it again.
H do "herdr-nvim.swap:$tab_a:$(enc "$demo/preview.md")" >/dev/null
poll 10 has_preview || fail "Preview (for Files) did not show"
files_button_off
poll 10 panel_is hidden || fail "Files did not hide the panel"
poll 10 no_preview || fail "Files off left the preview"
files_button
poll 10 panel_is files || fail "Files did not show the panel again"
poll 10 has_preview || fail "Files on did not bring the preview back"
# A code file clicked in the tree with the preview shown: nvim takes its place.
"$wrapper" open --no-focus "$demo/package.json" --ctx "$ctx" >/dev/null
poll 10 sidebar_of "$tab_a" || fail "the code file did not bring nvim back in the preview's place"
poll 5 no_preview || fail "the preview stayed beside nvim"
poll 10 nvim_on package.json || fail "nvim is not on package.json"
panel_is files || fail "the tree went away"
say "Files closes the preview and brings it back; a code file from the tree puts nvim back in its place"
fi

if begin teardown; then
# --- 5. daemons end with their tabs; uninstall puts everything back -----------
H do tab.close "{\"id\":\"$tab_b\",\"confirmed\":true}" >/dev/null
poll 15 sh -c "! kill -0 $pid_b" || fail "tab B's daemon $pid_b survived its tab"
alive "$pid_a" || fail "closing tab B stopped tab A's daemon"
H do space.close "{\"id\":\"$space\",\"confirmed\":true}" >/dev/null
poll 15 sh -c "! kill -0 $pid_a" || fail "tab A's daemon survived its space"
poll 15 sh -c "! hc ls --json | grep -q herdr-nvim-watch" || fail "the watcher kept running with no daemons"
say "closing tab B stopped its daemon, closing the space the rest and the watcher"
app_install --uninstall 2>&1 | tee -a "$log" >/dev/null
[ "$(layouts_now)" = "$layouts_installed" ] || fail "uninstall deleted or added a layout: $(layouts_now)"
shown_is main-copy || fail "uninstall switched the layout away from main-copy"
ldiff "$layouts/main.json" "$layouts/main-copy.json" --expect "$no_change" || fail "main-copy is not back to main"
grep -q herdr-nvim "$HERDR_APP_HOME/keys.json" && fail "uninstall left keys"
[ ! -e "$HERDR_APP_HOME/mods/herdr-nvim" ] || fail "uninstall left the mod"
[ ! -e "$HERDR_APP_HOME/mods/herdr-nvim-header" ] || fail "uninstall left the header mod"
[ ! -e "$XDG_DATA_HOME/herdr-nvim/app" ] || fail "uninstall left a runtime: $(ls "$XDG_DATA_HOME/herdr-nvim/app")"
H status | grep -q '"errors": \[\]' || fail "the app reports errors after the uninstall: $(H status)"
say "uninstall: the layout back to main's content, keys and mods gone, no errors"
fi

say "PASS: ${only:-every flow}"
