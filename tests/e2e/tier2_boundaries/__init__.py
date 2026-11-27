"""Shared opaque-box helpers for Tier 2 boundary & negative E2E tests."""

from __future__ import annotations

import json
import os
import re
import shutil
import signal
import subprocess
import time
from pathlib import Path
from typing import Any, Sequence

from harness.assertions import read_mock_calls
from harness.sandbox import SandboxEnv
from harness.types import E2EResult, RunContext, Status


def find_gall_logs(sb: SandboxEnv) -> list[Path]:
    """Locate any Gall.log files written inside the isolated sandbox."""
    candidates: list[Path] = []
    for base in (sb.data, sb.config, sb.cache, sb.state):
        if base.is_dir():
            candidates.extend(sorted(base.rglob("Gall.log")))
    return candidates


def read_gall_logs(sb: SandboxEnv) -> str:
    """Return concatenated Gall.log contents from the sandbox."""
    parts: list[str] = []
    for path in find_gall_logs(sb):
        try:
            parts.append(path.read_text(encoding="utf-8", errors="replace"))
        except OSError:
            pass
    return "\n".join(parts)


def wait_for_log_contains(sb: SandboxEnv, needle: str, timeout: float = 8.0) -> bool:
    """Poll Gall.log inside the sandbox until needle appears."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if needle in read_gall_logs(sb):
            return True
        time.sleep(0.05)
    return needle in read_gall_logs(sb)


def find_session_files(sb: SandboxEnv) -> list[Path]:
    """Locate session.json files inside the isolated sandbox."""
    found: list[Path] = []
    for base in (sb.data, sb.config):
        if base.is_dir():
            found.extend(sorted(base.rglob("session.json")))
    return found


def write_gall_settings(sb: SandboxEnv, settings: dict[str, Any]) -> list[Path]:
    """Write settings.json to both candidate XDG config dirs inside SandboxEnv."""
    written: list[Path] = []
    for sub in ("org.gall.app", "gall"):
        cfg_dir = sb.config / sub
        cfg_dir.mkdir(parents=True, exist_ok=True)
        dest = cfg_dir / "settings.json"
        dest.write_text(json.dumps(settings, indent=2) + "\n", encoding="utf-8")
        written.append(dest)
    return written


def make_restricted_bin(sb: SandboxEnv, exclude: set[str], name: str = "restricted_bin") -> Path:
    """Create a symlinked POSIX/python/dbus bin dir omitting specific tools."""
    rbin = sb.tmp / name
    rbin.mkdir(parents=True, exist_ok=True)
    needed = (
        "bash",
        "sh",
        "python3",
        "env",
        "dirname",
        "realpath",
        "mktemp",
        "rm",
        "mkdir",
        "seq",
        "printf",
        "cat",
        "sed",
        "grep",
        "chmod",
        "touch",
        "file",
        "dbus-daemon",
        "dbus-run-session",
        "gdbus",
        "git",
    )
    for tool in needed:
        if tool in exclude:
            continue
        found = shutil.which(tool)
        if found:
            link = rbin / tool
            if not link.exists():
                os.symlink(found, link)
    return rbin


def require_cli_flags(ctx: RunContext, *flags: str) -> E2EResult | None:
    """Return PENDING_IMPL if --help explicitly lacks the expected M3 CLI flags."""
    help_flags = ctx.env_probe.binary_help_flags
    if not help_flags:
        return None
    missing = [f for f in flags if f not in help_flags]
    if missing:
        return E2EResult(
            Status.PENDING_IMPL,
            f"Gall --help does not yet advertise CLI flag(s): {', '.join(missing)}",
            {"missing_flags": missing, "advertised_flags": sorted(help_flags)},
        )
    return None


def require_selftest_mode(ctx: RunContext, mode: str) -> E2EResult | None:
    """Return PENDING_IMPL if scripts/selftest.sh does not yet register --<mode>."""
    clean = mode.lstrip("-")
    flag = f"--{clean}"
    if flag not in ctx.env_probe.selftest_modes:
        return E2EResult(
            Status.PENDING_IMPL,
            f"Selftest mode {flag} not yet registered in scripts/selftest.sh",
            {"required_mode": flag, "available_modes": sorted(ctx.env_probe.selftest_modes)},
        )
    return None


def check_m2_desktop_updated(repo_root: Path) -> tuple[bool, str]:
    """Return (ready, reason) checking whether M2 desktop/service artifacts exist."""
    desktop = repo_root / "packaging" / "linux" / "gall.desktop"
    service = repo_root / "packaging" / "linux" / "org.gall.app.FileManager1.service"
    if not desktop.is_file():
        return False, "packaging/linux/gall.desktop does not exist"
    text = desktop.read_text(encoding="utf-8", errors="replace")
    if "StartupWMClass=" not in text or "Actions=new-window" not in text:
        return False, "packaging/linux/gall.desktop is still the unedited baseline template"
    if not service.is_file():
        return False, "packaging/linux/org.gall.app.FileManager1.service not yet created"
    return True, "M2 desktop/service artifacts present"


def run_gall_selftest(
    ctx: RunContext,
    sb: SandboxEnv,
    mode: str,
    extra_args: Sequence[str | os.PathLike[str]] = (),
    env_overrides: dict[str, str | None] | None = None,
    timeout: float = 60.0,
) -> tuple[subprocess.CompletedProcess[Any], str, Path]:
    """Execute Gall under GALL_SELFTEST=<mode> inside SandboxEnv."""
    assert ctx.binary_path is not None
    clean_mode = mode.lstrip("-")
    out_dir = sb.logs / f"selftest-{clean_mode}"
    out_dir.mkdir(parents=True, exist_ok=True)
    report_path = out_dir / "report.txt"
    if report_path.exists():
        report_path.unlink()

    env: dict[str, str | None] = {
        "GALL_SELFTEST": clean_mode,
        "GALL_SELFTEST_DIR": str(out_dir),
        "GALL_SELFTEST_NOFONTS": "1",
    }
    if env_overrides:
        env.update(env_overrides)

    cmd = [ctx.binary_path, *(os.fspath(a) for a in extra_args)]
    if sb.dbus_address or ctx.env_probe.dbus_run_session:
        proc = sb.run_in_dbus(cmd, env_overrides=env, timeout=timeout)
    else:
        proc = sb.run(cmd, env_overrides=env, timeout=timeout)

    report = report_path.read_text(encoding="utf-8", errors="replace") if report_path.is_file() else ""
    return proc, report, out_dir


def check_selftest_outcome(
    proc: subprocess.CompletedProcess[Any],
    report: str,
    mode: str,
    sb: SandboxEnv | None = None,
) -> E2EResult | None:
    """Detect crashes, panics, or selftest failures; returns None if clean."""
    clean_mode = mode.lstrip("-")
    if "panicked at" in (proc.stderr or ""):
        return E2EResult(
            Status.FAIL,
            f"Gall panicked during selftest '{clean_mode}'",
            {"stderr": (proc.stderr or "")[-1000:]},
        )
    logs = read_gall_logs(sb) if sb is not None else ""
    if "panicked at" in logs:
        return E2EResult(
            Status.FAIL,
            f"Gall panicked in Gall.log during selftest '{clean_mode}'",
            {"log": logs[-1000:]},
        )
    if not report.strip():
        combined_lower = ((proc.stderr or "") + "\n" + logs).lower()
        if "unknown selftest mode" in combined_lower or "unrecognized selftest" in combined_lower:
            return E2EResult(
                Status.PENDING_IMPL,
                f"Selftest mode '{clean_mode}' not yet implemented in binary",
            )
    if proc.returncode != 0:
        return E2EResult(
            Status.FAIL,
            f"Gall selftest '{clean_mode}' exited with code {proc.returncode}",
            {"stdout": (proc.stdout or "")[-1000:], "stderr": (proc.stderr or "")[-1000:]},
        )
    if not report.strip():
        return E2EResult(
            Status.FAIL,
            f"Selftest '{clean_mode}' produced empty or missing report.txt",
        )
    if not re.search(r"^0\s+failure", report, re.MULTILINE):
        return E2EResult(
            Status.FAIL,
            f"Selftest '{clean_mode}' reported failures in report.txt",
            {"report": report[-1500:]},
        )
    return None


def wait_for_dbus_name(sb: SandboxEnv, bus_name: str, timeout: float = 8.0) -> bool:
    """Poll the sandbox D-Bus session bus until bus_name appears in ListNames."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        res = sb.run_in_dbus(
            [
                "gdbus",
                "call",
                "--session",
                "--dest",
                "org.freedesktop.DBus",
                "--object-path",
                "/org/freedesktop/DBus",
                "--method",
                "org.freedesktop.DBus.ListNames",
            ],
            timeout=5.0,
        )
        if res.returncode == 0 and f"'{bus_name}'" in (res.stdout or ""):
            return True
        time.sleep(0.1)
    return False


def gdbus_call(
    sb: SandboxEnv,
    dest: str,
    obj_path: str,
    method: str,
    *args: str,
    timeout: float = 10.0,
) -> subprocess.CompletedProcess[str]:
    """Invoke a D-Bus method on the sandbox session bus via gdbus."""
    return sb.run_in_dbus(
        [
            "gdbus",
            "call",
            "--session",
            "--dest",
            dest,
            "--object-path",
            obj_path,
            "--method",
            method,
            *args,
        ],
        timeout=timeout,
    )


def count_gall_pids(sb: SandboxEnv) -> list[int]:
    """Return PIDs of running Gall processes tagged with this sandbox's GALL_E2E_RUN_ID."""
    needle = f"GALL_E2E_RUN_ID={sb.run_id}".encode("utf-8")
    pids: list[int] = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        pid = int(entry)
        try:
            env_bytes = Path(f"/proc/{pid}/environ").read_bytes()
            comm = Path(f"/proc/{pid}/comm").read_text(encoding="utf-8", errors="ignore").strip()
        except OSError:
            continue
        if needle in env_bytes.split(b"\0") and "gall" in comm.lower():
            pids.append(pid)
    return sorted(pids)


def stop_proc(proc: Any) -> None:
    """Terminate a subprocess cleanly if it is still running."""
    if proc is None or not hasattr(proc, "poll") or proc.poll() is not None:
        return
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except OSError:
        try:
            proc.terminate()
        except OSError:
            return
    try:
        proc.wait(timeout=2.0)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except OSError:
            try:
                proc.kill()
            except OSError:
                return


def start_clipboard_owner(
    sb: SandboxEnv,
    mime_payloads: dict[str, bytes],
) -> tuple[subprocess.Popen[Any] | None, E2EResult | None]:
    """Start genuine GTK/Wayland clipboard owner via harness.mocks.start_gtk_clipboard_owner."""
    from harness.mocks import ClipboardOwnerUnavailable, start_gtk_clipboard_owner

    try:
        proc = start_gtk_clipboard_owner(sb, mime_payloads)
    except ClipboardOwnerUnavailable as exc:
        return None, E2EResult(
            Status.ENV_UNAVAILABLE,
            f"GTK3/Wayland clipboard owner unavailable: {exc}",
        )
    if proc is None or proc.poll() is not None:
        rc = getattr(proc, "returncode", None)
        stderr = ""
        if proc is not None and getattr(proc, "stderr", None):
            try:
                stderr = proc.stderr.read().decode("utf-8", errors="replace")[:500]
            except OSError:
                pass
        if rc in (2, 3):
            return None, E2EResult(
                Status.ENV_UNAVAILABLE,
                f"GTK3/Wayland clipboard owner unavailable (rc={rc})",
                {"stderr": stderr},
            )
        return None, E2EResult(
            Status.FAIL,
            f"GTK3/Wayland clipboard owner crashed unexpectedly (rc={rc})",
            {"stderr": stderr},
        )
    return proc, None


def make_transcode_clipboard_script(
    sb: SandboxEnv,
    out_path: Path,
    payload: bytes,
) -> tuple[str, Path, Path]:
    """Build an omarchy-transcode mock script that places output URI on real Wayland/GTK clipboard."""
    from harness.mocks import _GTK_CLIPBOARD_OWNER_SCRIPT

    status_path = sb.tmp / "transcode_clip_status.txt"
    pid_path = sb.tmp / "transcode_clip_pid.txt"
    ready_path = sb.tmp / "transcode_clip_ready.txt"
    err_path = sb.tmp / "transcode_clip_err.log"
    spec_path = sb.tmp / "transcode_clip_spec.json"
    helper_path = sb.tmp / "transcode_gtk_owner.py"

    for p in (status_path, pid_path, ready_path, err_path, spec_path):
        if p.exists():
            p.unlink()

    helper_path.write_text(_GTK_CLIPBOARD_OWNER_SCRIPT, encoding="utf-8")

    script = f"""#!/usr/bin/env python3
import base64, json, os, pathlib, signal, subprocess, sys, time, urllib.parse

out = pathlib.Path({str(out_path)!r})
out.write_bytes({payload!r})
uri = "file://" + urllib.parse.quote(str(out), safe="/") + "\\n"
status_file = pathlib.Path({str(status_path)!r})
pid_file = pathlib.Path({str(pid_path)!r})
ready_file = pathlib.Path({str(ready_path)!r})
err_file = pathlib.Path({str(err_path)!r})
spec_file = pathlib.Path({str(spec_path)!r})

spec_file.write_text(
    json.dumps({{"text/uri-list": base64.b64encode(uri.encode("utf-8")).decode("ascii")}}),
    encoding="utf-8",
)
with open(err_file, "wb") as ef:
    child = subprocess.Popen(
        [sys.executable, {str(helper_path)!r}, str(spec_file), str(ready_file)],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=ef,
        start_new_session=True,
    )

deadline = time.monotonic() + 5.0
while time.monotonic() < deadline:
    if ready_file.is_file() or child.poll() is not None:
        break
    time.sleep(0.05)

if ready_file.is_file() and child.poll() is None:
    pid_file.write_text(str(child.pid), encoding="utf-8")
    status_file.write_text("ok:clipboard-owner\\n", encoding="utf-8")
    raise SystemExit(0)

err_tail = ""
if err_file.is_file():
    err_tail = " ".join(err_file.read_text(encoding="utf-8", errors="replace").strip().splitlines())[-300:]

rc = child.poll()
if rc is None:
    try:
        os.killpg(child.pid, signal.SIGTERM)
    except OSError:
        try:
            child.terminate()
        except OSError:
            pass
    try:
        child.wait(timeout=1.0)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except OSError:
            try:
                child.kill()
            except OSError:
                pass
    status_file.write_text(f"error:clipboard owner timed out: {{err_tail}}\\n", encoding="utf-8")
elif rc in (2, 3):
    status_file.write_text(f"env_unavailable:clipboard owner exited rc={{rc}}: {{err_tail}}\\n", encoding="utf-8")
else:
    status_file.write_text(f"error:clipboard owner crashed rc={{rc}}: {{err_tail}}\\n", encoding="utf-8")
"""
    return script, status_path, pid_path


def cleanup_transcode_clipboard(pid_path: Path) -> None:
    """Terminate background GTK clipboard owner spawned by transcode mock if running."""
    if not pid_path.is_file():
        return
    try:
        try:
            pid = int(pid_path.read_text(encoding="utf-8").strip())
        except (OSError, ValueError):
            return
        if pid <= 0:
            return
        try:
            os.killpg(pid, signal.SIGTERM)
        except OSError:
            try:
                os.kill(pid, signal.SIGTERM)
            except OSError:
                return
        deadline = time.monotonic() + 1.0
        alive = True
        while time.monotonic() < deadline:
            try:
                os.kill(pid, 0)
            except OSError:
                alive = False
                break
            try:
                status_txt = Path(f"/proc/{pid}/status").read_text(encoding="utf-8", errors="ignore")
                if re.search(r"(?m)^State:\s*Z\b", status_txt):
                    alive = False
                    break
            except OSError:
                pass
            time.sleep(0.05)
        if alive:
            try:
                os.killpg(pid, signal.SIGKILL)
            except OSError:
                try:
                    os.kill(pid, signal.SIGKILL)
                except OSError:
                    pass
    finally:
        pid_path.unlink(missing_ok=True)


def assert_unknown_arg_not_ignored(sb: SandboxEnv, proc: subprocess.CompletedProcess[Any], flag: str) -> E2EResult | None:
    """Detect pre-M3 binary rejecting new CLI flags as unknown arguments."""
    combined = (proc.stdout or "") + "\n" + (proc.stderr or "") + "\n" + read_gall_logs(sb)
    lower = combined.lower()
    needle = flag.lower()
    if (
        f"unknown argument {needle}" in lower
        or f"unexpected argument '{needle}'" in lower
        or f"unrecognized option '{needle}'" in lower
    ):
        return E2EResult(
            Status.PENDING_IMPL,
            f"Gall binary rejected {flag} as an unknown argument (M3 CLI not yet implemented)",
        )
    return None


__all__ = [
    "assert_unknown_arg_not_ignored",
    "check_m2_desktop_updated",
    "check_selftest_outcome",
    "cleanup_transcode_clipboard",
    "count_gall_pids",
    "find_gall_logs",
    "find_session_files",
    "gdbus_call",
    "make_restricted_bin",
    "make_transcode_clipboard_script",
    "read_gall_logs",
    "read_mock_calls",
    "require_cli_flags",
    "require_selftest_mode",
    "run_gall_selftest",
    "start_clipboard_owner",
    "stop_proc",
    "wait_for_dbus_name",
    "wait_for_log_contains",
    "write_gall_settings",
]
