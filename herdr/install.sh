#!/usr/bin/env bash
# herdr plugin build hook: fetch the prebuilt binary for this platform, or
# build from source as a fallback (reviewr pattern).
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p bin
# Never leave a partial/rejected download behind, on any exit route.
trap 'rm -f bin/herdr-nvim.tmp' EXIT
version=$(sed -n 's/^version = "\(.*\)"/\1/p' herdr-plugin.toml | head -1)
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)  target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-x86_64)  target=x86_64-unknown-linux-gnu ;;
  Linux-aarch64) target=aarch64-unknown-linux-gnu ;;
  *) target="" ;;
esac
url="https://github.com/ChmaraX/herdr-nvim/releases/download/v${version}/herdr-nvim-${target}"

build_from_source() {
  if ! command -v cargo >/dev/null; then
    echo "herdr-nvim: no usable prebuilt binary for ${target:-$(uname -s)-$(uname -m)} and no cargo to build from source" >&2
    exit 1
  fi
  echo "herdr-nvim: building from source" >&2
  cargo build --release
  cp target/release/herdr-nvim bin/herdr-nvim
  chmod +x bin/herdr-nvim
}

if [ -n "$target" ] && curl -fsSL "$url" -o bin/herdr-nvim.tmp; then
  chmod +x bin/herdr-nvim.tmp
  # The release binary is built on ubuntu-latest (glibc ~2.39 as of this
  # writing) and dynamically linked, so it silently refuses to even start
  # on any distro with an older libc -- Debian 12 (glibc 2.36), Ubuntu
  # 22.04 (2.35), RHEL 9 (2.34), etc: "version `GLIBC_2.38' not found".
  # A no-argument invocation is a deterministic, side-effect-free compat
  # probe: every build on every platform hits main()'s default match arm,
  # which prints usage and exits 2 -- so anything OTHER than exit 2 means
  # the binary itself never ran (wrong libc, wrong arch, truncated
  # download, ...), and the source build below is the correct response,
  # not a "the fetch failed" special case.
  # Let the probe's own stderr through: on a broken prebuilt this is where
  # the real reason (e.g. "version `GLIBC_2.38' not found") surfaces, which
  # matters when the source-build fallback is also unavailable.
  status=0
  ./bin/herdr-nvim.tmp >/dev/null || status=$?
  if [ "$status" -eq 2 ]; then
    mv bin/herdr-nvim.tmp bin/herdr-nvim
  else
    echo "herdr-nvim: prebuilt binary for ${target} did not run (exit ${status}); falling back to a source build" >&2
    build_from_source
  fi
else
  build_from_source
fi
