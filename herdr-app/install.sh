#!/usr/bin/env bash
# A convenience helper: does what RECIPE.md describes, the way AGENTS.md says
# to add a mod, so an agent can do the same by hand.
#
#   herdr-app/install.sh [--home DIR] [--app-bin PATH] [--panel-width PX] [--tree native|nvim]
#   herdr-app/install.sh --uninstall [--home DIR] [--app-bin PATH]
#
# The Herdr app must be open on that home (default: $HERDR_APP_HOME, else
# ~/.config/herdr-app). Install builds herdr-nvim, copies its runtime (binary +
# lua) to ${XDG_DATA_HOME:-~/.local/share}/herdr-nvim/app, copies the two mods
# into <home>/mods with a wrapper at mods/herdr-nvim/bin/herdr-nvim, runs
# `herdr-app check` on them, merges keys.json into the person's keys (only
# free keys: one they already use is kept and reported), and makes RECIPE.md's
# layout changes in the person's personal layout, the one on screen. On one
# of Herdr's own layouts it uses the person's copy of it, or clones it once
# (`layout.clone`, `<name>-copy`). The side panel is --panel-width wide
# (default 300, unless the layout file sets a width of its own). --tree writes
# [app] tree into herdr-nvim's config (native: the native tree beside nvim, the
# default; nvim: nvim's own explorer inside the nvim pane). Run it again to
# upgrade in place: tab nvims already running keep running and are reported
# as stale; each restarts on the new runtime the next time its pair opens
# (unless it has unsaved work). Uninstall takes out exactly those edits (keys
# still bound to herdr-nvim, its layout changes in every personal layout; no
# layout is deleted or switched), the mods and the runtime, and stops the
# app's nvims.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(dirname "$here")
mode=install
home=${HERDR_APP_HOME:-$HOME/.config/herdr-app}
app_bin=${HERDR_APP_BIN:-}
panel_width=
tree=
mods=(herdr-nvim herdr-nvim-pick)

die() { echo "herdr-nvim: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case $1 in
    --uninstall) mode=uninstall ;;
    --home) home=$2; shift ;;
    --app-bin) app_bin=$2; shift ;;
    --panel-width) panel_width=$2; shift ;;
    --tree) tree=$2; shift ;;
    -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
    *) die "unknown argument $1" ;;
  esac
  shift
done

# The herdr-app binary: given, on PATH, or a Herdr bundle (found by its bundle
# id, never by running an app that merely has the name).
find_app_bin() {
  if [ -n "$app_bin" ]; then echo "$app_bin"; return; fi
  if command -v herdr-app >/dev/null; then command -v herdr-app; return; fi
  for bundle in /Applications/*.app "$HOME"/Applications/*.app; do
    plist=$bundle/Contents/Info.plist
    [ -f "$plist" ] || continue
    [ "$(plutil -extract CFBundleIdentifier raw "$plist" 2>/dev/null)" = dev.herdr.app ] || continue
    echo "$bundle/Contents/MacOS/$(plutil -extract CFBundleExecutable raw "$plist")"
    return
  done
  die "cannot find the Herdr app; pass --app-bin /Applications/Herdr.app/Contents/MacOS/Herdr"
}

app_bin=$(find_app_bin)
home=$(cd "$home" 2>/dev/null && pwd) || die "no Herdr app home at $home"
export HERDR_APP_HOME=$home
"$app_bin" status >/dev/null 2>&1 || die "open the Herdr app first (home: $home)"

runtime=${XDG_DATA_HOME:-$HOME/.local/share}/herdr-nvim/app
wrapper=$home/mods/herdr-nvim/bin/herdr-nvim

if [ "$mode" = uninstall ]; then
  if [ -x "$wrapper" ]; then
    "$wrapper" app-setup uninstall --keys "$here/keys.json"
    "$wrapper" daemons stop --all --force || true
  fi
  hc kill herdr-nvim-watch >/dev/null 2>&1 || true
  for m in "${mods[@]}"; do rm -rf "${home:?}/mods/$m"; done
  rm -rf "$runtime"
  echo "herdr-nvim: uninstalled from $home"
  exit 0
fi

cmp -s "$here/mods/herdr-nvim/lib.js" "$here/mods/herdr-nvim-pick/lib.js" ||
  die "mods/*/lib.js differ: keep both copies identical"

build_log=$(mktemp)
(cd "$repo" && cargo build --release --quiet 2>"$build_log") || { cat "$build_log" >&2; die "cargo build failed"; }
rm -f "$build_log"
mkdir -p "$runtime/bin"
cp "$repo/target/release/herdr-nvim" "$runtime/bin/.herdr-nvim.new"
mv -f "$runtime/bin/.herdr-nvim.new" "$runtime/bin/herdr-nvim"
rm -rf "$runtime/lua" "$runtime/plugin"
cp -R "$repo/lua" "$repo/plugin" "$runtime/"
# The runtime's stamp: each tab nvim records the one it started with, so one
# started before this install counts as stale (restarted when its pair next
# opens, never stopped here).
(cd "$runtime" && find bin/herdr-nvim lua plugin -type f | LC_ALL=C sort | xargs shasum -a 256 |
  shasum -a 256 | cut -c1-16) > "$runtime/.runtime-stamp.new"
mv -f "$runtime/.runtime-stamp.new" "$runtime/runtime-stamp"

# Each mod folder is swapped in whole, so the app never loads half a copy.
for m in "${mods[@]}"; do
  rm -rf "$home/mods/.$m.new"
  cp -R "$here/mods/$m" "$home/mods/.$m.new"
  rm -rf "${home:?}/mods/$m"
  mv "$home/mods/.$m.new" "$home/mods/$m"
done

mkdir -p "$(dirname "$wrapper")"
cat > "$wrapper" <<EOF
#!/bin/sh
# Written by herdr-nvim's herdr-app/install.sh; run it again to change this.
export HERDR_NVIM_HOST=app
export HERDR_NVIM_SELF='$wrapper'
export HERDR_NVIM_PLUGIN_ROOT='$runtime'
export HERDR_APP_BIN='$app_bin'
export HERDR_APP_HOME='$home'
exec '$runtime/bin/herdr-nvim' "\$@"
EOF
chmod +x "$wrapper"

for m in "${mods[@]}"; do "$app_bin" check "$home/mods/$m"; done
"$wrapper" app-setup install --keys "$here/keys.json" ${panel_width:+--panel-width "$panel_width"}
[ -z "$tree" ] || "$wrapper" app-setup tree "$tree"
echo "herdr-nvim: installed in $home (Files or cmd-e: the file tree with the tab's nvim; cmd-shift-e: into the tree; cmd-o: files from the agent)"
