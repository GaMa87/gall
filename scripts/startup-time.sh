#!/usr/bin/env bash
# Start-up time of a built Gall: starts it several times in a private headless
# compositor when available (kwin_wayland, labwc, weston, sway, or Xvfb), or on
# the active desktop otherwise (own D-Bus session, config and data folders, so
# a running Gall is left alone) and prints what the app logs about its own start.
#   ./scripts/startup-time.sh [binary] [runs]     default: target/release/gall, 7 runs
# Build the binary with the UI inside it:
#   npm --prefix ui run build && cargo build -p gall --release --features tauri/custom-protocol
# GALL_STARTUP_DIR=<folder> opens that folder in both panes (default: a small one).
# Override compositor selection with GALL_GUI_BACKEND=kwin|labwc|weston|sway|xvfb|real.
# The first start (welcome setup, empty caches) is not counted.
set -u
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck source=scripts/gui-session.sh
. "$REPO_ROOT/scripts/gui-session.sh"
APP="$(realpath "${1:-$REPO_ROOT/target/release/gall}")"
RUNS="${2:-7}"
[ -x "$APP" ] || { echo "no binary at $APP"; exit 2; }
BACKEND="$(gall_gui_select_backend)" || exit $?
echo "Startup benchmark mode: $(gall_gui_backend_label "$BACKEND")"
T="$(mktemp -d -t gall-startup-XXXX)"
trap 'rm -rf "$T"' EXIT
mkdir -p "$T/config" "$T/data" "$T/td/Projects"
for i in $(seq 1 40); do echo "file $i" > "$T/td/file$i.txt"; done
DIR="${GALL_STARTUP_DIR:-$T/td}"
[ -d "$HOME/.local/share/fonts" ] && ln -s "$HOME/.local/share/fonts" "$T/data/fonts"
export XDG_CONFIG_HOME="$T/config" XDG_DATA_HOME="$T/data" GALL_MULTI_INSTANCE=1
LOG="$T/data/org.gall.app/logs/Gall.log"
cat > "$T/run-once.sh" <<EOF
#!/bin/sh
"$APP" -p "$DIR" -p2 "$DIR" &
app_pid=\$!
cleanup() { kill "\$app_pid" 2>/dev/null || true; wait "\$app_pid" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
i=0
while [ \$i -lt 300 ]; do
  if grep -q "startup: bootstrap took" "$LOG" 2>/dev/null && grep -q "startup: window shown" "$LOG" 2>/dev/null; then
    break
  fi
  kill -0 "\$app_pid" 2>/dev/null || break
  sleep 0.1
  i=\$((i + 1))
done
sleep 0.5
EOF
chmod +x "$T/run-once.sh"
rows=()
for run in $(seq 0 "$RUNS"); do
  rm -f "$LOG"
  GALL_GUI_BACKEND="$BACKEND" gall_gui_run "$T" 45 "$T/app.log" "$T/run-once.sh" || true
  value() { grep -o "startup: $1[^0-9]*[0-9]*" "$LOG" 2>/dev/null | head -1 | grep -o '[0-9]*$'; }
  setup=$(value "setup at"); created=$(value "window '[a-z0-9-]*' created at"); loaded=$(value "state loaded at")
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
