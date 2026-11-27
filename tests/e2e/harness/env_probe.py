"""Environment and capability probing for the E2E harness."""

from __future__ import annotations

import os
import re
import shutil
import stat
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

HEADLESS_COMPOSITOR_CANDIDATES: tuple[str, ...] = (
    "kwin_wayland",
    "labwc",
    "weston",
    "sway",
    "Xvfb",
)


@dataclass
class EnvProbe:
    repo_root: Path
    binary_path: Path | None
    binary_explicit: bool
    binary_exists: bool
    binary_help_flags: set[str] = field(default_factory=set)
    binary_help_text: str = ""
    selftest_modes: set[str] = field(default_factory=set)
    benchmark_script_exists: bool = False
    headless_compositor: str | None = None
    wayland_display: str | None = None
    wayland_socket: Path | None = None
    x11_display: str | None = None
    x11_socket: Path | None = None
    gtk_init_ok: bool | None = None
    live_display: bool = False
    gui_available: bool = False
    dbus_run_session: bool = False
    dbus_daemon: bool = False
    gdbus: bool = False
    busctl: bool = False
    desktop_file_validate: bool = False
    update_desktop_database: bool = False
    xmllint: bool = False
    rsvg_convert: bool = False
    gio: bool = False
    wl_copy: bool = False
    wl_paste: bool = False
    zoxide: bool = False
    hyprctl: bool = False
    bwrap: bool = False
    bwrap_works: bool = False
    tool_paths: dict[str, str | None] = field(default_factory=dict)

    def to_dict(self) -> dict[str, Any]:
        return {
            "repo_root": str(self.repo_root),
            "binary_path": str(self.binary_path) if self.binary_path else None,
            "binary_explicit": self.binary_explicit,
            "binary_exists": self.binary_exists,
            "binary_help_flags": sorted(self.binary_help_flags),
            "selftest_modes": sorted(self.selftest_modes),
            "benchmark_script_exists": self.benchmark_script_exists,
            "headless_compositor": self.headless_compositor,
            "wayland_display": self.wayland_display,
            "wayland_socket": str(self.wayland_socket) if self.wayland_socket else None,
            "x11_display": self.x11_display,
            "x11_socket": str(self.x11_socket) if self.x11_socket else None,
            "gtk_init_ok": self.gtk_init_ok,
            "live_display": self.live_display,
            "gui_available": self.gui_available,
            "dbus_run_session": self.dbus_run_session,
            "dbus_daemon": self.dbus_daemon,
            "gdbus": self.gdbus,
            "busctl": self.busctl,
            "desktop_file_validate": self.desktop_file_validate,
            "update_desktop_database": self.update_desktop_database,
            "xmllint": self.xmllint,
            "rsvg_convert": self.rsvg_convert,
            "gio": self.gio,
            "wl_copy": self.wl_copy,
            "wl_paste": self.wl_paste,
            "zoxide": self.zoxide,
            "hyprctl": self.hyprctl,
            "bwrap": self.bwrap,
            "bwrap_works": self.bwrap_works,
        }


def _is_socket(path: Path) -> bool:
    try:
        st = path.stat()
        return stat.S_ISSOCK(st.st_mode)
    except OSError:
        return False


def _probe_wayland_socket() -> tuple[str | None, Path | None]:
    wd = os.environ.get("WAYLAND_DISPLAY", "").strip()
    if not wd:
        return None, None
    wd_path = Path(wd)
    if wd_path.is_absolute():
        return wd, wd_path if _is_socket(wd_path) else None
    runtime_dir = os.environ.get("XDG_RUNTIME_DIR", "").strip()
    if not runtime_dir:
        runtime_dir = f"/run/user/{os.getuid()}"
    candidate = Path(runtime_dir) / wd
    return wd, candidate if _is_socket(candidate) else None


def _probe_x11_socket() -> tuple[str | None, Path | None]:
    disp = os.environ.get("DISPLAY", "").strip()
    if not disp:
        return None, None
    match = re.search(r":(\d+)(?:\.\d+)?$", disp)
    if not match:
        return disp, None
    sock = Path(f"/tmp/.X11-unix/X{match.group(1)}")
    return disp, sock if _is_socket(sock) else None


def _probe_gtk_init() -> bool | None:
    code = (
        "import gi; gi.require_version('Gtk', '3.0'); "
        "from gi.repository import Gtk; "
        "raise SystemExit(0 if Gtk.init_check([])[0] else 1)"
    )
    try:
        res = subprocess.run(
            [sys.executable, "-c", code],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
            check=False,
        )
        # If gi isn't installed, ImportError exits 1; distinguish via quick import check.
        if res.returncode == 0:
            return True
        check_gi = subprocess.run(
            [sys.executable, "-c", "import gi; gi.require_version('Gtk', '3.0')"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=3,
            check=False,
        )
        if check_gi.returncode != 0:
            return None
        return False
    except (OSError, subprocess.TimeoutExpired):
        return None


def _probe_binary_help(binary_path: Path) -> tuple[set[str], str]:
    # Run with scrubbed display/bus so --help prints and exits cleanly without touching desktop.
    clean_env = {
        "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
        "LANG": "C.UTF-8",
        "LC_ALL": "C.UTF-8",
        "HOME": os.environ.get("HOME", "/tmp"),
    }
    try:
        res = subprocess.run(
            [str(binary_path), "--help"],
            env=clean_env,
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
        text = (res.stdout or "") + ("\n" + res.stderr if res.stderr else "")
        flags = set(re.findall(r"(?<![A-Za-z0-9_-])--?[A-Za-z0-9][A-Za-z0-9-]*", text))
        return flags, text.strip()
    except (OSError, subprocess.TimeoutExpired):
        return set(), ""


def _parse_selftest_modes(repo_root: Path) -> set[str]:
    selftest_sh = repo_root / "scripts" / "selftest.sh"
    if not selftest_sh.is_file():
        return set()
    modes: set[str] = set()
    try:
        content = selftest_sh.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return set()
    for line in content.splitlines():
        stripped = line.strip()
        if stripped.startswith("#") and "--" in stripped:
            for m in re.findall(r"--[a-z0-9][a-z0-9-]*", stripped):
                modes.add(m)
        elif 'if [ "${1:-}" =' in stripped or 'case "${1:-}"' in stripped or "--" in stripped:
            if "MODE=" in stripped or "${1:-}" in stripped:
                for m in re.findall(r"--[a-z0-9][a-z0-9-]*", stripped):
                    modes.add(m)
    return modes


def _probe_bwrap_works(bwrap_path: str | None) -> bool:
    if not bwrap_path:
        return False
    try:
        res = subprocess.run(
            [bwrap_path, "--ro-bind", "/", "/", "true"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=5,
            check=False,
        )
        return res.returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def probe_env(repo_root: Path, binary_arg: str | None = None) -> EnvProbe:
    repo_root = Path(repo_root).resolve()
    env_binary = os.environ.get("GALL_E2E_BINARY", "").strip() or None
    binary_explicit = bool(binary_arg or env_binary)
    raw_bin = binary_arg or env_binary
    binary_path = Path(raw_bin).expanduser().resolve() if raw_bin else (repo_root / "target" / "debug" / "gall")
    binary_exists = binary_path.is_file() and os.access(binary_path, os.X_OK)

    binary_help_flags: set[str] = set()
    binary_help_text = ""
    if binary_exists:
        binary_help_flags, binary_help_text = _probe_binary_help(binary_path)

    selftest_modes = _parse_selftest_modes(repo_root)
    bench_script = repo_root / "scripts" / "benchmark-nautilus.sh"
    benchmark_script_exists = bench_script.is_file()

    headless_compositor = next(
        (c for c in HEADLESS_COMPOSITOR_CANDIDATES if shutil.which(c)),
        None,
    )
    wayland_display, wayland_socket = _probe_wayland_socket()
    x11_display, x11_socket = _probe_x11_socket()

    has_socket = bool(wayland_socket or x11_socket)
    gtk_init_ok = _probe_gtk_init() if has_socket else False
    live_display = has_socket and (gtk_init_ok is not False)
    gui_available = bool(headless_compositor or live_display)

    tool_names = {
        "dbus_run_session": "dbus-run-session",
        "dbus_daemon": "dbus-daemon",
        "gdbus": "gdbus",
        "busctl": "busctl",
        "desktop_file_validate": "desktop-file-validate",
        "update_desktop_database": "update-desktop-database",
        "xmllint": "xmllint",
        "rsvg_convert": "rsvg-convert",
        "gio": "gio",
        "wl_copy": "wl-copy",
        "wl_paste": "wl-paste",
        "zoxide": "zoxide",
        "hyprctl": "hyprctl",
        "bwrap": "bwrap",
    }
    tool_paths = {attr: shutil.which(cmd) for attr, cmd in tool_names.items()}
    bwrap_works = _probe_bwrap_works(tool_paths["bwrap"])

    return EnvProbe(
        repo_root=repo_root,
        binary_path=binary_path,
        binary_explicit=binary_explicit,
        binary_exists=binary_exists,
        binary_help_flags=binary_help_flags,
        binary_help_text=binary_help_text,
        selftest_modes=selftest_modes,
        benchmark_script_exists=benchmark_script_exists,
        headless_compositor=headless_compositor,
        wayland_display=wayland_display,
        wayland_socket=wayland_socket,
        x11_display=x11_display,
        x11_socket=x11_socket,
        gtk_init_ok=gtk_init_ok,
        live_display=live_display,
        gui_available=gui_available,
        dbus_run_session=bool(tool_paths["dbus_run_session"]),
        dbus_daemon=bool(tool_paths["dbus_daemon"]),
        gdbus=bool(tool_paths["gdbus"]),
        busctl=bool(tool_paths["busctl"]),
        desktop_file_validate=bool(tool_paths["desktop_file_validate"]),
        update_desktop_database=bool(tool_paths["update_desktop_database"]),
        xmllint=bool(tool_paths["xmllint"]),
        rsvg_convert=bool(tool_paths["rsvg_convert"]),
        gio=bool(tool_paths["gio"]),
        wl_copy=bool(tool_paths["wl_copy"]),
        wl_paste=bool(tool_paths["wl_paste"]),
        zoxide=bool(tool_paths["zoxide"]),
        hyprctl=bool(tool_paths["hyprctl"]),
        bwrap=bool(tool_paths["bwrap"]),
        bwrap_works=bwrap_works,
        tool_paths=tool_paths,
    )
