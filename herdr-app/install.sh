#!/usr/bin/env bash
# Install herdr-nvim into the Herdr desktop app, or take it out again.
#
#   herdr-app/install.sh [--home DIR] [--app-bin PATH]
#   herdr-app/install.sh --uninstall [--home DIR] [--app-bin PATH]
#
# The Herdr app must be open on that home (default: $HERDR_APP_HOME, else
# ~/.config/herdr-app). Install builds herdr-nvim, copies its runtime (binary +
# lua) to ${XDG_DATA_HOME:-~/.local/share}/herdr-nvim/app, copies the two mods
# into <home>/mods with a wrapper at mods/herdr-nvim/bin/herdr-nvim, adds the
# keys from keys.json where they're free, and points the layout's Files slot at
# herdr-nvim (cloning Herdr's own layout as <name>-mods first). Both are safe
# to run again. Uninstall undoes each step and stops the app's nvim daemons.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(dirname "$here")
mode=install
home=${HERDR_APP_HOME:-$HOME/.config/herdr-app}
app_bin=${HERDR_APP_BIN:-}
mods=(herdr-nvim herdr-nvim-pick)

die() { echo "herdr-nvim: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case $1 in
    --uninstall) mode=uninstall ;;
    --home) home=$2; shift ;;
    --app-bin) app_bin=$2; shift ;;
    -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
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
"$wrapper" app-setup install --keys "$here/keys.json"
echo "herdr-nvim: installed in $home (Files: native tree that opens files in nvim; cmd-e: nvim sidebar; cmd-o: open file from agent output)"
