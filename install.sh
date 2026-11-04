#!/bin/sh
# herdr-nvim's installer, for the Herdr desktop app, the herdr TUI, or both:
#
#   curl -fsSL https://raw.githubusercontent.com/ChmaraX/herdr-nvim/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/ChmaraX/herdr-nvim/main/install.sh | sh -s -- --app
#
#   install.sh [--app] [--tui] [--uninstall] [--install-nvim] [--version vX.Y.Z] [--bundle FILE] [-- app options]
#
# With neither --app nor --tui: whichever Herdr is installed (both if both).
# Run it again to upgrade.
#
# App: downloads the release's app bundle for this machine (the herdr-nvim
# binary, its Lua, the two mods; no clone, no Rust toolchain), checks it
# against the release's SHA-256, and runs the bundle's herdr-app/install.sh
# with the app open. Options after `--` go to that script (--panel-width PX,
# --tree native|nvim, --home DIR, --app-bin PATH, --force). --version picks a
# release (default: the latest), --bundle a bundle file instead of a download.
# Without Neovim 0.10+ it asks to install it (Homebrew's, else Neovim's own
# release, no sudo); --install-nvim says yes up front (no one to ask).
#
# TUI: `herdr plugin install ChmaraX/herdr-nvim` (or `herdr plugin update`);
# its keys are bound by hand, as the README says.
set -eu

repo=ChmaraX/herdr-nvim
plugin=chmarax.herdr-nvim
app= tui= uninstall= version= bundle= install_nvim=
die() { echo "herdr-nvim: $*" >&2; exit 1; }
say() { echo "herdr-nvim: $*"; }

while [ $# -gt 0 ]; do
  case $1 in
    --app) app=1 ;;
    --tui) tui=1 ;;
    --uninstall) uninstall=1 ;;
    --install-nvim) install_nvim=1 ;;
    --version) [ $# -ge 2 ] || die "--version needs a tag"; version=$2; shift ;;
    --bundle) [ $# -ge 2 ] || die "--bundle needs a file"; bundle=$2; shift ;;
    -h|--help) sed -n '2,22p' "$0" 2>/dev/null || echo "see https://github.com/$repo#install"; exit 0 ;;
    --) shift; break ;;
    *) die "unknown argument $1 (app options go after --)" ;;
  esac
  shift
done
# What is left ("$@") goes to herdr-app/install.sh.

# The Herdr app: herdr-app on PATH, a Herdr bundle (macOS, by its bundle id),
# or an app home (Linux, or a launch that never put it on PATH).
have_app() {
  command -v herdr-app >/dev/null 2>&1 && return 0
  for b in /Applications/*.app "$HOME"/Applications/*.app; do
    [ -f "$b/Contents/Info.plist" ] || continue
    [ "$(plutil -extract CFBundleIdentifier raw "$b/Contents/Info.plist" 2>/dev/null)" = dev.herdr.app ] && return 0
  done
  [ -d "${HERDR_APP_HOME:-$HOME/.config/herdr-app}" ]
}
have_tui() { command -v herdr >/dev/null 2>&1; }

if [ -z "$app$tui" ]; then
  have_app && app=1
  have_tui && tui=1
  [ -n "$app$tui" ] || die "found neither the Herdr app nor the herdr TUI; install one first (https://herdr.dev)"
fi

target() {
  case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) echo aarch64-apple-darwin ;;
    Darwin-x86_64) echo x86_64-apple-darwin ;;
    Linux-x86_64) echo x86_64-unknown-linux-gnu ;;
    Linux-aarch64|Linux-arm64) echo aarch64-unknown-linux-gnu ;;
    *) die "no prebuilt herdr-nvim for $(uname -s) $(uname -m); build from source (README: From source)" ;;
  esac
}

sha256() {
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1"; else sha256sum "$1"; fi | cut -d' ' -f1
}

install_app() {
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  if [ -n "$bundle" ]; then
    [ -f "$bundle" ] || die "no bundle at $bundle"
    cp "$bundle" "$work/app.tar.gz"
    [ ! -f "$bundle.sha256" ] || cp "$bundle.sha256" "$work/app.tar.gz.sha256"
  else
    t=$(target)
    name=herdr-nvim-app-$t.tar.gz
    if [ -n "$version" ]; then base=https://github.com/$repo/releases/download/$version
    else base=https://github.com/$repo/releases/latest/download; fi
    say "downloading $name (${version:-latest})"
    curl -fsSL "$base/$name" -o "$work/app.tar.gz" ||
      die "could not download $base/$name (releases before the app bundle have none; build from source: README)"
    curl -fsSL "$base/$name.sha256" -o "$work/app.tar.gz.sha256" || die "could not download $name.sha256"
  fi
  if [ -f "$work/app.tar.gz.sha256" ]; then
    want=$(cut -d' ' -f1 "$work/app.tar.gz.sha256")
    [ "$(sha256 "$work/app.tar.gz")" = "$want" ] || die "the bundle does not match its SHA-256; nothing installed"
  fi
  tar -xzf "$work/app.tar.gz" -C "$work"
  dir=$(find "$work" -mindepth 1 -maxdepth 1 -type d -name 'herdr-nvim-app-*' | head -1)
  [ -n "$dir" ] && [ -x "$dir/bin/herdr-nvim" ] || die "the bundle has no bin/herdr-nvim"
  # The binary runs here (no argument: usage, exit 2); anything else (an
  # older glibc, another architecture) means it can't.
  rc=0
  "$dir/bin/herdr-nvim" >/dev/null 2>&1 || rc=$?
  [ "$rc" -eq 2 ] || die "the prebuilt herdr-nvim does not run on this machine (exit $rc); build from source (README: From source)"
  bash "$dir/herdr-app/install.sh" --binary "$dir/bin/herdr-nvim" ${uninstall:+--uninstall} ${install_nvim:+--install-nvim} "$@"
}

install_tui() {
  command -v herdr >/dev/null 2>&1 || die "the herdr TUI is not installed (https://herdr.dev)"
  listed=$(herdr plugin list 2>/dev/null | grep -F -- "- $plugin " || true)
  if [ -n "$uninstall" ]; then
    [ -n "$listed" ] || { say "the TUI plugin is not installed"; return; }
    herdr plugin uninstall "$plugin"
    say "removed the TUI plugin"
  elif echo "$listed" | grep -q '\[local:'; then
    say "the TUI plugin is linked from a checkout ($listed); left as it is"
  elif [ -n "$listed" ]; then
    herdr plugin update --yes "$plugin"
    say "updated the TUI plugin"
  else
    herdr plugin install --yes "$repo"
    say "installed the TUI plugin; bind its keys in ~/.config/herdr/config.toml (https://github.com/$repo#herdr-tui)"
  fi
}

[ -z "$app" ] || install_app "$@"
[ -z "$tui" ] || install_tui
