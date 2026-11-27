#!/usr/bin/env bash
# Run Gall's title-bar click scenario on the active Hyprland session and save
# compositor state plus socket events before and after the live app run.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
if [ -z "${HYPRLAND_INSTANCE_SIGNATURE:-}" ] || ! command -v hyprctl >/dev/null 2>&1 || ! command -v socat >/dev/null 2>&1; then
  echo "Run this check from a Hyprland session with hyprctl and socat installed." >&2
  exit 2
fi

socket="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock"
if [ ! -S "$socket" ]; then
  echo "Hyprland event socket not found: $socket" >&2
  exit 2
fi

out="$(mktemp -d -t gall-titlebar-hyprland-XXXX)"
hyprctl monitors -j > "$out/before-monitors.json"
hyprctl clients -j > "$out/before-clients.json"
hyprctl activeworkspace -j > "$out/before-workspace.json"
socat -u "UNIX-CONNECT:$socket" - > "$out/events.log" 2>&1 &
events_pid=$!
cleanup() {
  kill "$events_pid" 2>/dev/null || true
  wait "$events_pid" 2>/dev/null || true
}
trap cleanup EXIT
sleep 0.2

set +e
GALL_GUI_BACKEND=real GALL_GUI_HYPRLAND_FLOAT=0 \
  "$repo_root/scripts/selftest.sh" --titlebar "${1:-$repo_root/target/debug/gall}" > "$out/selftest.log" 2>&1
status=$?
set -e

hyprctl monitors -j > "$out/after-monitors.json"
hyprctl clients -j > "$out/after-clients.json"
hyprctl activeworkspace -j > "$out/after-workspace.json"
cleanup
trap - EXIT
cat "$out/selftest.log"
printf '\nHyprland evidence: %s\n' "$out"
printf 'Events: %s\n' "$out/events.log"
exit "$status"
