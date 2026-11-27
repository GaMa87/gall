#!/usr/bin/env bash
# Builds Gall from this checkout (bundled UI, release profile) and installs it for the
# current user, so the latest code runs on this Omarchy/Linux system.
#   ./scripts/build-omarchy.sh                 build, then install to ~/.local/bin/gall
#   ./scripts/build-omarchy.sh --no-install    build only (target/release/gall)
#   ./scripts/build-omarchy.sh --dest <file>   install somewhere else
# The binary is replaced atomically, so a running Gall keeps working until restarted.
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO"
DEST="${HOME}/.local/bin/gall"
INSTALL=1
while [ $# -gt 0 ]; do
  case "$1" in
    --no-install) INSTALL=0 ;;
    --dest) shift; DEST="${1:?--dest needs a file path}" ;;
    -h|--help) sed -n '2,7p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
  esac
  shift
done

for t in node npm cargo; do
  command -v "$t" >/dev/null || { echo "missing: $t" >&2; exit 2; }
done
[ -d ui/node_modules ] || npm --prefix ui install

echo "Building the UI…"
npm --prefix ui run build
echo "Building Gall (release)…"
# custom-protocol embeds ui/dist; without it the binary would load the Vite dev server.
cargo build -p gall --release --features tauri/custom-protocol

BIN="target/release/gall"
[ -x "$BIN" ] || { echo "$BIN not found after the build" >&2; exit 1; }
VERSION="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
echo "Built Gall $VERSION ($(git rev-parse --short HEAD 2>/dev/null || echo unknown)): $REPO/$BIN"

if [ "$INSTALL" = 1 ]; then
  mkdir -p "$(dirname "$DEST")"
  install -m755 "$BIN" "$DEST.new"
  mv -f "$DEST.new" "$DEST"
  echo "Installed to $DEST"
  if pgrep -x gall >/dev/null; then
    echo "Gall is running: close it and start it again to use the new build."
  fi
fi
