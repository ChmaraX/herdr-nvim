#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p bin
if command -v cargo >/dev/null; then
  cargo build --release
  cp target/release/herdr-nvim bin/herdr-nvim
else
  echo "herdr-nvim: cargo not found; install Rust or use a release build" >&2
  exit 1
fi
