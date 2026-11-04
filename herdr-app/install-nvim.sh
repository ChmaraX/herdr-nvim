#!/bin/sh
# Installs Neovim for herdr-nvim, which needs 0.10 or newer:
# - with Homebrew: `brew install neovim` (or `brew upgrade neovim`);
# - without: Neovim's official release for this machine, checked against the
#   SHA-256 GitHub lists for it, into ${XDG_DATA_HOME:-~/.local/share}/herdr-nvim/nvim
#   (no sudo, the system's own nvim untouched), and herdr-nvim's
#   `[sidebar] nvim_bin` pointed at it.
#
#   install-nvim.sh --herdr-nvim PATH [--method brew|download] [--pause]
#
# --herdr-nvim: the herdr-nvim binary (it checks the version and writes the
# setting). --pause: wait for Enter at the end (the app's terminal tab).
# Run by herdr-app/install.sh (after the person said yes) and by the app's
# "Neovim is needed" card (`herdr-nvim app-install-nvim`).
set -eu

hn= method= pause=
while [ $# -gt 0 ]; do
  case $1 in
    --herdr-nvim) hn=$2; shift ;;
    --method) method=$2; shift ;;
    --pause) pause=1 ;;
    *) echo "install-nvim: unknown argument $1" >&2; exit 2 ;;
  esac
  shift
done
[ -n "$hn" ] || { echo "install-nvim: --herdr-nvim PATH is needed" >&2; exit 2; }

say() { echo "herdr-nvim: $*"; }
finish() {
  status=$1
  if [ -n "$pause" ]; then
    printf '\nPress Enter to close this tab. '
    read -r _ || true
  fi
  exit "$status"
}
die() { echo "herdr-nvim: $*" >&2; finish 1; }
ok() { "$hn" app-nvim-check >/dev/null 2>&1; }

if ok; then
  say "Neovim is already there: $("$hn" app-nvim-check)"
  finish 0
fi
[ -n "$method" ] || { command -v brew >/dev/null 2>&1 && method=brew || method=download; }

case $method in
  brew)
    if brew list neovim >/dev/null 2>&1; then
      say "upgrading Homebrew's Neovim (brew upgrade neovim)"
      brew upgrade neovim || die "brew upgrade neovim failed"
    else
      say "installing Neovim with Homebrew (brew install neovim)"
      brew install neovim || die "brew install neovim failed"
    fi
    # Not the one herdr-nvim finds (another nvim first on its PATH): point
    # it at Homebrew's.
    ok || "$hn" app-config nvim-bin "$(brew --prefix)/bin/nvim"
    ;;
  download)
    case "$(uname -s)-$(uname -m)" in
      Darwin-arm64) asset=nvim-macos-arm64 ;;
      Darwin-x86_64) asset=nvim-macos-x86_64 ;;
      Linux-x86_64) asset=nvim-linux-x86_64 ;;
      Linux-aarch64|Linux-arm64) asset=nvim-linux-arm64 ;;
      *) die "no Neovim release for $(uname -s) $(uname -m); install it yourself (https://neovim.io)" ;;
    esac
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    say "looking up Neovim's latest release"
    curl -fsSL https://api.github.com/repos/neovim/neovim/releases/latest -o "$work/release.json" ||
      die "could not reach GitHub's API"
    # The asset's entry, as GitHub's API lays it out: its "name", then its
    # "digest" and "browser_download_url".
    field() { # field <key>: that key's string in the asset's entry
      awk -v want="\"$asset.tar.gz\"" -v key="\"$1\":" '
        /"name":/ { inside = index($0, want) > 0 }
        inside && index($0, key) { sub(/^[^:]*: *"/, ""); sub(/".*/, ""); print; exit }
      ' "$work/release.json"
    }
    url=$(field browser_download_url)
    digest=$(field digest)
    digest=${digest#sha256:}
    [ -n "${url:-}" ] || die "the latest Neovim release has no $asset.tar.gz"
    [ -n "${digest:-}" ] || die "GitHub lists no SHA-256 for $asset.tar.gz; not installed"
    tag=$(sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' "$work/release.json" | head -1)
    say "downloading Neovim $tag ($asset)"
    curl -fL --progress-bar "$url" -o "$work/nvim.tar.gz" || die "the download failed"
    if command -v shasum >/dev/null 2>&1; then got=$(shasum -a 256 "$work/nvim.tar.gz")
    else got=$(sha256sum "$work/nvim.tar.gz"); fi
    [ "${got%% *}" = "$digest" ] || die "the download does not match its SHA-256; not installed"
    dest=${XDG_DATA_HOME:-$HOME/.local/share}/herdr-nvim/nvim
    mkdir -p "$dest"
    tar -xzf "$work/nvim.tar.gz" -C "$work"
    rm -rf "$dest/$asset"
    mv "$work/$asset" "$dest/$asset"
    bin=$dest/$asset/bin/nvim
    "$bin" --version >/dev/null 2>&1 || die "the downloaded Neovim does not run on this machine ($bin)"
    "$hn" app-config nvim-bin "$bin" >/dev/null
    say "installed in $dest/$asset; herdr-nvim's nvim_bin points at it"
    ;;
  *) die "--method is brew or download, not $method" ;;
esac

ok || die "Neovim is still not usable: $("$hn" app-nvim-check 2>&1 || true)"
say "ready: $("$hn" app-nvim-check). Press cmd-e (or Files) to open nvim."
finish 0
