"""Tier 1 opaque-box E2E feature tests (F1-F11) and shared Tier 1 helpers."""

from __future__ import annotations

import json
import os
import re
import time
from pathlib import Path
from typing import Any, Sequence

from harness import E2ECase, E2EResult, RunContext, SandboxEnv, Status
from harness.mocks import ClipboardOwnerUnavailable, start_gtk_clipboard_owner
from harness.sandbox import ensure_headless_display


def gall_log_candidates(sb: SandboxEnv) -> list[Path]:
    return [
        sb.data / "org.gall.app" / "logs" / "Gall.log",
        sb.data / "gall" / "logs" / "Gall.log",
    ]


def find_gall_log(sb: SandboxEnv) -> Path | None:
    for cand in gall_log_candidates(sb):
        if cand.is_file():
            return cand
    return None


def read_gall_log(sb: SandboxEnv) -> str:
    log_path = find_gall_log(sb)
    if log_path is None:
        return ""
    return log_path.read_text(encoding="utf-8", errors="replace")


def write_gall_settings(sb: SandboxEnv, settings: dict[str, Any]) -> Path:
    cfg_dir = sb.config / "gall"
    cfg_dir.mkdir(parents=True, exist_ok=True)
    path = cfg_dir / "settings.json"
    path.write_text(json.dumps(settings, indent=2) + "\n", encoding="utf-8")
    return path


def read_gall_session(sb: SandboxEnv) -> dict[str, Any] | None:
    for cand in (
        sb.config / "gall" / "session.json",
        sb.data / "org.gall.app" / "session.json",
        sb.data / "gall" / "session.json",
    ):
        if cand.is_file():
            try:
                return json.loads(cand.read_text(encoding="utf-8"))
            except json.JSONDecodeError:
                return None
    return None


def cli_flag_missing_in_help(ctx: RunContext, flag: str) -> bool:
    if not ctx.env_probe.binary_exists:
        return True
    if not ctx.env_probe.binary_help_flags:
        return False
    return flag not in ctx.env_probe.binary_help_flags


def log_reports_unknown_arg(log_text: str, stderr: str, flag: str) -> bool:
    combined = (log_text + "\n" + stderr).lower()
    needle = flag.lower()
    return (
        f"unknown argument {needle}" in combined
        or f"unexpected argument '{needle}'" in combined
        or f"unrecognized option '{needle}'" in combined
    )


def require_selftest_mode(ctx: RunContext, mode: str) -> E2EResult | None:
    """Return PENDING_IMPL before launching Gall if scripts/selftest.sh lacks --<mode>."""
    flag = f"--{mode.lstrip('-')}"
    if flag not in ctx.env_probe.selftest_modes:
        return E2EResult(
            Status.PENDING_IMPL,
            f"Selftest mode {flag} not yet registered in scripts/selftest.sh",
            {"required_mode": flag, "available_modes": sorted(ctx.env_probe.selftest_modes)},
        )
    return None


def run_gall_selftest(
    ctx: RunContext,
    sb: SandboxEnv,
    mode: str,
    cli_args: Sequence[str | os.PathLike[str]] = (),
    env_overrides: dict[str, str | None] | None = None,
    timeout: float = 45.0,
) -> tuple[int, str, str, str]:
    """Run compiled gall binary in GALL_SELFTEST=<mode> inside SandboxEnv."""
    assert ctx.binary_path is not None
    clean_mode = mode.lstrip("-")
    out_dir = sb.tmp / f"selftest-{clean_mode}"
    out_dir.mkdir(parents=True, exist_ok=True)

    env: dict[str, str | None] = {
        "GALL_SELFTEST": clean_mode,
        "GALL_SELFTEST_DIR": str(out_dir),
        "GALL_SELFTEST_CAPTURE": "webview",
    }
    if env_overrides:
        env.update(env_overrides)

    res = sb.run_in_dbus(
        [str(ctx.binary_path), *(os.fspath(a) for a in cli_args)],
        env_overrides=env,
        timeout=timeout,
    )
    report_path = out_dir / "report.txt"
    report = (
        report_path.read_text(encoding="utf-8", errors="replace")
        if report_path.is_file()
        else ""
    )
    return res.returncode, res.stdout or "", res.stderr or "", report


def selftest_mode_pending(
    ctx: RunContext,
    mode_flag: str,
    report_text: str,
    log_text: str,
) -> bool:
    """Detect whether an M4 selftest mode is not yet wired in the binary."""
    norm_flag = f"--{mode_flag.lstrip('-')}"
    if norm_flag not in ctx.env_probe.selftest_modes:
        if not report_text.strip():
            return True
    lower = (report_text + "\n" + log_text).lower()
    if "unknown selftest mode" in lower or "unrecognized selftest" in lower:
        return True
    return False


def wait_for_file_contains(path: Path, needle: str, timeout: float = 8.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.is_file():
            txt = path.read_text(encoding="utf-8", errors="replace")
            if needle in txt:
                return True
        time.sleep(0.05)
    return False


def wait_for_gall_log_contains(
    sb: SandboxEnv,
    needle: str,
    timeout: float = 8.0,
    start_offset: int = 0,
) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for cand in gall_log_candidates(sb):
            if cand.is_file():
                txt = cand.read_text(encoding="utf-8", errors="replace")[start_offset:]
                if needle in txt:
                    return True
        time.sleep(0.05)
    return False


def count_gall_pids(sb: SandboxEnv) -> list[int]:
    """Return running Gall process PIDs belonging to this sandbox."""
    needle = f"GALL_E2E_RUN_ID={sb.run_id}".encode("utf-8")
    pids: list[int] = []
    try:
        entries = os.listdir("/proc")
    except OSError:
        return pids
    for entry in entries:
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


def wait_for_dbus_name(
    sb: SandboxEnv,
    name: str,
    timeout: float = 10.0,
) -> bool:
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
                "org.freedesktop.DBus.NameHasOwner",
                name,
            ],
            timeout=5.0,
        )
        if res.returncode == 0 and "true" in (res.stdout or "").lower():
            return True
        time.sleep(0.1)
    return False


def parse_selftest_failures(report_text: str) -> list[str]:
    failures: list[str] = []
    for line in report_text.splitlines():
        if re.match(r"^\s*FAIL\b", line):
            failures.append(line.strip())
    return failures


def get_cases() -> list[E2ECase]:
    from . import (
        f01_cli,
        f02_dbus,
        f03_clipboard,
        f04_terminal_editor,
        f05_gvfs,
        f06_packaging_updater,
        f07_omarchy_bridge,
        f08_adaptive_layout,
        f09_actions_zoxide,
        f10_vim_shortcuts,
        f11_benchmark,
    )

    all_cases: list[E2ECase] = []
    for mod in (
        f01_cli,
        f02_dbus,
        f03_clipboard,
        f04_terminal_editor,
        f05_gvfs,
        f06_packaging_updater,
        f07_omarchy_bridge,
        f08_adaptive_layout,
        f09_actions_zoxide,
        f10_vim_shortcuts,
        f11_benchmark,
    ):
        all_cases.extend(mod.get_cases())
    return all_cases


__all__ = [
    "ClipboardOwnerUnavailable",
    "cli_flag_missing_in_help",
    "count_gall_pids",
    "ensure_headless_display",
    "find_gall_log",
    "gall_log_candidates",
    "get_cases",
    "log_reports_unknown_arg",
    "parse_selftest_failures",
    "read_gall_log",
    "read_gall_session",
    "require_selftest_mode",
    "run_gall_selftest",
    "selftest_mode_pending",
    "start_gtk_clipboard_owner",
    "wait_for_dbus_name",
    "wait_for_file_contains",
    "wait_for_gall_log_contains",
    "write_gall_settings",
]
