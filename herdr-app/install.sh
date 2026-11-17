#!/bin/sh
# Installs herdr-nvim from this checkout into the Herdr app:
#
#   herdr-app/install.sh [--uninstall [--force]] [--home DIR] [--app-bin PATH]
#                        [--panel-width PX] [--tree native|nvim] [--install-nvim]
#
# It builds the release binary and runs `target/release/herdr-nvim
# app-install`, which does the install (RECIPE.md says what it changes). With
# --binary PATH (what installers before app-install passed, from a release
# bundle) it runs that binary's instead.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
binary=
for arg in "$@"; do
  shift
  if [ "$binary" = next ]; then binary=$arg; continue; fi
  if [ "$arg" = --binary ]; then binary=next; continue; fi
  set -- "$@" "$arg"
done
if [ -z "$binary" ]; then
  (cd "$here/.." && cargo build --release --quiet) || { echo "herdr-nvim: cargo build failed" >&2; exit 1; }
  binary=$here/../target/release/herdr-nvim
fi
exec "$binary" app-install "$@"
