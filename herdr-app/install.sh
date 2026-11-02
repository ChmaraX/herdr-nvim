#!/usr/bin/env bash
# A convenience helper: does what RECIPE.md describes, the way AGENTS.md says
# to add a mod, so an agent can do the same by hand.
#
#   herdr-app/install.sh [--home DIR] [--app-bin PATH] [--panel-width PX] [--tree native|nvim]
#   herdr-app/install.sh --uninstall [--force] [--home DIR] [--app-bin PATH]
#
# The Herdr app must be open on that home (default: $HERDR_APP_HOME, else
# ~/.config/herdr-app). Install builds herdr-nvim, copies its runtime (binary +
# lua) to ${XDG_DATA_HOME:-~/.local/share}/herdr-nvim/app/<stamp>, copies the
# mod into <home>/mods with a wrapper at mods/herdr-nvim/bin/herdr-nvim
# (pointing at that runtime), runs `herdr-app check` on it, merges keys.json into the person's keys (only
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
# app's nvims; one with unsaved buffers or pending comments is listed and
# kept (with its runtime) unless --force.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(dirname "$here")
mode=install
home=${HERDR_APP_HOME:-$HOME/.config/herdr-app}
app_bin=${HERDR_APP_BIN:-}
panel_width=
tree=
force=
mods=(herdr-nvim)

die() { echo "herdr-nvim: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case $1 in
    --uninstall) mode=uninstall ;;
    --force) force=1 ;;
    --home) home=$2; shift ;;
    --app-bin) app_bin=$2; shift ;;
    --panel-width) panel_width=$2; shift ;;
    --tree) tree=$2; shift ;;
    -h|--help) sed -n '2,27p' "$0"; exit 0 ;;
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

# The picker was its own mod (herdr-nvim-pick) before it joined herdr-nvim as
# `herdr-nvim.pick`: drop that folder, when it is that mod.
retire_pick() {
  grep -qs '"nvim: open file from agent output"' "$home/mods/herdr-nvim-pick/mod.json" &&
    rm -rf "${home:?}/mods/herdr-nvim-pick"
  return 0
}

# The runtime this install's wrapper runs (its plugin root), if there is one.
wrapper_root() {
  sed -n "s/^export HERDR_NVIM_PLUGIN_ROOT='\(.*\)'$/\1/p" "$wrapper" 2>/dev/null
}

# Whether a running process (a tab nvim: `--cmd set rtp+=<root>`) uses `root`.
in_use() {
  pgrep -f -- "set rtp\+=$1( |\$)" >/dev/null 2>&1
}

# Runtimes no running nvim uses, except `keep`: removed. The layout from
# before versioned runtimes ($runtime/{bin,lua,plugin}) counts as one.
prune_runtimes() {
  local keep=$1 dir
  for dir in "$runtime"/*/; do
    dir=${dir%/}
    [ -f "$dir/runtime-stamp" ] || continue
    [ "$dir" = "$keep" ] || in_use "$dir" || rm -rf "$dir"
  done
  if [ -d "$runtime/lua" ] && [ "$runtime" != "$keep" ] && ! in_use "$runtime"; then
    rm -rf "$runtime/bin" "$runtime/lua" "$runtime/plugin" "$runtime/runtime-stamp"
  fi
}

# herdr-nvim as the app's mods run it, from `root` (a runtime, or a build).
run_root() {
  local root=$1; shift
  HERDR_NVIM_HOST=app HERDR_NVIM_PLUGIN_ROOT="$root" HERDR_APP_BIN="$app_bin" HERDR_APP_HOME="$home" \
    "$root/bin/herdr-nvim" "$@"
}

if [ "$mode" = uninstall ]; then
  # Through a runtime binary, not the wrapper: a half-replaced mod folder
  # must not keep the layouts and keys from being taken back. No runtime
  # left: this repo's own build.
  root=$(wrapper_root)
  [ -x "$root/bin/herdr-nvim" ] || root=$(ls -dt "$runtime"/*/ 2>/dev/null | head -1 | sed 's:/$::')
  [ -x "$root/bin/herdr-nvim" ] || root=$runtime
  if [ ! -x "$root/bin/herdr-nvim" ]; then
    (cd "$repo" && cargo build --release --quiet) || die "cargo build failed"
    root=$(mktemp -d)
    mkdir -p "$root/bin" && cp "$repo/target/release/herdr-nvim" "$root/bin/"
    cp -R "$repo/lua" "$repo/plugin" "$root/"
  fi
  run_root "$root" app-setup uninstall --keys "$here/keys.json"
  # Unsaved work is never thrown away unasked: those nvims are listed and
  # kept, with the runtime they run.
  if ! stopped=$(run_root "$root" daemons stop --all ${force:+--force} 2>&1); then
    echo "$stopped" >&2
    echo "herdr-nvim: kept the nvims above; save (:wa) and quit them, or run --uninstall --force to discard their work" >&2
  elif [ -n "$stopped" ]; then
    echo "$stopped"
  fi
  hc kill herdr-nvim-watch >/dev/null 2>&1 || true
  for m in "${mods[@]}"; do rm -rf "${home:?}/mods/$m"; done
  retire_pick
  prune_runtimes ""
  rmdir "$runtime" 2>/dev/null || true
  echo "herdr-nvim: uninstalled from $home"
  exit 0
fi

build_log=$(mktemp)
(cd "$repo" && cargo build --release --quiet 2>"$build_log") || { cat "$build_log" >&2; die "cargo build failed"; }
rm -f "$build_log"

# The runtime goes into its own folder, named by its stamp, and the wrapper
# points at it: a tab nvim already running keeps the runtime it started with,
# whole (never new Lua modules loaded beside old ones), and is stale when its
# runtime isn't the wrapper's (restarted when its pair next opens, never
# stopped here).
mkdir -p "$runtime"
staged=$(mktemp -d "$runtime/.new.XXXXXX")
mkdir -p "$staged/bin"
cp "$repo/target/release/herdr-nvim" "$staged/bin/herdr-nvim"
cp -R "$repo/lua" "$repo/plugin" "$staged/"
stamp=$(cd "$staged" && find bin/herdr-nvim lua plugin -type f | LC_ALL=C sort | xargs shasum -a 256 |
  shasum -a 256 | cut -c1-16)
echo "$stamp" > "$staged/runtime-stamp"
root=$runtime/$stamp
if [ -d "$root" ]; then
  # The same runtime again: only its stamp file is made right.
  mv -f "$staged/runtime-stamp" "$root/runtime-stamp"
  rm -rf "$staged"
else
  mv "$staged" "$root"
fi

# Each mod folder is made whole beside the live one (with its wrapper),
# checked, then swapped in by a rename; the old one comes back if the swap
# fails, so the app never loads half a copy and an uninstall always finds
# a runtime.
for m in "${mods[@]}"; do
  stage=$home/mods/.$m.new old=$home/mods/.$m.old
  new=$stage/$m # named as the mod, for the check
  rm -rf "$stage" "$old"
  mkdir -p "$stage"
  cp -R "$here/mods/$m" "$new"
  if [ "$m" = herdr-nvim ]; then
    mkdir -p "$new/bin"
    cat > "$new/bin/herdr-nvim" <<WRAPPER
#!/bin/sh
# Written by herdr-nvim's herdr-app/install.sh; run it again to change this.
export HERDR_NVIM_HOST=app
export HERDR_NVIM_SELF='$wrapper'
export HERDR_NVIM_PLUGIN_ROOT='$root'
export HERDR_APP_BIN='$app_bin'
export HERDR_APP_HOME='$home'
exec '$root/bin/herdr-nvim' "\$@"
WRAPPER
    chmod +x "$new/bin/herdr-nvim"
  fi
  "$app_bin" check "$new" || { rm -rf "$stage"; die "herdr-app check failed for $m; nothing replaced"; }
  [ ! -e "$home/mods/$m" ] || mv "$home/mods/$m" "$old"
  if mv "$new" "$home/mods/$m"; then
    rm -rf "$old" "$stage"
  else
    [ ! -e "$old" ] || mv "$old" "$home/mods/$m"
    die "could not put $m in place; the previous one is back"
  fi
done
retire_pick
prune_runtimes "$root"

"$wrapper" app-setup install --keys "$here/keys.json" ${panel_width:+--panel-width "$panel_width"}
[ -z "$tree" ] || "$wrapper" app-setup tree "$tree"
echo "herdr-nvim: installed in $home (Files or cmd-e: the file tree with the tab's nvim; cmd-shift-e: into the tree; cmd-o: files from the agent)"
