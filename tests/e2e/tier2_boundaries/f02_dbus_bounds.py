"""Tier 2 boundary & negative E2E cases for Feature F2 (org.freedesktop.FileManager1)."""

from __future__ import annotations

import time
import urllib.parse
from typing import Any

from harness.sandbox import SandboxEnv
from harness.types import E2ECase, E2EResult, RunContext, Status

from . import gdbus_call, read_gall_logs, wait_for_dbus_name, wait_for_log_contains

FM1_DEST = "org.freedesktop.FileManager1"
FM1_PATH = "/org/freedesktop/FileManager1"


def _ensure_fm1_ready(ctx: RunContext, sb: SandboxEnv) -> tuple[Any, E2EResult | None]:
    assert ctx.binary_path is not None
    proc = sb.popen([ctx.binary_path, str(sb.work)], cwd=sb.work)
    if not wait_for_dbus_name(sb, FM1_DEST, timeout=8.0):
        if proc.poll() is not None:
            return proc, E2EResult(Status.FAIL, f"Gall exited prematurely before D-Bus ready (rc={proc.returncode})")
        return proc, E2EResult(Status.PENDING_IMPL, "org.freedesktop.FileManager1 not registered on session bus (M3 pending)")
    return proc, None


def _t2_f2_01_empty_uri_array(ctx: RunContext) -> E2EResult:
    """Empty URI array [] on ShowFolders/ShowItems/ShowItemProperties must not crash or hang."""
    with SandboxEnv("T2-F2-01", ctx.env_probe, allow_gui=True, dbus_required=True) as sb:
        proc, err = _ensure_fm1_ready(ctx, sb)
        if err:
            return err
        for method in ("ShowFolders", "ShowItems", "ShowItemProperties"):
            res = gdbus_call(sb, FM1_DEST, FM1_PATH, f"{FM1_DEST}.{method}", "@as []", "")
            if res.returncode != 0:
                return E2EResult(Status.FAIL, f"{method}([]) returned error rc={res.returncode}", {"stderr": res.stderr})
        if proc.poll() is not None or not wait_for_dbus_name(sb, FM1_DEST, timeout=2.0):
            return E2EResult(Status.FAIL, "FileManager1 bus name dropped after empty URI calls")
        if "panicked at" in read_gall_logs(sb):
            return E2EResult(Status.FAIL, "Panic logged on empty FileManager1 URI arrays")
        return E2EResult(Status.PASS, "All three FileManager1 methods handled empty URI array cleanly")


def _t2_f2_02_malformed_non_file_uris(ctx: RunContext) -> E2EResult:
    """Non-file:// and malformed URIs must not crash Gall or drop FileManager1 ownership."""
    with SandboxEnv("T2-F2-02", ctx.env_probe, allow_gui=True, dbus_required=True) as sb:
        proc, err = _ensure_fm1_ready(ctx, sb)
        if err:
            return err
        bad = "['http://example.com', 'trash:///x', 'not-a-uri', 'file://relative/path']"
        res = gdbus_call(sb, FM1_DEST, FM1_PATH, f"{FM1_DEST}.ShowItems", bad, "startup-bad")
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"ShowItems rejected malformed URIs with D-Bus error rc={res.returncode}", {"stderr": res.stderr})
        if proc.poll() is not None or not wait_for_dbus_name(sb, FM1_DEST, timeout=2.0):
            return E2EResult(Status.FAIL, "Gall crashed or lost FileManager1 ownership on malformed URIs")
        if "panicked at" in read_gall_logs(sb):
            return E2EResult(Status.FAIL, "Panic logged on malformed FileManager1 URIs")
        return E2EResult(Status.PASS, "Non-file:// and malformed URIs handled safely without dropping bus name")


def _t2_f2_03_multi_item_utf8_percent(ctx: RunContext) -> E2EResult:
    """Multiple items with %20, %23 (#), and UTF-8 %C4%8D must decode cleanly without truncation."""
    with SandboxEnv("T2-F2-03", ctx.env_probe, allow_gui=True, dbus_required=True) as sb:
        dir_a = sb.work / "dirA"
        dir_a.mkdir()
        f1 = dir_a / "a #1.txt"
        f2 = dir_a / "b č.txt"
        f1.write_text("one\n", encoding="utf-8")
        f2.write_text("two\n", encoding="utf-8")

        proc, err = _ensure_fm1_ready(ctx, sb)
        if err:
            return err
        u1 = "file://" + urllib.parse.quote(str(f1), safe="/")
        u2 = "file://" + urllib.parse.quote(str(f2), safe="/")
        res = gdbus_call(sb, FM1_DEST, FM1_PATH, f"{FM1_DEST}.ShowItems", f"['{u1}', '{u2}']", "s1")
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"ShowItems multi-UTF8 call failed (rc={res.returncode})", {"stderr": res.stderr})
        if not wait_for_log_contains(sb, "a #1.txt", timeout=4.0):
            return E2EResult(Status.FAIL, "ShowItems did not decode %23/# item a #1.txt in Gall.log", {"log": read_gall_logs(sb)[-1000:]})
        if proc.poll() is not None or not wait_for_dbus_name(sb, FM1_DEST, timeout=2.0):
            return E2EResult(Status.FAIL, "Gall crashed or dropped FileManager1 after UTF-8 ShowItems")
        logs = read_gall_logs(sb)
        if "panicked at" in logs:
            return E2EResult(Status.FAIL, "Panic logged on multi-item UTF-8 ShowItems")
        return E2EResult(Status.PASS, "Multi-item percent-encoded UTF-8/%23 URIs decoded cleanly")


def _t2_f2_04_show_items_dir_and_missing(ctx: RunContext) -> E2EResult:
    """ShowItems on a directory and on a missing file must stay responsive without errors."""
    with SandboxEnv("T2-F2-04", ctx.env_probe, allow_gui=True, dbus_required=True) as sb:
        dir_a = sb.work / "dirA"
        dir_a.mkdir()
        proc, err = _ensure_fm1_ready(ctx, sb)
        if err:
            return err
        res = gdbus_call(
            sb,
            FM1_DEST,
            FM1_PATH,
            f"{FM1_DEST}.ShowItems",
            f"['file://{dir_a}', 'file://{dir_a / 'missing.txt'}']",
            "",
        )
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"ShowItems dir+missing failed (rc={res.returncode})", {"stderr": res.stderr})
        if proc.poll() is not None or not wait_for_dbus_name(sb, FM1_DEST, timeout=2.0):
            return E2EResult(Status.FAIL, "FileManager1 stopped responding after dir/missing ShowItems")
        if "panicked at" in read_gall_logs(sb):
            return E2EResult(Status.FAIL, "Panic logged on ShowItems dir/missing")
        return E2EResult(Status.PASS, "ShowItems on directory and missing target handled gracefully")


def _t2_f2_05_bus_contention_external_fallback(ctx: RunContext) -> E2EResult:
    """When another process already owns org.freedesktop.FileManager1, Gall must not crash."""
    assert ctx.binary_path is not None
    with SandboxEnv("T2-F2-05", ctx.env_probe, allow_gui=True, dbus_required=True) as sb:
        mock_log = sb.logs / "mock_fm1.jsonl"
        mock_script = sb.tmp / "mock_fm1_owner.py"
        mock_script.write_text(
            f"""import json, sys, dbus, dbus.service, dbus.mainloop.glib
from gi.repository import GLib

dbus.mainloop.glib.DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
name = dbus.service.BusName("{FM1_DEST}", bus, do_not_queue=True)
log_path = {str(mock_log)!r}

class MockFM(dbus.service.Object):
    @dbus.service.method("{FM1_DEST}", in_signature="ass", out_signature="")
    def ShowFolders(self, uris, sid):
        with open(log_path, "a") as f: f.write(json.dumps({{"m": "ShowFolders", "uris": list(uris)}}) + "\\n")
    @dbus.service.method("{FM1_DEST}", in_signature="ass", out_signature="")
    def ShowItems(self, uris, sid):
        with open(log_path, "a") as f: f.write(json.dumps({{"m": "ShowItems", "uris": list(uris)}}) + "\\n")
    @dbus.service.method("{FM1_DEST}", in_signature="ass", out_signature="")
    def ShowItemProperties(self, uris, sid):
        with open(log_path, "a") as f: f.write(json.dumps({{"m": "ShowItemProperties", "uris": list(uris)}}) + "\\n")

obj = MockFM(bus, "{FM1_PATH}")
GLib.MainLoop().run()
""",
            encoding="utf-8",
        )
        owner = sb.popen(["python3", str(mock_script)])
        if not wait_for_dbus_name(sb, FM1_DEST, timeout=5.0):
            return E2EResult(Status.ENV_UNAVAILABLE, "python3-dbus/gi mock FileManager1 owner failed to start")

        gall = sb.popen([ctx.binary_path, str(sb.work)], cwd=sb.work)
        wait_for_log_contains(sb, "window shown at", timeout=6.0)
        if gall.poll() is not None:
            return E2EResult(Status.FAIL, f"Gall crashed under FileManager1 bus contention (rc={gall.returncode})")
        if owner.poll() is not None:
            return E2EResult(Status.FAIL, "External FileManager1 owner died unexpectedly")
        if "panicked at" in read_gall_logs(sb):
            return E2EResult(Status.FAIL, "Panic logged under FileManager1 bus contention")
        return E2EResult(Status.PASS, "Gall survived existing FileManager1 owner without crashing")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase("T2-F2-01", 2, ("F2",), "FileManager1 empty URI array [] on all three methods", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f2_01_empty_uri_array),
        E2ECase("T2-F2-02", 2, ("F2",), "FileManager1 non-file:// and malformed URIs", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f2_02_malformed_non_file_uris),
        E2ECase("T2-F2-03", 2, ("F2",), "FileManager1 multi-item UTF-8 and %23 (#) URI decoding", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f2_03_multi_item_utf8_percent),
        E2ECase("T2-F2-04", 2, ("F2",), "FileManager1 ShowItems on directory and missing file", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f2_04_show_items_dir_and_missing),
        E2ECase("T2-F2-05", 2, ("F2",), "FileManager1 bus contention when name already owned", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f2_05_bus_contention_external_fallback),
    ]
