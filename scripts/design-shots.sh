#!/usr/bin/env bash
# Screenshots of Settings (every section) and About, dark and light, for design work.
#   ./scripts/design-shots.sh [binary]      default: target/debug/gall
# Requires KDE Spectacle. Close Gall first. Prints the folder with the screenshots.
set -u
APP="$(realpath "${1:-target/debug/gall}")"
if pgrep -x gall >/dev/null; then echo "Gall is running; close it first."; exit 2; fi
T="$(mktemp -d -t gall-design-XXXX)"
mkdir -p "$T/config" "$T/data" "$T/out" "$T/td/Projects"
[ -d "$HOME/.local/share/fonts" ] && ln -s "$HOME/.local/share/fonts" "$T/data/fonts"
export GALL_SELFTEST_DBUS="${DBUS_SESSION_BUS_ADDRESS:-}"
export XDG_CONFIG_HOME="$T/config" XDG_DATA_HOME="$T/data" GALL_SELFTEST=design GALL_SELFTEST_DIR="$T/out"
timeout 180 dbus-run-session -- "$APP" -p "$T/td" -p2 "$T/td/Projects" > "$T/app.log" 2>&1
ls "$T/out"/*.png >/dev/null 2>&1 || { echo "NO SCREENSHOTS"; tail -20 "$T/app.log"; exit 1; }
echo "$T/out"
