#!/usr/bin/env bash
# herdr plugin build hook: fetch the prebuilt binary for this platform, or
# build from source as a fallback (reviewr pattern).
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p bin
version=$(sed -n 's/^version = "\(.*\)"/\1/p' herdr-plugin.toml | head -1)
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64)  target=x86_64-unknown-linux-gnu ;;
  Linux-aarch64) target=aarch64-unknown-linux-gnu ;;
  *) target="" ;;
esac
url="https://github.com/ChmaraX/herdr-nvim/releases/download/v${version}/herdr-nvim-${target}"
if [ -n "$target" ] && curl -fsSL "$url" -o bin/herdr-nvim.tmp; then
  mv bin/herdr-nvim.tmp bin/herdr-nvim
  chmod +x bin/herdr-nvim
elif command -v cargo >/dev/null; then
  echo "herdr-nvim: no prebuilt binary; building from source" >&2
  cargo build --release
  cp target/release/herdr-nvim bin/herdr-nvim
else
  echo "herdr-nvim: no prebuilt binary for ${target:-$(uname -s)-$(uname -m)} and no cargo" >&2
  exit 1
fi
