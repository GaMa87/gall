#!/usr/bin/env bash
# Start-up time of a built Gall: starts it several times in a private headless KWin
# (own D-Bus session, config and data folders, so the desktop and a running Gall are
# left alone) and prints what the app logs about its own start.
#   ./scripts/startup-time.sh [binary] [runs]     default: target/release/gall, 7 runs
# Build the binary with the UI inside it:
#   npm --prefix ui run build && cargo build -p gall --release --features tauri/custom-protocol
# GALL_STARTUP_DIR=<folder> opens that folder in both panes (default: a small one).
# Requires kwin_wayland. The first start (welcome setup, empty caches) is not counted.
set -u
APP="$(realpath "${1:-target/release/gall}")"
RUNS="${2:-7}"
[ -x "$APP" ] || { echo "no binary at $APP"; exit 2; }
command -v kwin_wayland >/dev/null || { echo "kwin_wayland is required"; exit 2; }
T="$(mktemp -d -t gall-startup-XXXX)"
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/config" "$T/data" "$T/td/Projects"
for i in $(seq 1 40); do echo "file $i" > "$T/td/file$i.txt"; done
DIR="${GALL_STARTUP_DIR:-$T/td}"
[ -d "$HOME/.local/share/fonts" ] && ln -s "$HOME/.local/share/fonts" "$T/data/fonts"
export XDG_CONFIG_HOME="$T/config" XDG_DATA_HOME="$T/data" GALL_MULTI_INSTANCE=1
unset DISPLAY WAYLAND_DISPLAY
printf '#!/bin/sh\nexec "%s" -p "%s" -p2 "%s"\n' "$APP" "$DIR" "$DIR" > "$T/start.sh"
chmod +x "$T/start.sh"
LOG="$T/data/org.gall.app/logs/Gall.log"
rows=()
for run in $(seq 0 "$RUNS"); do
  rm -f "$LOG"
  dbus-run-session -- kwin_wayland --virtual --no-lockscreen --no-global-shortcuts \
    --socket "gall-startup-$$-$run" --width 1920 --height 1200 --exit-with-session "$T/start.sh" \
    > "$T/app.log" 2>&1 &
  session=$!
  for _ in $(seq 1 300); do
    grep -q "startup: bootstrap took" "$LOG" 2>/dev/null && grep -q "startup: window shown" "$LOG" 2>/dev/null && break
    sleep 0.1
  done
  sleep 0.5
  pkill -P "$session" 2>/dev/null; kill "$session" 2>/dev/null; wait "$session" 2>/dev/null
  pkill -f "$T/start.sh" 2>/dev/null; pkill -f "gall-startup-$$-$run" 2>/dev/null
  value() { grep -o "startup: $1[^0-9]*[0-9]*" "$LOG" 2>/dev/null | head -1 | grep -o '[0-9]*$'; }
  setup=$(value "setup at"); created=$(value "window created at"); loaded=$(value "state loaded at")
  script=$(value "UI script running"); boot=$(value "bootstrap took"); shown=$(value "window shown at")
  if [ -z "$shown" ]; then echo "run $run: the window never appeared"; tail -5 "$T/app.log"; continue; fi
  [ "$run" = 0 ] && continue
  rows+=("$setup $created $loaded $script $boot $shown")
  printf 'run %d: setup %s  window created %s  state loaded %s  UI script +%s  bootstrap %s  window shown %s ms\n' \
    "$run" "$setup" "$created" "$loaded" "$script" "$boot" "$shown"
  grep -o "startup: [a-zA-Z]* took [0-9]* ms" "$LOG" | grep -v bootstrap | sed 's/^/        /'
done
[ "${#rows[@]}" -gt 0 ] || exit 1
median() { printf '%s\n' "${rows[@]}" | awk -v c="$1" '{print $c}' | sort -n | awk '{v[NR]=$1} END{print v[int((NR+1)/2)]}'; }
echo "median of ${#rows[@]}: setup $(median 1)  window created $(median 2)  state loaded $(median 3)  UI script +$(median 4)  bootstrap $(median 5)  window shown $(median 6) ms"
