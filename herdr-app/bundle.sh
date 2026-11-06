#!/usr/bin/env bash
# Packs the app bundle a release ships for one platform: what the Herdr app
# install needs, with a prebuilt binary, so no clone and no Rust toolchain.
#
#   herdr-app/bundle.sh <herdr-nvim binary> <target> <out dir>
#
# Writes <out dir>/herdr-nvim-app-<target>.tar.gz and its .sha256. Inside,
# one folder laid out like this repo (bin/herdr-nvim, lua/, plugin/,
# herdr-app/{install.sh,install-nvim.sh,keys.json,mods/}), so herdr-app/install.sh runs from
# it as from a clone. The root install.sh downloads and runs it.
set -euo pipefail
[ $# -eq 3 ] || { sed -n '5p' "$0" >&2; exit 2; }
binary=$1 target=$2 out=$3
repo=$(cd "$(dirname "$0")/.." && pwd)
name=herdr-nvim-app-$target
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
b=$work/$name
mkdir -p "$b/bin" "$b/herdr-app/mods"
cp "$binary" "$b/bin/herdr-nvim"
chmod +x "$b/bin/herdr-nvim"
cp -R "$repo/lua" "$repo/plugin" "$b/"
cp "$repo/herdr-app/install.sh" "$repo/herdr-app/install-nvim.sh" "$repo/herdr-app/keys.json" "$b/herdr-app/"
for m in "$repo"/herdr-app/mods/*/; do
  m=$(basename "$m")
  # Not what `herdr-app check` writes into a mod folder in a clone.
  (cd "$repo/herdr-app/mods" && find "$m" -type f ! -name gpui-kit.d.ts ! -name jsconfig.json) |
    while read -r f; do mkdir -p "$b/herdr-app/mods/$(dirname "$f")" && cp "$repo/herdr-app/mods/$f" "$b/herdr-app/mods/$f"; done
done
mkdir -p "$out"
out=$(cd "$out" && pwd)
(cd "$work" && tar -czf "$out/$name.tar.gz" "$name")
(cd "$out" && shasum -a 256 "$name.tar.gz" > "$name.tar.gz.sha256")
echo "$out/$name.tar.gz"
