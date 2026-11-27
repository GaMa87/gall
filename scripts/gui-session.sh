#!/usr/bin/env bash
# Shared headless/real graphical session runner for Linux self-tests, screenshots,
# UI audits and startup benchmarks.
#
# Environment variables:
#   GALL_GUI_BACKEND=auto|kwin|labwc|weston|sway|xvfb|real
#       auto (default): prefers a private headless compositor so mouse, keyboard
#       and screen stay free: kwin_wayland -> labwc -> weston -> sway -> Xvfb,
#       falling back to the active desktop (real).
#   GALL_SELFTEST_REAL=1
#       Shorthand for GALL_GUI_BACKEND=real.
#   GALL_GUI_WIDTH=1920  GALL_GUI_HEIGHT=1200
#       Virtual display dimensions for headless compositors.
#   GALL_GUI_HYPRLAND_FLOAT=1 (default) / 0
#       When running on a live Hyprland desktop (e.g. Omarchy), automatically
#       floats Gall test windows belonging to this run so win.setSize(...) works
#       for narrow/wide layout checks without modifying Hyprland configuration.

gall_gui_select_backend() {
  local req="${GALL_GUI_BACKEND:-}"
  if [ -z "$req" ] && [ -n "${GALL_SELFTEST_REAL:-}" ]; then
    req="real"
  fi
  req="${req:-auto}"

  case "$req" in
    auto)
      if [ "$(uname -s)" = "Darwin" ]; then
        echo "macos"
      elif command -v kwin_wayland >/dev/null 2>&1; then
        echo "kwin"
      elif command -v labwc >/dev/null 2>&1; then
        echo "labwc"
      elif command -v weston >/dev/null 2>&1; then
        echo "weston"
      elif command -v sway >/dev/null 2>&1; then
        echo "sway"
      elif command -v Xvfb >/dev/null 2>&1; then
        echo "xvfb"
      else
        echo "real"
      fi
      ;;
    macos|kwin|labwc|weston|sway|xvfb|real)
      echo "$req"
      ;;
    *)
      echo "Unknown GALL_GUI_BACKEND='$req' (expected auto, macos, kwin, labwc, weston, sway, xvfb, or real)" >&2
      return 2
      ;;
  esac
}

gall_gui_backend_label() {
  case "$1" in
    macos)  echo "macOS native desktop (WKWebView)" ;;
    kwin)   echo "private headless KWin (kwin_wayland)" ;;
    labwc)  echo "private headless labwc" ;;
    weston) echo "private headless Weston" ;;
    sway)   echo "private headless Sway" ;;
    xvfb)   echo "private headless Xvfb (X11)" ;;
    real)
      if [ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ] && command -v hyprctl >/dev/null 2>&1; then
        echo "real desktop (Hyprland, auto-floating test window)"
      else
        echo "real desktop"
      fi
      ;;
    *) echo "$1" ;;
  esac
}

# Stop services activated on a private dbus-run-session bus (portals, ksecretd, ...)
# that outlive the test command.
gall_gui_cleanup_bus() {
  local bus_file="$1"
  local bus
  bus="$(cat "$bus_file" 2>/dev/null || true)"
  [ -n "$bus" ] || return 0
  if command -v python3 >/dev/null 2>&1; then
    python3 - "$bus" <<'PY' 2>/dev/null || true
import os, signal, sys

needle = f"DBUS_SESSION_BUS_ADDRESS={sys.argv[1]}"
skip = {os.getpid(), os.getppid()}
targets = []
for name in os.listdir("/proc"):
    if not name.isdigit():
        continue
    pid = int(name)
    if pid in skip:
        continue
    try:
        with open(f"/proc/{pid}/environ", "rb") as f:
            entries = f.read().decode("utf-8", errors="ignore").split("\0")
    except OSError:
        continue
    if needle in entries:
        targets.append(pid)
for pid in targets:
    try:
        os.kill(pid, signal.SIGTERM)
    except OSError:
        pass
PY
  else
    local p pid
    for p in /proc/[0-9]*; do
      pid="${p#/proc/}"
      [ "$pid" = "$$" ] && continue
      if tr '\0' '\n' < "$p/environ" 2>/dev/null | grep -qxF "DBUS_SESSION_BUS_ADDRESS=$bus" 2>/dev/null; then
        kill "$pid" 2>/dev/null || true
      fi
    done 2>/dev/null
  fi
}

# Run a command inside a private D-Bus session (when dbus-run-session exists),
# recording its bus address for cleanup.
_gall_gui_with_dbus() {
  local timeout_sec="$1"
  local bus_file="$2"
  local log_file="$3"
  shift 3
  if command -v dbus-run-session >/dev/null 2>&1; then
    timeout "$timeout_sec" dbus-run-session -- sh -c '
      bus_file="$1"; shift
      printf "%s\n" "$DBUS_SESSION_BUS_ADDRESS" > "$bus_file"
      exec "$@"
    ' _ "$bus_file" "$@" > "$log_file" 2>&1
  else
    timeout "$timeout_sec" "$@" > "$log_file" 2>&1
  fi
}

# Find a free X11 display number for Xvfb.
_gall_gui_free_x_display() {
  local d
  for d in $(seq 99 199); do
    if [ ! -e "/tmp/.X${d}-lock" ] && [ ! -S "/tmp/.X11-unix/X${d}" ]; then
      echo "$d"
      return 0
    fi
  done
  return 1
}

# On a live Hyprland desktop (such as Omarchy), tiling overrides Tauri's
# win.setSize(...) calls used for narrow/wide layout checks. Float only windows
# whose process environment carries our unique session token.
_gall_gui_start_hyprland_floater() {
  local token="$1"
  [ "${GALL_GUI_HYPRLAND_FLOAT:-1}" = "0" ] && return 0
  [ -n "${HYPRLAND_INSTANCE_SIGNATURE:-}" ] || return 0
  command -v hyprctl >/dev/null 2>&1 || return 0
  command -v python3 >/dev/null 2>&1 || return 0

  python3 - "$token" <<'PY' >/dev/null 2>&1 &
import json, os, subprocess, sys, time

token = f"GALL_GUI_RUN_TOKEN={sys.argv[1]}"
floated = set()
deadline = time.monotonic() + 600

def pid_matches(pid: int) -> bool:
    try:
        with open(f"/proc/{pid}/environ", "rb") as f:
            entries = f.read().decode("utf-8", errors="ignore").split("\0")
        return token in entries
    except OSError:
        return False

while time.monotonic() < deadline:
    try:
        out = subprocess.check_output(["hyprctl", "clients", "-j"], timeout=2)
        clients = json.loads(out)
    except Exception:
        time.sleep(0.2)
        continue
    for client in clients:
        addr = client.get("address")
        pid = client.get("pid")
        if not addr or not isinstance(pid, int) or addr in floated:
            continue
        if not pid_matches(pid):
            continue
        if not client.get("floating", False) or client.get("fullscreen", 0) != 0:
            lua = (
                f'for _, w in ipairs(hl.get_windows()) do '
                f'if w.pid == {pid} then '
                f'hl.dispatch(hl.dsp.window.fullscreen({{ mode = "fullscreen", action = "unset", window = w }})); '
                f'hl.dispatch(hl.dsp.window.fullscreen({{ mode = "maximized", action = "unset", window = w }})); '
                f'hl.dispatch(hl.dsp.window.float({{ action = "on", window = w }})); '
                f'hl.dispatch(hl.dsp.window.center({{ window = w }})); '
                f'end end'
            )
            res = subprocess.run(
                ["hyprctl", "eval", lua],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=2,
            )
            if res.returncode != 0 or "error:" in (res.stdout + res.stderr).lower():
                subprocess.run(
                    ["hyprctl", "dispatch", "setfloating", f"address:{addr}"],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=2,
                )
                subprocess.run(
                    ["hyprctl", "dispatch", "centerwindow"],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=2,
                )
        floated.add(addr)
    time.sleep(0.15)
PY
  echo "$!"
}

# Usage: gall_gui_run <work_dir> <timeout_sec> <log_file> <entry_script>
# Runs <entry_script> inside the selected graphical backend, cleans up any
# private compositor / D-Bus processes afterwards, and returns its exit code.
gall_gui_run() {
  local work_dir="$1"
  local timeout_sec="$2"
  local log_file="$3"
  local entry_script="$4"

  local backend width height bus_file token sock rc=0
  backend="$(gall_gui_select_backend)" || return $?
  width="${GALL_GUI_WIDTH:-1920}"
  height="${GALL_GUI_HEIGHT:-1200}"
  bus_file="$work_dir/bus"
  token="gall-gui-$$-$(date +%s%N 2>/dev/null || echo 0)"
  sock="gall-gui-$$-$RANDOM"
  rm -f "$bus_file"

  case "$backend" in
    macos)
      python3 - "$timeout_sec" "$log_file" "$entry_script" <<'PY' || rc=$?
import os, signal, subprocess, sys

timeout_sec = int(sys.argv[1])
log_file = sys.argv[2]
entry = sys.argv[3]
with open(log_file, "wb") as log:
    proc = subprocess.Popen([entry], stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
    try:
        rc = proc.wait(timeout=timeout_sec)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except OSError:
            pass
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except OSError:
                pass
        sys.exit(124)
    sys.exit(rc)
PY
      return "$rc"
      ;;

    kwin)
      command -v kwin_wayland >/dev/null 2>&1 || { echo "kwin_wayland not found" >&2; return 2; }
      (
        unset DISPLAY WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE GALL_SELFTEST_DBUS
        export GALL_GUI_RUN_TOKEN="$token"
        _gall_gui_with_dbus "$timeout_sec" "$bus_file" "$log_file" \
          kwin_wayland --virtual --no-lockscreen --no-global-shortcuts \
            --socket "$sock" --width "$width" --height "$height" \
            --exit-with-session "$entry_script"
      ) || rc=$?
      ;;

    labwc)
      command -v labwc >/dev/null 2>&1 || { echo "labwc not found" >&2; return 2; }
      local cfg="$work_dir/labwc-cfg"
      mkdir -p "$cfg"
      cat > "$cfg/rc.xml" <<'XML'
<?xml version="1.0"?>
<labwc_config>
  <core><decoration>client</decoration></core>
</labwc_config>
XML
      local wrap="$work_dir/labwc-run.sh"
      cat > "$wrap" <<EOF
#!/bin/sh
if command -v wlr-randr >/dev/null 2>&1; then
  wlr-randr --output HEADLESS-1 --custom-mode "${width}x${height}" >/dev/null 2>&1 || true
fi
exec "$entry_script"
EOF
      chmod +x "$wrap"
      (
        unset DISPLAY WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE GALL_SELFTEST_DBUS
        export WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_HEADLESS_OUTPUTS=1
        export GALL_GUI_RUN_TOKEN="$token"
        _gall_gui_with_dbus "$timeout_sec" "$bus_file" "$log_file" \
          labwc -C "$cfg" -S "$wrap"
      ) || rc=$?
      ;;

    weston)
      command -v weston >/dev/null 2>&1 || { echo "weston not found" >&2; return 2; }
      local wrap="$work_dir/weston-run.sh"
      cat > "$wrap" <<EOF
#!/bin/sh
runtime_dir="\${XDG_RUNTIME_DIR:-/tmp}"
weston --backend=headless-backend.so --no-config --idle-time=0 \
  --width="$width" --height="$height" --socket="$sock" &
wpid=\$!
cleanup() { kill "\$wpid" 2>/dev/null || true; wait "\$wpid" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
i=0
while [ ! -S "\$runtime_dir/$sock" ] && [ \$i -lt 100 ]; do
  sleep 0.1
  i=\$((i + 1))
done
if [ ! -S "\$runtime_dir/$sock" ]; then
  echo "weston headless socket did not appear" >&2
  exit 1
fi
WAYLAND_DISPLAY="$sock" "$entry_script"
EOF
      chmod +x "$wrap"
      (
        unset DISPLAY WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE GALL_SELFTEST_DBUS
        export GALL_GUI_RUN_TOKEN="$token"
        _gall_gui_with_dbus "$timeout_sec" "$bus_file" "$log_file" "$wrap"
      ) || rc=$?
      ;;

    sway)
      command -v sway >/dev/null 2>&1 || { echo "sway not found" >&2; return 2; }
      local cfg="$work_dir/sway.conf"
      local rc_file="$work_dir/sway.rc"
      local runner="$work_dir/sway-run.sh"
      cat > "$runner" <<EOF
#!/bin/sh
"$entry_script"
printf '%s\n' "\$?" > "$rc_file"
swaymsg exit >/dev/null 2>&1 || true
EOF
      chmod +x "$runner"
      cat > "$cfg" <<EOF
output HEADLESS-1 resolution ${width}x${height}
default_border none
for_window [app_id=".*"] floating enable
for_window [class=".*"] floating enable
exec "$runner"
EOF
      (
        unset DISPLAY WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE GALL_SELFTEST_DBUS
        export WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 SWAYSOCK=""
        export GALL_GUI_RUN_TOKEN="$token"
        _gall_gui_with_dbus "$timeout_sec" "$bus_file" "$log_file" \
          sway -c "$cfg"
      ) || rc=$?
      if [ -s "$rc_file" ]; then
        rc="$(cat "$rc_file" 2>/dev/null || echo "$rc")"
      fi
      ;;

    xvfb)
      command -v Xvfb >/dev/null 2>&1 || { echo "Xvfb not found" >&2; return 2; }
      local disp
      disp="$(_gall_gui_free_x_display)" || { echo "No free X11 display for Xvfb" >&2; return 2; }
      local wrap="$work_dir/xvfb-run.sh"
      cat > "$wrap" <<EOF
#!/bin/sh
Xvfb ":$disp" -screen 0 "${width}x${height}x24" -nolisten tcp &
xpid=\$!
cleanup() { kill "\$xpid" 2>/dev/null || true; wait "\$xpid" 2>/dev/null || true; }
trap cleanup EXIT INT TERM
i=0
while [ ! -S "/tmp/.X11-unix/X$disp" ] && [ \$i -lt 100 ]; do
  sleep 0.1
  i=\$((i + 1))
done
if [ ! -S "/tmp/.X11-unix/X$disp" ]; then
  echo "Xvfb display :$disp did not start" >&2
  exit 1
fi
DISPLAY=":$disp" GDK_BACKEND=x11 "$entry_script"
EOF
      chmod +x "$wrap"
      (
        unset DISPLAY WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE GALL_SELFTEST_DBUS
        export GALL_GUI_RUN_TOKEN="$token"
        _gall_gui_with_dbus "$timeout_sec" "$bus_file" "$log_file" "$wrap"
      ) || rc=$?
      ;;

    real)
      if [ -z "${WAYLAND_DISPLAY:-}" ] && [ -z "${DISPLAY:-}" ]; then
        echo "No active graphical session (WAYLAND_DISPLAY/DISPLAY unset) and no headless compositor found." >&2
        echo "Install one of: kwin_wayland (KDE), labwc, weston, sway, or Xvfb (xorg-server-xvfb)." >&2
        return 2
      fi
      local floater_pid=""
      floater_pid="$(_gall_gui_start_hyprland_floater "$token")"
      (
        export GALL_SELFTEST_DBUS="${DBUS_SESSION_BUS_ADDRESS:-}"
        export GALL_GUI_RUN_TOKEN="$token"
        _gall_gui_with_dbus "$timeout_sec" "$bus_file" "$log_file" "$entry_script"
      ) || rc=$?
      if [ -n "$floater_pid" ]; then
        kill "$floater_pid" 2>/dev/null || true
        wait "$floater_pid" 2>/dev/null || true
      fi
      ;;
  esac

  gall_gui_cleanup_bus "$bus_file"
  return "$rc"
}
