#!/usr/bin/env bash
# Compare Gall against Nautilus (+ nautilus-python + sushi + ffmpegthumbnailer)
# on Arch/Omarchy package/ISO footprint, RAM (RSS/PSS via /proc/<pid>/smaps_rollup),
# and cold/warm-start wall-clock time inside isolated dbus-run-session sandboxes.
#
# Usage:
#   scripts/benchmark-nautilus.sh [options]
#
# Options:
#   --json              Emit machine-readable JSON on stdout (or --output file)
#   --tsv               Emit machine-readable TSV rows
#   --output <file>     Write report to <file> instead of stdout
#   --gall <path>       Path to gall binary (default: GALL_BIN, target/release/gall,
#                       target/debug/gall, or gall on PATH)
#   --runs <N>          Measured GUI runs after 1 discarded warmup (default: 5)
#   --dir <dir>         Directory to open in GUI benchmarks (default: temp dir)
#   --footprint-only    Run package/ISO footprint phase only (skip GUI/RAM/startup)
#   --skip-gui          Alias for --footprint-only
#   -h, --help          Show this help
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FORMAT="table"
OUTPUT_FILE=""
GALL_ARG="${GALL_BIN:-}"
RUNS=5
TARGET_DIR=""
SKIP_GUI=0

usage() {
  sed -n '2,19p' "$0" | sed 's/^# \{0,1\}//'
}

while (($# > 0)); do
  case "$1" in
    --json) FORMAT="json"; shift ;;
    --tsv) FORMAT="tsv"; shift ;;
    --output)
      [ $# -ge 2 ] || { echo "--output requires a path" >&2; exit 2; }
      OUTPUT_FILE="$2"; shift 2 ;;
    --gall)
      [ $# -ge 2 ] || { echo "--gall requires a path" >&2; exit 2; }
      GALL_ARG="$2"; shift 2 ;;
    --runs)
      [ $# -ge 2 ] || { echo "--runs requires a positive integer" >&2; exit 2; }
      RUNS="$2"; shift 2 ;;
    --dir)
      [ $# -ge 2 ] || { echo "--dir requires a directory path" >&2; exit 2; }
      TARGET_DIR="$2"; shift 2 ;;
    --footprint-only|--skip-gui) SKIP_GUI=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

case "$RUNS" in
  ''|*[!0-9]*|0) echo "--runs must be a positive integer (got '$RUNS')" >&2; exit 2 ;;
esac

if ! command -v python3 >/dev/null 2>&1; then
  echo "python3 is required" >&2
  exit 1
fi

TMP_ROOT="$(mktemp -d -t gall-bench-XXXX)"
cleanup() {
  rm -rf "$TMP_ROOT"
}
trap cleanup EXIT

if [ -z "$TARGET_DIR" ]; then
  TARGET_DIR="$TMP_ROOT/bench-dir"
  mkdir -p "$TARGET_DIR/Projects"
  for i in $(seq 1 40); do
    printf 'benchmark file %d\n' "$i" > "$TARGET_DIR/file$i.txt"
  done
elif [ ! -d "$TARGET_DIR" ]; then
  echo "--dir '$TARGET_DIR' is not a directory" >&2
  exit 2
fi

if [ -f "$REPO_ROOT/scripts/gui-session.sh" ]; then
  # shellcheck source=scripts/gui-session.sh
  . "$REPO_ROOT/scripts/gui-session.sh"
fi

REPORT_OUT="$(python3 - "$REPO_ROOT" "$TMP_ROOT" "$FORMAT" "$GALL_ARG" "$RUNS" "$TARGET_DIR" "$SKIP_GUI" <<'PY'
import datetime
import json
import os
import re
import shutil
import signal
import statistics
import subprocess
import sys
import time
import uuid

(
    repo_root,
    tmp_root,
    out_format,
    gall_arg,
    runs_str,
    target_dir,
    skip_gui_str,
) = sys.argv[1:8]

runs = int(runs_str)
skip_gui = skip_gui_str == "1"

NAUTILUS_PKGS = ["nautilus", "nautilus-python", "sushi", "ffmpegthumbnailer"]
GALL_RUNTIME_DEPS = ["webkit2gtk-4.1", "gtk3"]
GALL_OPTIONAL_DEPS = ["gst-plugins-good"]
OMARCHY_BASE_PKG_FILE = os.environ.get(
    "OMARCHY_BASE_PACKAGES", "/usr/share/omarchy/install/omarchy-base.packages"
)


def fmt_mib(b):
    if b is None:
        return "n/a"
    return f"{b / (1024 * 1024):.2f} MiB"


def discover_gall_binary():
    candidates = []
    if gall_arg:
        candidates.append((gall_arg, "explicit"))
    else:
        candidates.append((os.path.join(repo_root, "target", "release", "gall"), "release"))
        candidates.append((os.path.join(repo_root, "target", "debug", "gall"), "debug"))
        on_path = shutil.which("gall")
        if on_path:
            candidates.append((on_path, "path"))

    for path, source in candidates:
        if path and os.path.isfile(path) and os.access(path, os.X_OK):
            real = os.path.realpath(path)
            size = os.path.getsize(real)
            is_debug = source == "debug" or "/target/debug/" in real
            if not is_debug and shutil.which("file"):
                try:
                    info = subprocess.check_output(["file", "-b", real], text=True, timeout=5)
                    if "not stripped" in info:
                        is_debug = True
                except Exception:
                    pass
            return {
                "status": "found",
                "path": real,
                "source": source,
                "binary_bytes": size,
                "binary_mib": round(size / (1024 * 1024), 2),
                "is_debug": is_debug,
            }
    return {
        "status": "missing",
        "path": None,
        "source": None,
        "binary_bytes": None,
        "binary_mib": None,
        "is_debug": None,
    }


def load_installed_pkg_db():
    # expac -Q '%n\t%m\t%P' gives name, installed bytes, and space-separated provides
    out = subprocess.check_output(["expac", "-Q", "%n\t%m\t%P"], text=True, timeout=15)
    sizes = {}
    provides = {}
    for line in out.splitlines():
        parts = line.split("\t")
        if len(parts) < 2:
            continue
        name = parts[0].strip()
        try:
            size = int(parts[1].strip())
        except ValueError:
            size = 0
        sizes[name] = size
        provides.setdefault(name, name)
        if len(parts) >= 3 and parts[2].strip() and parts[2].strip() != "None":
            for prov in parts[2].split():
                clean = re.split(r"[<>=]", prov, maxsplit=1)[0]
                if clean:
                    provides.setdefault(clean, name)
    return sizes, provides


def load_sync_pkg_db():
    out = subprocess.check_output(["expac", "-S", "%n\t%m\t%P"], text=True, timeout=20)
    sizes = {}
    provides = {}
    for line in out.splitlines():
        parts = line.split("\t")
        if len(parts) < 2:
            continue
        name = parts[0].strip()
        try:
            size = int(parts[1].strip())
        except ValueError:
            size = 0
        # Prefer first repo match if multiple repos carry same package
        if name not in sizes:
            sizes[name] = size
        provides.setdefault(name, name)
        if len(parts) >= 3 and parts[2].strip() and parts[2].strip() != "None":
            for prov in parts[2].split():
                clean = re.split(r"[<>=]", prov, maxsplit=1)[0]
                if clean:
                    provides.setdefault(clean, name)
    return sizes, provides


def resolve_closure(pkgs, sizes, provides, sync=False):
    raw_tokens = set()
    missing_roots = []
    cmd_flag = "-su" if sync else "-u"
    for pkg in pkgs:
        if not sync and pkg not in sizes and pkg not in provides:
            missing_roots.append(pkg)
            continue
        res = subprocess.run(
            ["pactree", cmd_flag, pkg],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=15,
        )
        if res.returncode != 0:
            missing_roots.append(pkg)
            continue
        for line in res.stdout.splitlines():
            tok = re.split(r"[<>=]", line.strip(), maxsplit=1)[0]
            if tok:
                raw_tokens.add(tok)

    resolved = set()
    unresolved = []
    for tok in sorted(raw_tokens):
        if tok in sizes:
            resolved.add(tok)
        elif tok in provides and provides[tok] in sizes:
            resolved.add(provides[tok])
        elif not sync:
            q = subprocess.run(
                ["pacman", "-Qq", tok],
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                timeout=5,
            )
            if q.returncode == 0 and q.stdout.strip():
                pkg_name = q.stdout.strip().splitlines()[0]
                if pkg_name in sizes:
                    resolved.add(pkg_name)
                    continue
            unresolved.append(tok)
        else:
            unresolved.append(tok)

    total_bytes = sum(sizes.get(p, 0) for p in resolved)
    return resolved, total_bytes, missing_roots, unresolved


def measure_footprint(gall_info):
    missing_tools = [t for t in ("pactree", "expac", "pacman") if not shutil.which(t)]
    if missing_tools:
        return {
            "status": "skipped",
            "reason": f"missing Arch package tools: {', '.join(missing_tools)}",
        }

    sizes, provides = load_installed_pkg_db()

    naut_direct = {p: sizes[p] for p in NAUTILUS_PKGS if p in sizes}
    naut_closure, naut_bytes, naut_missing, naut_unresolved = resolve_closure(
        NAUTILUS_PKGS, sizes, provides, sync=False
    )

    gall_closure, gall_dep_bytes, gall_missing, gall_unresolved = resolve_closure(
        GALL_RUNTIME_DEPS, sizes, provides, sync=False
    )

    gall_installed_pkg_bytes = sizes.get("gall")

    result = {
        "status": "ok",
        "nautilus": {
            "packages": NAUTILUS_PKGS,
            "missing_packages": naut_missing,
            "direct_packages": len(naut_direct),
            "direct_bytes": sum(naut_direct.values()),
            "direct_mib": round(sum(naut_direct.values()) / (1024 * 1024), 2),
            "closure_packages": len(naut_closure),
            "closure_bytes": naut_bytes,
            "closure_mib": round(naut_bytes / (1024 * 1024), 2),
            "unresolved_tokens": naut_unresolved,
        },
        "gall": {
            "runtime_deps": GALL_RUNTIME_DEPS,
            "missing_deps": gall_missing,
            "closure_packages": len(gall_closure),
            "closure_bytes": gall_dep_bytes,
            "closure_mib": round(gall_dep_bytes / (1024 * 1024), 2),
            "unresolved_tokens": gall_unresolved,
            "installed_package_bytes": gall_installed_pkg_bytes,
            "binary_bytes": gall_info["binary_bytes"],
            "binary_mib": gall_info["binary_mib"],
            "binary_is_debug": gall_info["is_debug"],
        },
    }

    if os.path.isfile(OMARCHY_BASE_PKG_FILE):
        with open(OMARCHY_BASE_PKG_FILE, "r", encoding="utf-8") as f:
            base_pkgs = [
                line.split("#", 1)[0].strip()
                for line in f
                if line.split("#", 1)[0].strip()
            ]
        base_without_naut = [p for p in base_pkgs if p not in NAUTILUS_PKGS]
        base_closure, base_bytes, base_missing, _ = resolve_closure(
            base_without_naut, sizes, provides, sync=False
        )

        naut_delta_pkgs = sorted(naut_closure - base_closure)
        naut_delta_bytes = sum(sizes.get(p, 0) for p in naut_delta_pkgs)

        gall_delta_pkgs = sorted(gall_closure - base_closure)
        gall_delta_bytes = sum(sizes.get(p, 0) for p in gall_delta_pkgs)

        gall_bin_or_pkg = gall_installed_pkg_bytes if gall_installed_pkg_bytes is not None else (gall_info["binary_bytes"] or 0)
        gall_net_bytes = gall_delta_bytes + gall_bin_or_pkg

        # Optional gst-plugins-good delta via sync DB
        opt_delta = None
        try:
            sync_sizes, sync_provides = load_sync_pkg_db()
            opt_closure, _, _, _ = resolve_closure(
                GALL_OPTIONAL_DEPS, sync_sizes, sync_provides, sync=True
            )
            opt_excl = sorted(opt_closure - base_closure)
            opt_bytes = sum(sync_sizes.get(p, sizes.get(p, 0)) for p in opt_excl)
            opt_delta = {
                "packages": GALL_OPTIONAL_DEPS,
                "exclusive_packages": len(opt_excl),
                "exclusive_bytes": opt_bytes,
                "exclusive_mib": round(opt_bytes / (1024 * 1024), 2),
                "package_names": opt_excl,
            }
        except Exception:
            opt_delta = None

        result["omarchy_iso_delta"] = {
            "base_file": OMARCHY_BASE_PKG_FILE,
            "base_packages_count": len(base_pkgs),
            "base_without_nautilus_count": len(base_without_naut),
            "base_closure_packages": len(base_closure),
            "base_closure_bytes": base_bytes,
            "base_missing_packages": base_missing,
            "nautilus_exclusive_packages": len(naut_delta_pkgs),
            "nautilus_exclusive_bytes": naut_delta_bytes,
            "nautilus_exclusive_mib": round(naut_delta_bytes / (1024 * 1024), 2),
            "nautilus_exclusive_names": naut_delta_pkgs,
            "gall_deps_exclusive_packages": len(gall_delta_pkgs),
            "gall_deps_exclusive_bytes": gall_delta_bytes,
            "gall_deps_exclusive_mib": round(gall_delta_bytes / (1024 * 1024), 2),
            "gall_deps_exclusive_names": gall_delta_pkgs,
            "gall_total_net_bytes": gall_net_bytes,
            "gall_total_net_mib": round(gall_net_bytes / (1024 * 1024), 2),
            "gall_optional_gst_plugins_good": opt_delta,
        }
    else:
        result["omarchy_iso_delta"] = {
            "status": "skipped",
            "reason": f"{OMARCHY_BASE_PKG_FILE} not present",
        }

    return result


def detect_display_environment():
    if skip_gui:
        return None, "GUI/RAM/startup phase skipped (--footprint-only / --skip-gui)"

    if not shutil.which("dbus-run-session"):
        return None, "dbus-run-session not installed"

    # Prefer active desktop socket if reachable; else check headless compositors.
    xdg_runtime = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
    wayland_disp = os.environ.get("WAYLAND_DISPLAY", "")
    hypr_sig = os.environ.get("HYPRLAND_INSTANCE_SIGNATURE", "")

    if wayland_disp:
        sock_path = (
            wayland_disp
            if os.path.isabs(wayland_disp)
            else os.path.join(xdg_runtime, wayland_disp)
        )
        if os.path.exists(sock_path):
            if hypr_sig and shutil.which("hyprctl"):
                res = subprocess.run(
                    ["hyprctl", "clients", "-j"],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=3,
                )
                if res.returncode == 0:
                    return {"kind": "hyprland", "wayland_display": wayland_disp}, None
            else:
                return {"kind": "wayland", "wayland_display": wayland_disp}, None

    x_disp = os.environ.get("DISPLAY", "")
    if x_disp:
        num = x_disp.split(":", 1)[-1].split(".", 1)[0]
        if os.path.exists(f"/tmp/.X11-unix/X{num}"):
            return {"kind": "x11", "display": x_disp}, None
        if shutil.which("xdpyinfo"):
            res = subprocess.run(
                ["xdpyinfo"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=3,
            )
            if res.returncode == 0:
                return {"kind": "x11", "display": x_disp}, None

    for comp in ("kwin_wayland", "labwc", "weston", "sway", "Xvfb"):
        if shutil.which(comp):
            return {"kind": f"headless:{comp}"}, None

    return (
        None,
        "no reachable Wayland/X11 display socket or headless compositor "
        "(kwin_wayland, labwc, weston, sway, Xvfb)",
    )


def find_pids_by_env(needle_key, needle_val):
    needle = f"{needle_key}={needle_val}"
    matched = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        pid = int(entry)
        if pid == os.getpid():
            continue
        try:
            with open(f"/proc/{pid}/environ", "rb") as f:
                items = f.read().decode("utf-8", errors="ignore").split("\0")
        except OSError:
            continue
        if needle in items:
            matched.append(pid)
    return matched


def pid_comm(pid):
    try:
        with open(f"/proc/{pid}/comm", "r", encoding="utf-8", errors="ignore") as f:
            return f.read().strip()
    except OSError:
        return ""


def pid_cmdline(pid):
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as f:
            return f.read().replace(b"\0", b" ").decode("utf-8", errors="ignore").strip()
    except OSError:
        return ""


def read_smaps_rollup_kb(pid):
    try:
        with open(f"/proc/{pid}/smaps_rollup", "r", encoding="utf-8", errors="ignore") as f:
            text = f.read()
    except OSError:
        return None
    rss = pss = None
    for line in text.splitlines():
        if line.startswith("Rss:"):
            rss = int(line.split()[1])
        elif line.startswith("Pss:"):
            pss = int(line.split()[1])
    if rss is None and pss is None:
        return None
    return (rss or 0, pss or 0)


def collect_session_memory(bus_addr, app_kind):
    pids = find_pids_by_env("DBUS_SESSION_BUS_ADDRESS", bus_addr)
    app_rss_kb = 0
    app_pss_kb = 0
    daemons_rss_kb = 0
    daemons_pss_kb = 0
    details = []

    for pid in sorted(pids):
        comm = pid_comm(pid)
        cmd = pid_cmdline(pid)
        if not comm or comm in ("dbus-daemon", "dbus-run-sessio", "sh", "sleep"):
            continue
        mem = read_smaps_rollup_kb(pid)
        if mem is None:
            continue
        rss_kb, pss_kb = mem
        is_app = False
        if app_kind == "gall":
            if "gall" in comm or "WebKit" in comm or "WebKit" in cmd:
                is_app = True
        elif app_kind == "nautilus":
            if (
                "nautilus" in comm
                or "nautilus" in cmd
                or "localsearch" in comm
                or "localsearch" in cmd
                or "NautilusPreviewer" in cmd
                or comm in ("gjs", "gjs-console", "ffmpegthumbnailer")
                or "glycin" in comm
                or "glycin" in cmd
            ):
                is_app = True

        if is_app:
            app_rss_kb += rss_kb
            app_pss_kb += pss_kb
            role = "app"
        else:
            daemons_rss_kb += rss_kb
            daemons_pss_kb += pss_kb
            role = "session_helper"

        details.append(
            {
                "pid": pid,
                "comm": comm,
                "role": role,
                "rss_mib": round(rss_kb / 1024, 2),
                "pss_mib": round(pss_kb / 1024, 2),
            }
        )

    return {
        "app_rss_mib": round(app_rss_kb / 1024, 2),
        "app_pss_mib": round(app_pss_kb / 1024, 2),
        "session_helpers_rss_mib": round(daemons_rss_kb / 1024, 2),
        "session_helpers_pss_mib": round(daemons_pss_kb / 1024, 2),
        "total_rss_mib": round((app_rss_kb + daemons_rss_kb) / 1024, 2),
        "total_pss_mib": round((app_pss_kb + daemons_pss_kb) / 1024, 2),
        "processes": details,
    }


def cleanup_bus_addr(bus_addr):
    if not bus_addr:
        return
    for pid in find_pids_by_env("DBUS_SESSION_BUS_ADDRESS", bus_addr):
        try:
            os.kill(pid, signal.SIGTERM)
        except OSError:
            pass
    time.sleep(0.2)
    for pid in find_pids_by_env("DBUS_SESSION_BUS_ADDRESS", bus_addr):
        try:
            os.kill(pid, signal.SIGKILL)
        except OSError:
            pass


def hyprland_windows_for_token(token):
    try:
        out = subprocess.check_output(["hyprctl", "clients", "-j"], text=True, timeout=2)
        clients = json.loads(out)
    except Exception:
        return None
    if not isinstance(clients, list):
        return None
    matched = []
    for c in clients:
        pid = c.get("pid")
        if not isinstance(pid, int):
            continue
        try:
            with open(f"/proc/{pid}/environ", "rb") as f:
                entries = f.read().decode("utf-8", errors="ignore").split("\0")
            if f"GALL_BENCH_TOKEN={token}" in entries:
                matched.append(c)
        except OSError:
            continue
    return matched


def hyprland_window_for_token(token):
    matches = hyprland_windows_for_token(token)
    return matches[0] if matches else None


def hyprland_window_addresses_for_token(token):
    windows = hyprland_windows_for_token(token)
    if windows is None:
        return None
    return {
        c["address"]
        for c in windows
        if isinstance(c.get("address"), str) and c["address"]
    }


def run_gui_trial(app_kind, app_args, disp_info, run_dir, measure_mem=False, timeout_s=25.0):
    os.makedirs(run_dir, exist_ok=True)
    xdg_cfg = os.path.join(run_dir, "config")
    xdg_data = os.path.join(run_dir, "data")
    xdg_cache = os.path.join(run_dir, "cache")
    os.makedirs(xdg_cfg, exist_ok=True)
    os.makedirs(xdg_data, exist_ok=True)
    os.makedirs(xdg_cache, exist_ok=True)

    fonts_dir = os.path.expanduser("~/.local/share/fonts")
    if os.path.isdir(fonts_dir) and not os.path.exists(os.path.join(xdg_data, "fonts")):
        try:
            os.symlink(fonts_dir, os.path.join(xdg_data, "fonts"))
        except OSError:
            pass

    token = f"gall-bench-{uuid.uuid4().hex[:12]}"
    bus_file = os.path.join(run_dir, "bus_addr")
    app_log_out = os.path.join(run_dir, "stdout.log")
    gall_internal_log = os.path.join(xdg_data, "org.gall.app", "logs", "Gall.log")

    env = os.environ.copy()
    env["XDG_CONFIG_HOME"] = xdg_cfg
    env["XDG_DATA_HOME"] = xdg_data
    env["XDG_CACHE_HOME"] = xdg_cache
    env["GALL_MULTI_INSTANCE"] = "1"
    env["GALL_BENCH_TOKEN"] = token

    wrapper = os.path.join(run_dir, "launch.sh")
    with open(wrapper, "w", encoding="utf-8") as f:
        f.write(
            '#!/bin/sh\n'
            'printf "%s\\n" "$DBUS_SESSION_BUS_ADDRESS" > "$1"\n'
            'shift\n'
            'exec "$@"\n'
        )
    os.chmod(wrapper, 0o755)

    cmd = ["dbus-run-session", "--", wrapper, bus_file] + app_args
    bus_addr = None
    proc = None
    try:
        t0 = time.monotonic_ns()
        with open(app_log_out, "w", encoding="utf-8") as logf:
            proc = subprocess.Popen(
                cmd,
                env=env,
                stdout=logf,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )

        mapped_ns = None
        gall_shown_ms = None
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            if bus_addr is None and os.path.isfile(bus_file):
                try:
                    val = open(bus_file, "r", encoding="utf-8").read().strip()
                    if val:
                        bus_addr = val
                except OSError:
                    pass

            if disp_info["kind"] == "hyprland":
                if hyprland_window_for_token(token) is not None:
                    mapped_ns = time.monotonic_ns()
                    break
            else:
                # Fallback readiness for non-Hyprland sessions
                if app_kind == "gall" and os.path.isfile(gall_internal_log):
                    try:
                        txt = open(gall_internal_log, "r", encoding="utf-8", errors="ignore").read()
                        if re.search(r"startup: window (?:'[^']*' )?shown at", txt):
                            mapped_ns = time.monotonic_ns()
                            break
                    except OSError:
                        pass
                elif app_kind == "nautilus" and bus_addr:
                    pids = find_pids_by_env("DBUS_SESSION_BUS_ADDRESS", bus_addr)
                    if any("nautilus" in pid_comm(p) for p in pids) and (time.monotonic_ns() - t0) > 1_000_000_000:
                        mapped_ns = time.monotonic_ns()
                        break

            if proc.poll() is not None:
                break
            time.sleep(0.025)

        if app_kind == "gall" and os.path.isfile(gall_internal_log):
            try:
                txt = open(gall_internal_log, "r", encoding="utf-8", errors="ignore").read()
                m = re.search(r"startup: window (?:'[^']*' )?shown at[^0-9]*([0-9]+)", txt)
                if m:
                    gall_shown_ms = int(m.group(1))
            except OSError:
                pass

        mem = None
        if mapped_ns is not None and measure_mem and bus_addr:
            time.sleep(1.5)
            mem = collect_session_memory(bus_addr, app_kind)

        wall_ms = round((mapped_ns - t0) / 1_000_000, 1) if mapped_ns is not None else None
        return {
            "mapped": mapped_ns is not None,
            "wall_ms": wall_ms,
            "gall_internal_window_shown_ms": gall_shown_ms,
            "memory": mem,
            "exit_code": proc.poll(),
        }
    finally:
        if proc is not None and proc.poll() is None:
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except OSError:
                pass
        if bus_addr is None and os.path.isfile(bus_file):
            try:
                bus_addr = open(bus_file, "r", encoding="utf-8").read().strip() or None
            except OSError:
                pass
        cleanup_bus_addr(bus_addr)


def benchmark_gui_app(app_kind, app_args, disp_info):
    wall_samples = []
    internal_samples = []
    mem_sample = None

    for idx in range(0, runs + 1):
        trial_dir = os.path.join(tmp_root, f"{app_kind}-run-{idx}")
        res = run_gui_trial(
            app_kind,
            app_args,
            disp_info,
            trial_dir,
            measure_mem=(idx == runs),
        )
        if not res["mapped"]:
            return {
                "status": "failed",
                "reason": f"{app_kind} window did not map on run {idx} (exit_code={res['exit_code']})",
            }
        if idx == 0:
            # Discard cold cache warmup
            continue
        wall_samples.append(res["wall_ms"])
        if res["gall_internal_window_shown_ms"] is not None:
            internal_samples.append(res["gall_internal_window_shown_ms"])
        if res["memory"] is not None:
            mem_sample = res["memory"]

    out = {
        "status": "ok",
        "runs": runs,
        "wall_ms_samples": wall_samples,
        "wall_ms_median": round(statistics.median(wall_samples), 1),
        "wall_ms_min": min(wall_samples),
        "wall_ms_max": max(wall_samples),
        "memory": mem_sample,
    }
    if internal_samples:
        out["gall_internal_shown_ms_median"] = round(statistics.median(internal_samples), 1)
    return out


def probe_daemon_warm_start(gall_path, disp_info):
    daemon_dir = os.path.join(tmp_root, "gall-daemon-probe")
    os.makedirs(daemon_dir, exist_ok=True)
    for sub in ("config", "data", "cache"):
        os.makedirs(os.path.join(daemon_dir, sub), exist_ok=True)

    token = f"gall-daemon-{uuid.uuid4().hex[:12]}"
    bus_file = os.path.join(daemon_dir, "bus_addr")
    wrapper = os.path.join(daemon_dir, "daemon.sh")
    with open(wrapper, "w", encoding="utf-8") as f:
        f.write(
            '#!/bin/sh\n'
            'printf "%s\\n" "$DBUS_SESSION_BUS_ADDRESS" > "$1"\n'
            'shift\n'
            'exec "$@"\n'
        )
    os.chmod(wrapper, 0o755)

    env = os.environ.copy()
    env.pop("GALL_MULTI_INSTANCE", None)
    env["XDG_CONFIG_HOME"] = os.path.join(daemon_dir, "config")
    env["XDG_DATA_HOME"] = os.path.join(daemon_dir, "data")
    env["XDG_CACHE_HOME"] = os.path.join(daemon_dir, "cache")
    env["GALL_BENCH_TOKEN"] = token

    proc = None
    bus_addr = None
    try:
        proc = subprocess.Popen(
            ["dbus-run-session", "--", wrapper, bus_file, gall_path, "--daemon"],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        time.sleep(1.5)
        if os.path.isfile(bus_file):
            bus_addr = open(bus_file, "r", encoding="utf-8").read().strip() or None

        if proc.poll() is not None:
            return {
                "status": "unsupported",
                "reason": f"gall --daemon exited early (code={proc.poll()})",
            }
        if disp_info["kind"] == "hyprland" and hyprland_window_for_token(token) is not None:
            return {
                "status": "unsupported",
                "reason": "gall --daemon mapped a window immediately (--daemon not yet implemented in this build)",
            }

        idle_mem = collect_session_memory(bus_addr, "gall") if bus_addr else None

        if not bus_addr:
            return {
                "status": "failed",
                "reason": "daemon D-Bus session bus address was not captured",
                "idle_memory": idle_mem,
                "warm_open_ms_samples": [],
                "warm_open_ms_median": None,
            }

        if disp_info["kind"] != "hyprland":
            return {
                "status": "partial",
                "reason": f"warm --new-window window-map timing requires Hyprland (display={disp_info['kind']})",
                "idle_memory": idle_mem,
                "warm_open_ms_samples": [],
                "warm_open_ms_median": None,
            }

        # Measure warm window open via --new-window on the same isolated bus
        warm_runs = min(3, runs)
        warm_samples = []
        known_addrs = set()
        warm_env = env.copy()
        warm_env["DBUS_SESSION_BUS_ADDRESS"] = bus_addr

        for i in range(warm_runs):
            before_addrs = None
            for _ in range(10):
                if proc.poll() is not None:
                    return {
                        "status": "failed",
                        "reason": f"gall --daemon exited mid-probe (code={proc.returncode})",
                        "idle_memory": idle_mem,
                        "warm_open_ms_samples": warm_samples,
                        "warm_open_ms_median": round(statistics.median(warm_samples), 1) if warm_samples else None,
                    }
                before_addrs = hyprland_window_addresses_for_token(token)
                if before_addrs is not None:
                    break
                time.sleep(0.05)

            if before_addrs is None:
                return {
                    "status": "failed",
                    "reason": f"hyprctl clients -j failed before warm-start trial {i}",
                    "idle_memory": idle_mem,
                    "warm_open_ms_samples": warm_samples,
                    "warm_open_ms_median": round(statistics.median(warm_samples), 1) if warm_samples else None,
                }
            known_addrs |= before_addrs

            t0 = time.monotonic_ns()
            client = None
            try:
                client = subprocess.Popen(
                    [gall_path, "--new-window", target_dir],
                    env=warm_env,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )
                deadline = time.monotonic() + 10.0
                mapped_ns = None
                while time.monotonic() < deadline:
                    if proc.poll() is not None:
                        return {
                            "status": "failed",
                            "reason": f"gall --daemon exited mid-probe (code={proc.returncode})",
                            "idle_memory": idle_mem,
                            "warm_open_ms_samples": warm_samples,
                            "warm_open_ms_median": round(statistics.median(warm_samples), 1) if warm_samples else None,
                        }
                    client_rc = client.poll()
                    if client_rc not in (None, 0):
                        return {
                            "status": "failed",
                            "reason": f"gall --new-window exited with code {client_rc}",
                            "idle_memory": idle_mem,
                            "warm_open_ms_samples": warm_samples,
                            "warm_open_ms_median": round(statistics.median(warm_samples), 1) if warm_samples else None,
                        }
                    current_addrs = hyprland_window_addresses_for_token(token)
                    if current_addrs is not None and len(current_addrs - known_addrs) > 0:
                        mapped_ns = time.monotonic_ns()
                        known_addrs |= current_addrs
                        break
                    time.sleep(0.02)
            finally:
                if client is not None:
                    try:
                        client.wait(timeout=0.5)
                    except subprocess.TimeoutExpired:
                        try:
                            client.terminate()
                            client.wait(timeout=1)
                        except Exception:
                            try:
                                client.kill()
                                client.wait(timeout=1)
                            except Exception:
                                pass

            if mapped_ns is None:
                return {
                    "status": "failed",
                    "reason": f"gall --new-window did not map a window within 10s on warm-start trial {i}",
                    "idle_memory": idle_mem,
                    "warm_open_ms_samples": warm_samples,
                    "warm_open_ms_median": round(statistics.median(warm_samples), 1) if warm_samples else None,
                }
            warm_samples.append(round((mapped_ns - t0) / 1_000_000, 1))

        if len(warm_samples) != warm_runs:
            return {
                "status": "failed",
                "reason": f"expected {warm_runs} warm-start samples, got {len(warm_samples)}",
                "idle_memory": idle_mem,
                "warm_open_ms_samples": warm_samples,
                "warm_open_ms_median": round(statistics.median(warm_samples), 1) if warm_samples else None,
            }

        return {
            "status": "ok",
            "idle_memory": idle_mem,
            "warm_open_ms_samples": warm_samples,
            "warm_open_ms_median": round(statistics.median(warm_samples), 1),
        }
    finally:
        if proc is not None and proc.poll() is None:
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except OSError:
                pass
        cleanup_bus_addr(bus_addr)
        if proc is not None:
            try:
                proc.wait(timeout=2)
            except Exception:
                pass


gall_info = discover_gall_binary()
footprint = measure_footprint(gall_info)
disp_info, disp_skip_reason = detect_display_environment()

if disp_info is None:
    ram_section = {"status": "skipped", "reason": disp_skip_reason}
    cold_start_section = {"status": "skipped", "reason": disp_skip_reason}
    warm_start_section = {"status": "skipped", "reason": disp_skip_reason}
elif disp_info["kind"].startswith("headless:"):
    reason = (
        f"headless compositor ({disp_info['kind'].split(':', 1)[1]}) detected; "
        "run under live Hyprland/Wayland/X11 or wrap with gui-session.sh for full window-map timings"
    )
    ram_section = {"status": "skipped", "reason": reason}
    cold_start_section = {"status": "skipped", "reason": reason}
    warm_start_section = {"status": "skipped", "reason": reason}
else:
    gall_gui = (
        benchmark_gui_app("gall", [gall_info["path"], target_dir], disp_info)
        if gall_info["status"] == "found"
        else {"status": "skipped", "reason": "gall binary not found"}
    )
    naut_gui = (
        benchmark_gui_app("nautilus", ["nautilus", "--new-window", target_dir], disp_info)
        if shutil.which("nautilus")
        else {"status": "skipped", "reason": "nautilus not installed"}
    )
    ram_section = {
        "status": "ok",
        "gall": gall_gui.get("memory") if gall_gui.get("status") == "ok" else gall_gui,
        "nautilus": naut_gui.get("memory") if naut_gui.get("status") == "ok" else naut_gui,
    }
    cold_start_section = {
        "status": "ok",
        "gall": {k: v for k, v in gall_gui.items() if k != "memory"},
        "nautilus": {k: v for k, v in naut_gui.items() if k != "memory"},
    }
    if gall_info["status"] == "found":
        warm_start_section = probe_daemon_warm_start(gall_info["path"], disp_info)
    else:
        warm_start_section = {"status": "skipped", "reason": "gall binary not found"}

report = {
    "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    "host": os.uname().nodename,
    "target_dir": target_dir,
    "gall_binary": gall_info,
    "footprint": footprint,
    "ram": ram_section,
    "cold_start": cold_start_section,
    "daemon_warm_start": warm_start_section,
}

if out_format == "json":
    print(json.dumps(report, indent=2))
elif out_format == "tsv":
    rows = ["metric\tgall\tnautilus\tstatus\tnotes"]
    if footprint.get("status") == "ok":
        n = footprint["nautilus"]
        g = footprint["gall"]
        rows.append(
            f"closure_packages\t{g['closure_packages']}\t{n['closure_packages']}\tok\tGall deps vs Nautilus stack"
        )
        rows.append(
            f"closure_bytes\t{g['closure_bytes']}\t{n['closure_bytes']}\tok\tTransitive closure"
        )
        d = footprint.get("omarchy_iso_delta", {})
        if "nautilus_exclusive_bytes" in d:
            rows.append(
                f"iso_delta_exclusive_packages\t{d['gall_deps_exclusive_packages']}\t{d['nautilus_exclusive_packages']}\tok\tAgainst omarchy-base.packages"
            )
            rows.append(
                f"iso_delta_exclusive_bytes\t{d['gall_total_net_bytes']}\t{d['nautilus_exclusive_bytes']}\tok\tGall includes binary size if built"
            )
    else:
        rows.append(f"footprint\t\t\t{footprint.get('status')}\t{footprint.get('reason','')}")

    if ram_section.get("status") == "ok":
        gm = ram_section.get("gall") or {}
        nm = ram_section.get("nautilus") or {}
        rows.append(f"ram_app_pss_mib\t{gm.get('app_pss_mib','')}\t{nm.get('app_pss_mib','')}\tok\tMain + renderer/helpers")
        rows.append(f"ram_total_pss_mib\t{gm.get('total_pss_mib','')}\t{nm.get('total_pss_mib','')}\tok\tIncluding D-Bus session helpers")
    else:
        rows.append(f"ram\t\t\t{ram_section.get('status')}\t{ram_section.get('reason','')}")

    if cold_start_section.get("status") == "ok":
        gc = cold_start_section.get("gall") or {}
        nc = cold_start_section.get("nautilus") or {}
        rows.append(f"cold_start_median_ms\t{gc.get('wall_ms_median','')}\t{nc.get('wall_ms_median','')}\tok\tRuns={runs}")
    else:
        rows.append(f"cold_start\t\t\t{cold_start_section.get('status')}\t{cold_start_section.get('reason','')}")

    warm_med = warm_start_section.get("warm_open_ms_median")
    warm_med_str = "" if warm_med is None else str(warm_med)
    warm_reason = warm_start_section.get("reason") or ""
    rows.append(
        f"daemon_warm_start\t{warm_med_str}\t\t{warm_start_section.get('status', 'skipped')}\t{warm_reason}"
    )
    print("\n".join(rows))
else:
    lines = [
        "# Gall vs. Nautilus Comparison Benchmark",
        f"- Timestamp: {report['timestamp_utc']}",
        f"- Gall binary: {gall_info['path'] or 'not found'}"
        + (f" ({fmt_mib(gall_info['binary_bytes'])}, debug={gall_info['is_debug']})" if gall_info["path"] else ""),
        "",
        "## 1. Package & Omarchy ISO Footprint",
    ]
    if footprint.get("status") == "ok":
        n = footprint["nautilus"]
        g = footprint["gall"]
        lines.append(
            f"- Nautilus stack ({', '.join(n['packages'])}): "
            f"{n['direct_packages']} direct pkgs ({fmt_mib(n['direct_bytes'])}), "
            f"{n['closure_packages']} closure pkgs ({fmt_mib(n['closure_bytes'])})"
        )
        lines.append(
            f"- Gall runtime deps ({', '.join(g['runtime_deps'])}): "
            f"{g['closure_packages']} closure pkgs ({fmt_mib(g['closure_bytes'])})"
        )
        d = footprint.get("omarchy_iso_delta", {})
        if "nautilus_exclusive_bytes" in d:
            lines.append(
                f"- Omarchy ISO exclusive delta vs `{d['base_file']}` "
                f"({d['base_without_nautilus_count']} remaining base pkgs -> {d['base_closure_packages']} closure pkgs):"
            )
            lines.append(
                f"  - Nautilus stack: +{d['nautilus_exclusive_packages']} exclusive pkgs (+{fmt_mib(d['nautilus_exclusive_bytes'])})"
            )
            lines.append(
                f"  - Gall runtime deps: +{d['gall_deps_exclusive_packages']} exclusive pkgs (+{fmt_mib(d['gall_deps_exclusive_bytes'])})"
            )
            lines.append(
                f"  - Gall net ISO delta (deps + binary/pkg): +{fmt_mib(d['gall_total_net_bytes'])}"
            )
            opt = d.get("gall_optional_gst_plugins_good")
            if opt:
                lines.append(
                    f"  - Optional `gst-plugins-good`: +{opt['exclusive_packages']} exclusive pkgs (+{fmt_mib(opt['exclusive_bytes'])})"
                )
        else:
            lines.append(f"- Omarchy ISO delta: skipped ({d.get('reason', 'unknown')})")
    else:
        lines.append(f"- Skipped: {footprint.get('reason', 'unknown')}")

    lines.append("")
    lines.append("## 2. RAM Usage (RSS / PSS via /proc/<pid>/smaps_rollup)")
    if ram_section.get("status") == "ok":
        for label, key in (("Gall", "gall"), ("Nautilus", "nautilus")):
            m = ram_section.get(key) or {}
            if "app_pss_mib" in m:
                lines.append(
                    f"- {label}: app RSS {m['app_rss_mib']} MiB / PSS {m['app_pss_mib']} MiB "
                    f"(total incl. session helpers: RSS {m['total_rss_mib']} MiB / PSS {m['total_pss_mib']} MiB)"
                )
            else:
                lines.append(f"- {label}: {m.get('status', 'skipped')} ({m.get('reason', '')})")
    else:
        lines.append(f"- Skipped: {ram_section.get('reason', 'unknown')}")

    lines.append("")
    lines.append(f"## 3. Cold-Start Latency (median of {runs} runs after 1 warmup)")
    if cold_start_section.get("status") == "ok":
        for label, key in (("Gall", "gall"), ("Nautilus", "nautilus")):
            c = cold_start_section.get(key) or {}
            if c.get("status") == "ok":
                extra = (
                    f", internal window shown median {c['gall_internal_shown_ms_median']} ms"
                    if "gall_internal_shown_ms_median" in c
                    else ""
                )
                lines.append(
                    f"- {label}: median {c['wall_ms_median']} ms (min {c['wall_ms_min']} ms, max {c['wall_ms_max']} ms{extra})"
                )
            else:
                lines.append(f"- {label}: {c.get('status', 'skipped')} ({c.get('reason', '')})")
    else:
        lines.append(f"- Skipped: {cold_start_section.get('reason', 'unknown')}")

    lines.append("")
    lines.append("## 4. Gall `--daemon` Warm-Start Probe")
    if warm_start_section.get("status") == "ok":
        idle = warm_start_section.get("idle_memory") or {}
        lines.append(
            f"- Daemon idle PSS: {idle.get('app_pss_mib', 'n/a')} MiB; "
            f"warm `--new-window` median: {warm_start_section.get('warm_open_ms_median')} ms"
        )
    else:
        warm_reason = warm_start_section.get("reason") or "no details"
        lines.append(
            f"- Status: {warm_start_section.get('status', 'skipped')} ({warm_reason})"
        )

    print("\n".join(lines))
PY
)"

if [ -n "$OUTPUT_FILE" ]; then
  printf '%s\n' "$REPORT_OUT" > "$OUTPUT_FILE"
else
  printf '%s\n' "$REPORT_OUT"
fi
