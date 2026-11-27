"""Tier 2 boundary & negative E2E cases for Feature F5 (GVFS mounts & slow-mount guards)."""

from __future__ import annotations

import os
import shutil
import time

from harness.assertions import read_mock_calls
from harness.mocks import MockRegistry
from harness.sandbox import SandboxEnv
from harness.types import E2ECase, E2EResult, RunContext, Status

from . import (
    check_selftest_outcome,
    make_restricted_bin,
    read_gall_logs,
    require_selftest_mode,
    run_gall_selftest,
)


def _t2_f5_01_malformed_percent_gvfs_names(ctx: RunContext) -> E2EResult:
    """Malformed and percent-encoded GVFS mount directories must not crash sidebar/listing."""
    if pending := require_selftest_mode(ctx, "e2e-gvfs-network"):
        return pending
    with SandboxEnv("T2-F5-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_gio()
        mocks.seed_gvfs_mount("mtp:")
        mocks.seed_gvfs_mount("smb-share:broken")
        mocks.seed_gvfs_mount("smb-share:server=nas,share=My%20Media", {"clip.txt": "ok\n"})
        mocks.seed_gvfs_mount("unknown:foo=bar")

        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-gvfs-network")
        if fail := check_selftest_outcome(proc, report, "e2e-gvfs-network", sb=sb):
            return fail
        combined = report + "\n" + read_gall_logs(sb)
        if "My Media" not in combined and "smb-share" not in combined and "nas" not in combined:
            return E2EResult(Status.FAIL, "Percent-encoded GVFS mount My Media was not surfaced in e2e-gvfs-network report", {"report": report})
        return E2EResult(Status.PASS, "Malformed and percent-encoded GVFS mount names handled cleanly")


def _t2_f5_02_missing_gvfs_dir_and_gio_binary(ctx: RunContext) -> E2EResult:
    """Missing $XDG_RUNTIME_DIR/gvfs and absent gio binary must leave UI responsive with empty Network."""
    if pending := require_selftest_mode(ctx, "e2e-gvfs-network"):
        return pending
    with SandboxEnv("T2-F5-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        gvfs_dir = sb.runtime / "gvfs"
        if gvfs_dir.exists():
            shutil.rmtree(gvfs_dir)
        rbin = make_restricted_bin(sb, exclude={"gio"}, name="rbin_nogio")
        proc, report, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-gvfs-network",
            env_overrides={"PATH": f"{sb.mock_bin}:{rbin}"},
        )
        if fail := check_selftest_outcome(proc, report, "e2e-gvfs-network", sb=sb):
            return fail
        if "network" not in report.lower() and "gvfs" not in report.lower():
            return E2EResult(Status.FAIL, "e2e-gvfs-network report missing network/gvfs marker", {"report": report})
        return E2EResult(Status.PASS, "Missing gvfs/ directory and missing gio binary handled cleanly")


def _t2_f5_03_connect_server_error_and_empty_uri(ctx: RunContext) -> E2EResult:
    """connect_server with gio exit=1 or empty URI must show user error without crashing."""
    if pending := require_selftest_mode(ctx, "e2e-gvfs-connect"):
        return pending
    with SandboxEnv("T2-F5-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_gio(mount_exit=1)
        proc, report, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-gvfs-connect",
            env_overrides={"MOCK_GIO_MOUNT_EXIT": "1"},
        )
        if fail := check_selftest_outcome(proc, report, "e2e-gvfs-connect", sb=sb):
            return fail
        if "connect" not in report.lower():
            return E2EResult(Status.FAIL, "e2e-gvfs-connect report missing connect marker", {"report": report})
        if not read_mock_calls(sb, cmd="gio"):
            return E2EResult(Status.FAIL, "Expected gio mock invocation during gvfs connect_server error test")
        return E2EResult(Status.PASS, "connect_server failure and empty URI handled gracefully")


def _t2_f5_04_hung_gio_mount_list_non_blocking(ctx: RunContext) -> E2EResult:
    """Hung `gio mount -l` (10s sleep) must not block window startup or local browsing."""
    if pending := require_selftest_mode(ctx, "e2e-gvfs-slow"):
        return pending
    with SandboxEnv("T2-F5-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_gio(list_delay=10.0)
        local_dir = sb.work / "local_dir"
        local_dir.mkdir()
        (local_dir / "fast.txt").write_text("fast\n", encoding="utf-8")

        t0 = time.monotonic()
        proc, report, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-gvfs-slow",
            extra_args=[str(local_dir)],
            timeout=8.0,
        )
        elapsed = time.monotonic() - t0
        if fail := check_selftest_outcome(proc, report, "e2e-gvfs-slow", sb=sb):
            return fail
        if "slow" not in report.lower() and "guard" not in report.lower():
            return E2EResult(Status.FAIL, "e2e-gvfs-slow report missing slow/guard marker", {"report": report})
        if elapsed >= 8.0:
            return E2EResult(Status.FAIL, f"Local browsing blocked {elapsed:.2f}s on slow gio mount -l")
        return E2EResult(Status.PASS, f"Startup and local browse finished in {elapsed:.2f}s despite 10s gio delay")


def _t2_f5_05_unreadable_and_vanished_mount(ctx: RunContext) -> E2EResult:
    """chmod 000 subdir or vanished GVFS mount mid-browse must not panic or trigger error storm."""
    if pending := require_selftest_mode(ctx, "e2e-gvfs-slow"):
        return pending
    with SandboxEnv("T2-F5-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_gio()
        mount_dir = mocks.seed_gvfs_mount("sftp:host=nas", {"ok.txt": "1\n"})
        locked = mount_dir / "locked"
        locked.mkdir()
        os.chmod(locked, 0o000)
        sb.add_cleanup(lambda: os.chmod(locked, 0o755) if locked.exists() else None)

        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-gvfs-slow", extra_args=[str(mount_dir)])
        if fail := check_selftest_outcome(proc, report, "e2e-gvfs-slow", sb=sb):
            return fail
        if "slow" not in report.lower() and "guard" not in report.lower():
            return E2EResult(Status.FAIL, "e2e-gvfs-slow report missing slow/guard marker on chmod 000", {"report": report})

        # Also verify vanished mount directory recovers cleanly without panic
        os.chmod(locked, 0o755)
        shutil.rmtree(mount_dir)
        proc2, report2, _ = run_gall_selftest(ctx, sb, "e2e-gvfs-slow", extra_args=[str(mount_dir)])
        if fail := check_selftest_outcome(proc2, report2, "e2e-gvfs-slow", sb=sb):
            return fail
        if "slow" not in report2.lower() and "guard" not in report2.lower():
            return E2EResult(Status.FAIL, "e2e-gvfs-slow report missing slow/guard marker on vanished mount", {"report": report2})
        return E2EResult(Status.PASS, "Unreadable (000) and vanished GVFS mount handled without panic")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase("T2-F5-01", 2, ("F5",), "GVFS malformed and percent-encoded mount directory names", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f5_01_malformed_percent_gvfs_names),
        E2ECase("T2-F5-02", 2, ("F5",), "GVFS missing runtime/gvfs dir and missing gio binary", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f5_02_missing_gvfs_dir_and_gio_binary),
        E2ECase("T2-F5-03", 2, ("F5",), "GVFS connect_server error (gio exit 1) and empty URI", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f5_03_connect_server_error_and_empty_uri),
        E2ECase("T2-F5-04", 2, ("F5",), "GVFS hung gio mount -l (10s) does not block local browsing", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f5_04_hung_gio_mount_list_non_blocking),
        E2ECase("T2-F5-05", 2, ("F5",), "GVFS chmod 000 / vanished mount mid-browse resilience", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f5_05_unreadable_and_vanished_mount),
    ]
