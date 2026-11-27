"""Tier 1 E2E tests for F5: GVFS mounts (Drives/Network), Connect to server, and slow-mount guards."""

from __future__ import annotations

import time

from harness import E2ECase, E2EResult, MockRegistry, RunContext, SandboxEnv, Status, read_mock_calls

from . import (
    parse_selftest_failures,
    read_gall_log,
    require_selftest_mode,
    run_gall_selftest,
    selftest_mode_pending,
)


def _gio_sample_list(sb: SandboxEnv) -> str:
    return (
        f"Mount(0): Pixel 8 -> mtp://Google_Pixel_8/\n"
        f"  default_location=file://{sb.runtime}/gvfs/mtp:host=Pixel_8\n"
        f"Mount(1): Canon EOS -> gphoto2://Canon_EOS/\n"
        f"  default_location=file://{sb.runtime}/gvfs/gphoto2:host=Canon_EOS\n"
        f"Mount(2): media on nas -> smb://nas/media/\n"
        f"  default_location=file://{sb.runtime}/gvfs/smb-share:server=nas,share=media\n"
        f"Mount(3): dev sftp -> sftp://alice@dev/\n"
        f"  default_location=file://{sb.runtime}/gvfs/sftp:host=dev,user=alice\n"
    )


def _t1_f5_01_mtp_gphoto2_in_drives(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-gvfs-drives"):
        return pending
    with SandboxEnv("T1-F5-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.seed_gvfs_mount("mtp:host=Pixel_8", {"DCIM/photo.jpg": b"jpg"})
        mocks.seed_gvfs_mount("gphoto2:host=Canon_EOS", {"IMG_001.CR3": b"raw"})
        mocks.install_gio(mounts_output=_gio_sample_list(sb))

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-gvfs-drives", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-gvfs-drives", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-gvfs-drives not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-gvfs-drives failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "Pixel" not in report or "Canon" not in report:
            return E2EResult(Status.FAIL, "MTP/gphoto2 drives missing from Drives UI", {"report": report})
        return E2EResult(Status.PASS, "MTP and gphoto2 GVFS mounts surfaced in Drives")


def _t1_f5_02_network_schemes_in_network(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-gvfs-network"):
        return pending
    with SandboxEnv("T1-F5-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        for name in (
            "smb-share:server=nas,share=media",
            "nfs:host=filer",
            "sftp:host=dev,user=alice",
            "dav:host=cloud",
            "ftp:host=pub",
        ):
            mocks.seed_gvfs_mount(name, {"readme.txt": "ok\n"})
        mocks.install_gio(mounts_output=_gio_sample_list(sb))

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-gvfs-network", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-gvfs-network", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-gvfs-network not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-gvfs-network failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        for token in ("nas", "filer", "dev", "cloud", "pub"):
            if token not in report:
                return E2EResult(Status.FAIL, f"Network mount {token} missing from Network sidebar", {"report": report})
        return E2EResult(Status.PASS, "SMB, NFS, SFTP, DAV, and FTP mounts surfaced in Network group")


def _t1_f5_03_connect_to_server(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-gvfs-connect"):
        return pending
    with SandboxEnv("T1-F5-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_gio()

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-gvfs-connect", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-gvfs-connect", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-gvfs-connect not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-gvfs-connect failed (rc={rc})", {"stderr": err})
        if failures := parse_selftest_failures(report):
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        gio_calls = read_mock_calls(sb, "gio")
        if not any(c.get("argv", [])[:1] == ["mount"] and "sftp://" in " ".join(c.get("argv", [])) for c in gio_calls):
            return E2EResult(Status.FAIL, "Connect to server did not invoke gio mount <uri>", {"gio_calls": gio_calls})
        return E2EResult(Status.PASS, "Connect to server invoked gio mount with target URI")


def _t1_f5_04_eject_and_disconnect(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-gvfs-unmount"):
        return pending
    with SandboxEnv("T1-F5-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.seed_gvfs_mount("mtp:host=Pixel_8", {"photo.jpg": b"jpg"})
        mocks.seed_gvfs_mount("smb-share:server=nas,share=media", {"share.txt": "ok"})
        mocks.install_gio(mounts_output=_gio_sample_list(sb))

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-gvfs-unmount", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-gvfs-unmount", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-gvfs-unmount not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-gvfs-unmount failed (rc={rc})", {"stderr": err})
        if failures := parse_selftest_failures(report):
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        gio_calls = read_mock_calls(sb, "gio")
        unmount_calls = [c for c in gio_calls if "mount" in c.get("argv", []) and ("-u" in c.get("argv", []) or "-e" in c.get("argv", []))]
        if not unmount_calls:
            return E2EResult(Status.FAIL, "Eject/Disconnect did not call gio mount -u/-e", {"gio_calls": gio_calls})
        return E2EResult(Status.PASS, "Eject MTP and Disconnect Network invoked gio mount -u/-e")


def _t1_f5_05_non_blocking_slow_mount(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-gvfs-slow"):
        return pending
    with SandboxEnv("T1-F5-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        files = {f"entry_{i:03d}.txt": f"data {i}\n" for i in range(150)}
        files["big.mp4"] = b"\x00" * 1024
        mount_dir = mocks.seed_gvfs_mount("sftp:host=slow", files)
        mocks.install_gio(mounts_output=_gio_sample_list(sb))

        t0 = time.monotonic()
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-gvfs-slow", cli_args=[mount_dir])
        elapsed = time.monotonic() - t0
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-gvfs-slow", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-gvfs-slow not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-gvfs-slow failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if elapsed > 10.0:
            return E2EResult(Status.FAIL, f"Slow GVFS mount browse took {elapsed:.2f}s (>10s)")
        return E2EResult(Status.PASS, f"Browsed 151-entry GVFS mount non-blockingly in {elapsed:.2f}s")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase(
            id="T1-F5-01",
            tier=1,
            features=("F5",),
            title="MTP and gphoto2 GVFS mounts appear in Drives",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f5_01_mtp_gphoto2_in_drives,
        ),
        E2ECase(
            id="T1-F5-02",
            tier=1,
            features=("F5",),
            title="SMB/NFS/SFTP/DAV/FTP GVFS mounts appear in Network",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f5_02_network_schemes_in_network,
        ),
        E2ECase(
            id="T1-F5-03",
            tier=1,
            features=("F5",),
            title="Connect to server invokes gio mount <uri>",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f5_03_connect_to_server,
        ),
        E2ECase(
            id="T1-F5-04",
            tier=1,
            features=("F5",),
            title="Eject MTP and Disconnect Network invoke gio mount -u/-e",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f5_04_eject_and_disconnect,
        ),
        E2ECase(
            id="T1-F5-05",
            tier=1,
            features=("F5",),
            title="Browsing slow GVFS mount avoids blocking folder scan, watcher, and video probe",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f5_05_non_blocking_slow_mount,
        ),
    ]
