#!/usr/bin/env bash
# Screenshots of Settings (every section) and About, dark and light, for design work.
#   ./scripts/design-shots.sh [binary]      default: target/debug/gall
# Uses a private headless compositor when available (kwin_wayland, labwc, weston,
# sway, or Xvfb), or the active desktop (auto-floating on Hyprland/Omarchy).
# Close Gall first. Prints the folder with the screenshots.
set -u
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=scripts/gui-session.sh
. "$REPO_ROOT/scripts/gui-session.sh"
APP="$(realpath "${1:-$REPO_ROOT/target/debug/gall}")"
if pgrep -x gall >/dev/null; then echo "Gall is running; close it first."; exit 2; fi
T="$(mktemp -d -t gall-design-XXXX)"
mkdir -p "$T/config" "$T/data" "$T/out" "$T/td/Projects"
[ -d "$HOME/.local/share/fonts" ] && ln -s "$HOME/.local/share/fonts" "$T/data/fonts"
export XDG_CONFIG_HOME="$T/config" XDG_DATA_HOME="$T/data" GALL_SELFTEST=design GALL_SELFTEST_DIR="$T/out"
BACKEND="$(gall_gui_select_backend)" || exit $?
echo "Design-shots mode: $(gall_gui_backend_label "$BACKEND")" >&2
printf '#!/bin/sh\nexec "%s" -p "%s/td" -p2 "%s/td/Projects"\n' "$APP" "$T" "$T" > "$T/run-app.sh"
chmod +x "$T/run-app.sh"
GALL_GUI_BACKEND="$BACKEND" gall_gui_run "$T" 180 "$T/app.log" "$T/run-app.sh" || true
ls "$T/out"/*.png >/dev/null 2>&1 || { echo "NO SCREENSHOTS"; tail -20 "$T/app.log"; exit 1; }
echo "$T/out"
