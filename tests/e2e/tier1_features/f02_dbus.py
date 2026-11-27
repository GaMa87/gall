"""Tier 1 E2E tests for F2: org.freedesktop.FileManager1 D-Bus service & loop guard."""

from __future__ import annotations

import time
from urllib.parse import quote

from harness import E2ECase, E2EResult, RunContext, SandboxEnv, Status

from . import (
    parse_selftest_failures,
    read_gall_log,
    require_selftest_mode,
    run_gall_selftest,
    selftest_mode_pending,
    wait_for_dbus_name,
    wait_for_gall_log_contains,
)

FM1_BUS = "org.freedesktop.FileManager1"
FM1_PATH = "/org/freedesktop/FileManager1"


def _file_uri(path: object) -> str:
    return "file://" + quote(str(path), safe="/")


def _t1_f2_01_name_and_introspect(ctx: RunContext) -> E2EResult:
    assert ctx.binary_path is not None
    with SandboxEnv("T1-F2-01", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        proc = sb.popen([str(ctx.binary_path), sb.work])
        if not wait_for_dbus_name(sb, FM1_BUS, timeout=8.0):
            if proc.poll() is not None:
                return E2EResult(Status.FAIL, f"gall exited {proc.returncode}", {"log": read_gall_log(sb)[-1000:]})
            return E2EResult(Status.PENDING_IMPL, f"{FM1_BUS} not registered on session bus yet")

        introspect = sb.run_in_dbus(
            ["gdbus", "introspect", "--session", "--dest", FM1_BUS, "--object-path", FM1_PATH],
            timeout=5.0,
        )
        if introspect.returncode != 0:
            return E2EResult(Status.FAIL, "gdbus introspect failed", {"stderr": introspect.stderr})

        xml_or_text = introspect.stdout or ""
        for method in ("ShowFolders", "ShowItems", "ShowItemProperties"):
            if method not in xml_or_text:
                return E2EResult(
                    Status.FAIL,
                    f"Missing D-Bus method {method} in introspection",
                    {"introspect": xml_or_text},
                )
        return E2EResult(Status.PASS, "FileManager1 registered and exposes ShowFolders/ShowItems/ShowItemProperties")


def _t1_f2_02_show_folders(ctx: RunContext) -> E2EResult:
    assert ctx.binary_path is not None
    with SandboxEnv("T1-F2-02", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        dir_a = sb.work / "dirA"
        dir_b = sb.work / "dirB"
        dir_a.mkdir()
        dir_b.mkdir()

        proc = sb.popen([str(ctx.binary_path), str(dir_a)])
        if not wait_for_dbus_name(sb, FM1_BUS, timeout=8.0):
            if proc.poll() is not None:
                return E2EResult(Status.FAIL, f"gall exited {proc.returncode}", {"log": read_gall_log(sb)[-1000:]})
            return E2EResult(Status.PENDING_IMPL, f"{FM1_BUS} not registered on session bus yet")

        call = sb.run_in_dbus(
            [
                "gdbus",
                "call",
                "--session",
                "--dest",
                FM1_BUS,
                "--object-path",
                FM1_PATH,
                "--method",
                f"{FM1_BUS}.ShowFolders",
                f"['{_file_uri(dir_b)}']",
                "",
            ],
            timeout=5.0,
        )
        if call.returncode != 0:
            return E2EResult(Status.FAIL, f"ShowFolders call failed: {call.stderr}")

        if not wait_for_gall_log_contains(sb, str(dir_b), timeout=5.0):
            return E2EResult(Status.FAIL, "ShowFolders did not navigate Gall to dirB", {"log": read_gall_log(sb)[-1000:]})
        return E2EResult(Status.PASS, "ShowFolders navigated Gall to dirB")


def _t1_f2_03_show_items(ctx: RunContext) -> E2EResult:
    assert ctx.binary_path is not None
    with SandboxEnv("T1-F2-03", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        dir_a = sb.work / "dirA"
        dir_b = sb.work / "dirB"
        dir_a.mkdir()
        dir_b.mkdir()
        item = dir_b / "item.txt"
        item.write_text("item\n", encoding="utf-8")

        proc = sb.popen([str(ctx.binary_path), str(dir_a)])
        if not wait_for_dbus_name(sb, FM1_BUS, timeout=8.0):
            if proc.poll() is not None:
                return E2EResult(Status.FAIL, f"gall exited {proc.returncode}", {"log": read_gall_log(sb)[-1000:]})
            return E2EResult(Status.PENDING_IMPL, f"{FM1_BUS} not registered on session bus yet")

        call = sb.run_in_dbus(
            [
                "gdbus",
                "call",
                "--session",
                "--dest",
                FM1_BUS,
                "--object-path",
                FM1_PATH,
                "--method",
                f"{FM1_BUS}.ShowItems",
                f"['{_file_uri(item)}']",
                "startup-1",
            ],
            timeout=5.0,
        )
        if call.returncode != 0:
            return E2EResult(Status.FAIL, f"ShowItems call failed: {call.stderr}")

        if not wait_for_gall_log_contains(sb, "item.txt", timeout=5.0):
            return E2EResult(Status.FAIL, "ShowItems did not open parent and select item.txt", {"log": read_gall_log(sb)[-1000:]})
        return E2EResult(Status.PASS, "ShowItems opened dirB and targeted item.txt")


def _t1_f2_04_show_item_properties(ctx: RunContext) -> E2EResult:
    assert ctx.binary_path is not None
    with SandboxEnv("T1-F2-04", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        dir_a = sb.work / "dirA"
        dir_a.mkdir()
        item = dir_a / "props.txt"
        item.write_text("properties\n", encoding="utf-8")

        proc = sb.popen([str(ctx.binary_path), str(dir_a)])
        if not wait_for_dbus_name(sb, FM1_BUS, timeout=8.0):
            if proc.poll() is not None:
                return E2EResult(Status.FAIL, f"gall exited {proc.returncode}", {"log": read_gall_log(sb)[-1000:]})
            return E2EResult(Status.PENDING_IMPL, f"{FM1_BUS} not registered on session bus yet")

        call = sb.run_in_dbus(
            [
                "gdbus",
                "call",
                "--session",
                "--dest",
                FM1_BUS,
                "--object-path",
                FM1_PATH,
                "--method",
                f"{FM1_BUS}.ShowItemProperties",
                f"['{_file_uri(item)}']",
                "",
            ],
            timeout=5.0,
        )
        if call.returncode != 0:
            return E2EResult(Status.FAIL, f"ShowItemProperties call failed: {call.stderr}")
        time.sleep(0.4)
        if proc.poll() is not None:
            return E2EResult(
                Status.FAIL,
                f"Gall exited after ShowItemProperties (rc={proc.returncode})",
                {"log": read_gall_log(sb)[-1000:]},
            )
        return E2EResult(Status.PASS, "ShowItemProperties succeeded without error")


def _t1_f2_05_reveal_path_loop_guard(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-dbus-reveal"):
        return pending
    with SandboxEnv("T1-F2-05", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        sub = sb.work / "nested"
        sub.mkdir()
        target = sub / "found.txt"
        target.write_text("find me\n", encoding="utf-8")

        mon = sb.popen(
            ["dbus-monitor", "--session", f"type='method_call',interface='{FM1_BUS}'"],
        )
        try:
            time.sleep(0.3)

            rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-dbus-reveal", cli_args=[sb.work])
            log = read_gall_log(sb)
            if selftest_mode_pending(ctx, "--e2e-dbus-reveal", report, log) and rc == 0 and not report:
                return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-dbus-reveal not yet registered")
            if rc != 0:
                return E2EResult(Status.FAIL, f"e2e-dbus-reveal failed (rc={rc})", {"stderr": err, "log": log[-1000:]})

            failures = parse_selftest_failures(report)
            if failures:
                return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        finally:
            if mon.poll() is None:
                mon.terminate()
            mon_out, _ = mon.communicate(timeout=2.0)

        mon_text = mon_out.decode("utf-8", errors="replace") if isinstance(mon_out, bytes) else str(mon_out)
        if "member=ShowItems" in mon_text:
            return E2EResult(
                Status.FAIL,
                "reveal_path emitted outbound ShowItems call while Gall owns FileManager1 (loop guard violated)",
                {"dbus_monitor": mon_text},
            )
        return E2EResult(Status.PASS, "reveal_path bypassed external D-Bus ShowItems when Gall owned FileManager1")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase(
            id="T1-F2-01",
            tier=1,
            features=("F2",),
            title="org.freedesktop.FileManager1 bus registration and method introspection",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f2_01_name_and_introspect,
        ),
        E2ECase(
            id="T1-F2-02",
            tier=1,
            features=("F2",),
            title="ShowFolders D-Bus method navigates Gall to requested folder",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f2_02_show_folders,
        ),
        E2ECase(
            id="T1-F2-03",
            tier=1,
            features=("F2",),
            title="ShowItems D-Bus method opens parent directory and selects item",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f2_03_show_items,
        ),
        E2ECase(
            id="T1-F2-04",
            tier=1,
            features=("F2",),
            title="ShowItemProperties D-Bus method handles item URI cleanly",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f2_04_show_item_properties,
        ),
        E2ECase(
            id="T1-F2-05",
            tier=1,
            features=("F2",),
            title="reveal_path loop guard avoids outbound D-Bus ShowItems when self-owned",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f2_05_reveal_path_loop_guard,
        ),
    ]
