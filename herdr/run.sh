#!/usr/bin/env bash
# herdr runs plugin commands with a minimal PATH; ensure nvim/herdr resolve on
# common install locations regardless of the invoking shell's environment
# (same pattern as adamchmara.gitview's herdr/run.sh).
set -euo pipefail
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:${PATH:-}"

if [ -n "${HERDR_PLUGIN_ROOT:-}" ]; then
  BIN="$HERDR_PLUGIN_ROOT/bin/herdr-nvim"
else
  BIN="$(cd "$(dirname "$0")/.." && pwd)/bin/herdr-nvim"
fi

exec "$BIN" "$@"
