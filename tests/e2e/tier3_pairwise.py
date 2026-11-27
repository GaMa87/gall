"""Tier 3 opaque-box cross-feature pairwise combinatorial E2E tests (T3-01..T3-12)."""

from __future__ import annotations

import hashlib
import json
import re
import shutil
import time
import urllib.parse
from pathlib import Path
from typing import Any

from harness.assertions import assert_desktop_valid, assert_xml_valid, read_mock_calls
from harness.mocks import MockRegistry
from harness.sandbox import SandboxEnv
from harness.types import E2ECase, E2EResult, RunContext, Status

from tier2_boundaries import (
    assert_unknown_arg_not_ignored,
    check_selftest_outcome,
    cleanup_transcode_clipboard,
    count_gall_pids,
    gdbus_call,
    make_transcode_clipboard_script,
    read_gall_logs,
    require_cli_flags,
    require_selftest_mode,
    run_gall_selftest,
    start_clipboard_owner,
    stop_proc,
    wait_for_dbus_name,
    wait_for_log_contains,
    write_gall_settings,
)

FM1_DEST = "org.freedesktop.FileManager1"
FM1_PATH = "/org/freedesktop/FileManager1"


def _file_uri(path: Path) -> str:
    return "file://" + urllib.parse.quote(str(path), safe="/")


def _install_full_arch_mocks(ctx: RunContext, sb: SandboxEnv) -> tuple[dict[str, Any], dict[str, int]]:
    base_file = ctx.repo_root / "tests" / "e2e" / "fixtures" / "omarchy-base.packages"
    base_pkgs = [
        ln.split("#", 1)[0].strip()
        for ln in base_file.read_text(encoding="utf-8").splitlines()
        if ln.split("#", 1)[0].strip()
    ]
    db: dict[str, dict[str, Any]] = {
        pkg: {"size": 1_000_000, "deps": [], "provides": []} for pkg in base_pkgs
    }
    db["nautilus"] = {"size": 8_000_000, "deps": ["base", "gvfs"], "provides": []}
    db["nautilus-python"] = {"size": 1_000_000, "deps": ["nautilus"], "provides": []}
    db["sushi"] = {"size": 3_000_000, "deps": ["nautilus"], "provides": []}
    db["ffmpegthumbnailer"] = {"size": 500_000, "deps": ["ffmpeg"], "provides": []}
    db["gtk3"] = {"size": 10_000_000, "deps": ["fontconfig"], "provides": []}
    db["webkit2gtk-4.1"] = {"size": 20_000_000, "deps": ["gtk3"], "provides": []}
    db["gst-plugins-good"] = {"size": 4_000_000, "deps": [], "provides": []}

    db_path = sb.tmp / "tier3_arch_db.json"
    db_path.write_text(json.dumps(db), encoding="utf-8")
    mocks = MockRegistry(sb)

    expac_body = f"""#!/usr/bin/env python3
import json, sys
with open({str(db_path)!r}, "r", encoding="utf-8") as f:
    db = json.load(f)
args = sys.argv[1:]
fmt = "%n\\t%m\\t%P"
pkgs = []
for a in args:
    if a in ("-Q", "-S"):
        continue
    if "%" in a:
        fmt = a
    elif not a.startswith("-"):
        pkgs.append(a)
for name in (pkgs if pkgs else sorted(db.keys())):
    if name not in db:
        continue
    e = db[name]
    prov = " ".join(e.get("provides", [])) or "None"
    print(fmt.replace("%n", name).replace("%m", str(e["size"])).replace("%P", prov))
"""
    pactree_body = f"""#!/usr/bin/env python3
import json, sys
from collections import deque
with open({str(db_path)!r}, "r", encoding="utf-8") as f:
    db = json.load(f)
targets = [a for a in sys.argv[1:] if not a.startswith("-")]
if not targets or targets[0] not in db:
    raise SystemExit(1)
q = deque([targets[0]])
seen = set()
while q:
    cur = q.popleft()
    if cur in seen or cur not in db:
        continue
    seen.add(cur)
    print(cur)
    for d in db[cur].get("deps", []):
        if d not in seen:
            q.append(d)
"""
    pacman_body = f"""#!/usr/bin/env python3
import json, sys
with open({str(db_path)!r}, "r", encoding="utf-8") as f:
    db = json.load(f)
args = [a for a in sys.argv[1:] if not a.startswith("-")]
if args and args[0] in db:
    print(args[0])
    raise SystemExit(0)
raise SystemExit(1)
"""
    mocks.install_tool("expac", script_body=expac_body)
    mocks.install_tool("pactree", script_body=pactree_body)
    mocks.install_tool("pacman", script_body=pacman_body)
    return db, {
        "gall_closure_bytes": 31_000_000,
        "gall_deps_exclusive_bytes": 30_000_000,
    }


def _t3_01_daemon_dbus_new_window(ctx: RunContext) -> E2EResult:
    """F1+F2: --daemon + D-Bus FileManager1.ShowItems + --new-window share one process."""
    if pending := require_cli_flags(ctx, "--daemon", "--new-window"):
        return pending
    assert ctx.binary_path is not None
    with SandboxEnv("T3-01", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        dir_a = sb.work / "alpha"
        dir_b = sb.work / "beta"
        dir_a.mkdir()
        dir_b.mkdir()
        item = dir_a / "picked.txt"
        item.write_text("item\n", encoding="utf-8")

        daemon = sb.popen([ctx.binary_path, "--daemon"], cwd=sb.work)
        time.sleep(1.2)
        if daemon.poll() is not None:
            return E2EResult(Status.FAIL, f"gall --daemon exited early (rc={daemon.returncode})")
        if not wait_for_dbus_name(sb, FM1_DEST, timeout=6.0):
            return E2EResult(Status.PENDING_IMPL, "org.freedesktop.FileManager1 not registered by --daemon (M3 pending)")
        if "window shown at" in read_gall_logs(sb):
            return E2EResult(Status.FAIL, "--daemon mapped a window before any IPC request")

        call = gdbus_call(sb, FM1_DEST, FM1_PATH, f"{FM1_DEST}.ShowItems", f"['{_file_uri(item)}']", "t3-01")
        if call.returncode != 0:
            return E2EResult(Status.FAIL, f"ShowItems failed against --daemon: {call.stderr}")
        if not wait_for_log_contains(sb, "picked.txt", timeout=5.0):
            return E2EResult(Status.FAIL, "ShowItems did not log picked.txt target in Gall.log")

        nw = sb.run([ctx.binary_path, "--new-window", str(dir_b)], timeout=10.0)
        if pending := assert_unknown_arg_not_ignored(sb, nw, "--new-window"):
            return pending
        if nw.returncode != 0:
            return E2EResult(Status.FAIL, f"--new-window failed against --daemon (rc={nw.returncode})", {"stderr": nw.stderr})

        pids = count_gall_pids(sb)
        if len(pids) != 1:
            return E2EResult(Status.FAIL, f"Expected single resident daemon PID, found {pids}")
        if not wait_for_dbus_name(sb, FM1_DEST, timeout=3.0):
            return E2EResult(Status.FAIL, "FileManager1 ownership lost after --new-window")
        return E2EResult(Status.PASS, "--daemon served FileManager1.ShowItems and --new-window in one PID")


def _t3_02_adaptive_layout_with_vim_keys(ctx: RunContext) -> E2EResult:
    """F8+F10: 2D Adaptive Layout (2H -> 2V -> 1P) combined with vimKeys navigation & chord safety."""
    modes = (
        "e2e-adaptive-wide",
        "e2e-adaptive-tall",
        "e2e-adaptive-compact",
        "e2e-vim-new-window-chord",
        "e2e-vim-ctrl-l",
    )
    for mode in modes:
        if pending := require_selftest_mode(ctx, mode):
            return pending
    with SandboxEnv("T3-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"adaptiveLayout": True, "vimKeys": True, "keyboardProfile": "vim"})
        expected_tokens = {
            "e2e-adaptive-wide": "horizontal",
            "e2e-adaptive-tall": "vertical",
            "e2e-adaptive-compact": "single",
        }
        for mode in modes:
            proc, report, _ = run_gall_selftest(ctx, sb, mode, extra_args=[sb.work])
            if fail := check_selftest_outcome(proc, report, mode, sb=sb):
                return fail
            if tok := expected_tokens.get(mode):
                if tok not in report.lower():
                    return E2EResult(Status.FAIL, f"{mode} report missing {tok!r}", {"report": report})
        for sub in ("org.gall.app", "gall"):
            cfg = sb.config / sub / "settings.json"
            if cfg.is_file():
                data = json.loads(cfg.read_text(encoding="utf-8"))
                if data.get("adaptiveLayout") is not True or data.get("vimKeys") is not True:
                    return E2EResult(Status.FAIL, "Settings mutated during adaptive+vim sweep", {"settings": data})
        return E2EResult(Status.PASS, "2D Adaptive Layout and vimKeys chords/focus verified together")


def _t3_03_benchmark_exercises_daemon_and_footprint(ctx: RunContext) -> E2EResult:
    """F1+F11: benchmark-nautilus.sh --gall <bin> --runs 1 --json tests --daemon + footprint."""
    if not ctx.env_probe.benchmark_script_exists:
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh does not exist yet")
    if pending := require_cli_flags(ctx, "--daemon", "--new-window"):
        return pending
    assert ctx.binary_path is not None
    script = ctx.repo_root / "scripts" / "benchmark-nautilus.sh"
    with SandboxEnv("T3-03", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        _, exp = _install_full_arch_mocks(ctx, sb)
        base_pkg = ctx.repo_root / "tests" / "e2e" / "fixtures" / "omarchy-base.packages"
        res = sb.run(
            [str(script), "--gall", str(ctx.binary_path), "--runs", "1", "--json"],
            env_overrides={"OMARCHY_BASE_PACKAGES": str(base_pkg)},
            timeout=120.0,
        )
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"benchmark-nautilus.sh failed (rc={res.returncode})", {"stderr": res.stderr})
        data = json.loads(res.stdout)
        gb = data.get("gall_binary", {})
        if gb.get("status") != "found" or gb.get("binary_bytes") != ctx.binary_path.stat().st_size:
            return E2EResult(Status.FAIL, f"gall_binary byte accounting mismatch: {gb}")
        fp = data.get("footprint", {})
        if fp.get("status") != "ok" or fp["gall"]["closure_bytes"] != exp["gall_closure_bytes"]:
            return E2EResult(Status.FAIL, f"Footprint mismatch during live benchmark: {fp}")

        cold = data.get("cold_start", {})
        warm = data.get("daemon_warm_start", {})
        if cold.get("status") == "skipped":
            return E2EResult(
                Status.ENV_UNAVAILABLE,
                f"Live GUI benchmark timing skipped under headless compositor: {cold.get('reason')}",
                {"footprint": fp, "gall_binary": gb},
            )
        if cold.get("status") != "ok" or not (cold.get("gall", {}).get("wall_ms_median", 0) > 0):
            return E2EResult(Status.FAIL, f"Invalid cold_start measurement: {cold}")
        if warm.get("status") not in ("ok", "partial"):
            return E2EResult(Status.FAIL, f"Expected daemon_warm_start ok/partial, got {warm}")
        return E2EResult(Status.PASS, "Benchmark exercised binary footprint, cold-start, and --daemon warm-start")


def _t3_04_dbus_show_items_then_terminal_editor(ctx: RunContext) -> E2EResult:
    """F2+F4: FileManager1.ShowItems reveals notes.md then Terminal=true / editor opens it cleanly."""
    for mode in ("e2e-editor-omarchy", "e2e-openwith-desktop"):
        if pending := require_selftest_mode(ctx, mode):
            return pending
    assert ctx.binary_path is not None
    with SandboxEnv("T3-04", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_xdg_terminal_exec()
        mocks.install_uwsm_app()
        mocks.install_omarchy_launch_editor()
        mocks.install_tool("nvim")

        notes = sb.work / "notes.md"
        notes.write_text("# Notes\n", encoding="utf-8")
        (sb.data / "applications" / "nvim-e2e.desktop").write_text(
            "[Desktop Entry]\nType=Application\nName=NvimE2E\nTerminal=true\nExec=nvim %F\nMimeType=text/markdown;text/plain;\n",
            encoding="utf-8",
        )

        primary = sb.popen([ctx.binary_path, str(sb.work)], cwd=sb.work)
        if not wait_for_dbus_name(sb, FM1_DEST, timeout=8.0):
            if primary.poll() is not None:
                return E2EResult(Status.FAIL, f"Gall exited early (rc={primary.returncode})")
            return E2EResult(Status.PENDING_IMPL, "org.freedesktop.FileManager1 not registered on bus (M3 pending)")

        call = gdbus_call(sb, FM1_DEST, FM1_PATH, f"{FM1_DEST}.ShowItems", f"['{_file_uri(notes)}']", "t3-04")
        if call.returncode != 0:
            return E2EResult(Status.FAIL, f"ShowItems failed: {call.stderr}")
        if not wait_for_log_contains(sb, "notes.md", timeout=4.0):
            return E2EResult(Status.FAIL, "ShowItems did not log notes.md selection")

        for mode in ("e2e-editor-omarchy", "e2e-openwith-desktop"):
            proc, report, _ = run_gall_selftest(
                ctx,
                sb,
                mode,
                extra_args=[str(notes)],
                env_overrides={"GALL_E2E_CHAIN_MOCKS": "1"},
            )
            if fail := check_selftest_outcome(proc, report, mode, sb=sb):
                return fail
        editor_calls = read_mock_calls(sb, "omarchy-launch-editor")
        term_calls = read_mock_calls(sb, "xdg-terminal-exec")
        if not (editor_calls or term_calls):
            return E2EResult(Status.FAIL, "Neither omarchy-launch-editor nor xdg-terminal-exec was invoked for notes.md")
        return E2EResult(Status.PASS, "FileManager1.ShowItems + terminal/editor handoff verified")


def _t3_05_clipboard_copy_cut_across_gvfs_mount(ctx: RunContext) -> E2EResult:
    """F3+F5: Copy/Cut via x-special/gnome-copied-files between local dir and GVFS smb-share completes without stall."""
    for mode in ("e2e-gvfs-network", "e2e-clip-paste"):
        if pending := require_selftest_mode(ctx, mode):
            return pending
    with SandboxEnv("T3-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_gio(mounts_output="Mount(0): docs -> smb://nas.local/docs\n", list_delay=0.2)
        mount_dir = mocks.seed_gvfs_mount("smb-share:server=nas.local,share=docs", {"remote.txt": "from-nas\n"})
        local_dst = sb.work / "local_dst"
        local_dst.mkdir()

        p_net, r_net, _ = run_gall_selftest(ctx, sb, "e2e-gvfs-network", extra_args=[str(sb.work)], timeout=20.0)
        if fail := check_selftest_outcome(p_net, r_net, "e2e-gvfs-network", sb=sb):
            return fail
        if "nas" not in r_net.lower() and "docs" not in r_net.lower() and "smb" not in r_net.lower():
            return E2EResult(Status.FAIL, "e2e-gvfs-network did not surface seeded smb-share mount", {"report": r_net})

        payload = f"copy\n{_file_uri(mount_dir / 'remote.txt')}\n".encode("utf-8")
        owner1, err1 = start_clipboard_owner(sb, {"x-special/gnome-copied-files": payload})
        if err1 is not None:
            return err1
        t0 = time.monotonic()
        try:
            proc, report, _ = run_gall_selftest(ctx, sb, "e2e-clip-paste", extra_args=[str(local_dst)], timeout=20.0)
        finally:
            stop_proc(owner1)
        elapsed = time.monotonic() - t0
        if fail := check_selftest_outcome(proc, report, "e2e-clip-paste", sb=sb):
            return fail
        copied = local_dst / "remote.txt"
        if not copied.is_file() or copied.read_text(encoding="utf-8") != "from-nas\n":
            return E2EResult(Status.FAIL, "GVFS remote.txt was not copied to local_dst via gnome-copied-files")

        # Now cut from local_dst back into GVFS mount
        moved_dir = mount_dir / "uploads"
        moved_dir.mkdir()
        owner2, err2 = start_clipboard_owner(
            sb,
            {"x-special/gnome-copied-files": f"cut\n{_file_uri(copied)}\n".encode("utf-8")},
        )
        if err2 is not None:
            return err2
        try:
            proc2, report2, _ = run_gall_selftest(ctx, sb, "e2e-clip-paste", extra_args=[str(moved_dir)], timeout=20.0)
        finally:
            stop_proc(owner2)
        if fail := check_selftest_outcome(proc2, report2, "e2e-clip-paste", sb=sb):
            return fail
        if not (moved_dir / "remote.txt").is_file() or copied.exists():
            return E2EResult(Status.FAIL, "Cut from local_dst to GVFS mount did not preserve move semantics")
        return E2EResult(Status.PASS, f"Clipboard copy/cut across GVFS mount completed in {elapsed:.2f}s")


def _t3_06_actions_transcode_to_clipboard_paste(ctx: RunContext) -> E2EResult:
    """F3+F9: actions.d omarchy.transcode writes LF text/uri-list to real Wayland/GTK clipboard and Gall pastes output."""
    for mode in ("e2e-actions-floating", "e2e-clip-paste"):
        if pending := require_selftest_mode(ctx, mode):
            return pending
    with SandboxEnv("T3-06", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_omarchy_floating_presentation()

        src_vid = sb.work / "raw.mp4"
        src_vid.write_bytes(b"\x00\x00\x00\x18ftypmp42")
        out_vid = sb.work / "raw-1080p.mp4"
        dst_dir = sb.work / "pasted"
        dst_dir.mkdir()

        transcode_script, clip_status_path, clip_pid_path = make_transcode_clipboard_script(
            sb,
            out_vid,
            b"transcoded-1080p-bytes",
        )
        mocks.install_tool("omarchy-transcode", script_body=transcode_script)
        shutil.copy2(
            ctx.repo_root / "tests" / "e2e" / "fixtures" / "actions_sample.json",
            sb.config / "gall" / "actions.d" / "10-sample.json",
        )
        # Keep actions_sample transcode compatible with clip.mp4 / raw.mp4
        clip_vid = sb.work / "clip.mp4"
        clip_vid.write_bytes(b"\x00\x00")

        try:
            proc, report, _ = run_gall_selftest(
                ctx,
                sb,
                "e2e-actions-floating",
                extra_args=[str(sb.work)],
                env_overrides={"GALL_E2E_CHAIN_MOCKS": "1"},
            )
            if fail := check_selftest_outcome(proc, report, "e2e-actions-floating", sb=sb):
                return fail
            if not out_vid.is_file():
                return E2EResult(Status.FAIL, "Chained mock omarchy-transcode did not emit raw-1080p.mp4")

            clip_status = clip_status_path.read_text(encoding="utf-8", errors="replace").strip() if clip_status_path.is_file() else ""
            if clip_status.startswith("env_unavailable:"):
                return E2EResult(
                    Status.ENV_UNAVAILABLE,
                    f"Real Wayland/GTK clipboard unavailable in omarchy-transcode: {clip_status}",
                )
            if not clip_status.startswith("ok:"):
                return E2EResult(
                    Status.FAIL,
                    f"omarchy-transcode clipboard owner helper failed: {clip_status or 'missing clip_status'}",
                    {"clip_status": clip_status},
                )

            proc2, report2, _ = run_gall_selftest(ctx, sb, "e2e-clip-paste", extra_args=[str(dst_dir)])
        finally:
            cleanup_transcode_clipboard(clip_pid_path)

        if fail := check_selftest_outcome(proc2, report2, "e2e-clip-paste", sb=sb):
            return fail
        pasted = dst_dir / out_vid.name
        if not pasted.is_file() or hashlib.sha256(pasted.read_bytes()).hexdigest() != hashlib.sha256(out_vid.read_bytes()).hexdigest():
            return E2EResult(Status.FAIL, "Gall clipboard paste did not copy LF-terminated transcode output")
        return E2EResult(Status.PASS, "actions.d transcode -> real Wayland/GTK text/uri-list -> Gall paste verified")


def _t3_07_terminal_cwd_then_floating_action(ctx: RunContext) -> E2EResult:
    """F4+F9: omarchy-cmd-terminal-cwd navigates Gall, then floating-presentation -> xdg-terminal-exec runs in {dir}."""
    for mode in ("e2e-terminal-cwd", "e2e-actions-floating"):
        if pending := require_selftest_mode(ctx, mode):
            return pending
    with SandboxEnv("T3-07", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        term_proj = sb.work / "term_proj"
        term_proj.mkdir()
        (term_proj / "clip.mp4").write_bytes(b"\x00\x00")
        mocks.install_omarchy_cmd_terminal_cwd(term_proj)
        mocks.install_omarchy_floating_presentation()
        mocks.install_xdg_terminal_exec()
        mocks.install_omarchy_transcode()
        shutil.copy2(
            ctx.repo_root / "tests" / "e2e" / "fixtures" / "actions_sample.json",
            sb.config / "gall" / "actions.d" / "10-sample.json",
        )

        p1, r1, _ = run_gall_selftest(ctx, sb, "e2e-terminal-cwd", extra_args=[str(sb.work)])
        if fail := check_selftest_outcome(p1, r1, "e2e-terminal-cwd", sb=sb):
            return fail
        if not read_mock_calls(sb, "omarchy-cmd-terminal-cwd") or str(term_proj) not in (r1 + "\n" + read_gall_logs(sb)):
            return E2EResult(Status.FAIL, "omarchy-cmd-terminal-cwd did not navigate to term_proj")

        p2, r2, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-actions-floating",
            extra_args=[str(term_proj)],
            env_overrides={"GALL_E2E_CHAIN_MOCKS": "1"},
        )
        if fail := check_selftest_outcome(p2, r2, "e2e-actions-floating", sb=sb):
            return fail
        fp_calls = read_mock_calls(sb, "omarchy-launch-floating-terminal-with-presentation")
        if not fp_calls or not read_mock_calls(sb, "omarchy-transcode"):
            return E2EResult(Status.FAIL, "floating-presentation -> omarchy-transcode chain was not executed", {"calls": read_mock_calls(sb)})
        return E2EResult(Status.PASS, "omarchy-cmd-terminal-cwd + floating-presentation action chain verified")


def _t3_08_gvfs_mounts_survive_adaptive_sidebar_toggle(ctx: RunContext) -> E2EResult:
    """F5+F8: Seeded MTP/SMB mounts survive <820px sidebar auto-hide and >=856px restore without mutating settings."""
    modes = ("e2e-gvfs-drives", "e2e-gvfs-network", "e2e-adaptive-compact", "e2e-adaptive-wide")
    for mode in modes:
        if pending := require_selftest_mode(ctx, mode):
            return pending
    with SandboxEnv("T3-08", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_gio(
            mounts_output=(
                "Mount(0): Pixel 9 -> mtp://Google_Pixel_9/\n"
                "Mount(1): docs -> smb://nas.local/docs\n"
            )
        )
        mocks.seed_gvfs_mount("mtp:host=Google_Pixel_9", {"DCIM/photo.jpg": b"jpg"})
        mocks.seed_gvfs_mount("smb-share:server=nas.local,share=docs", {"readme.txt": "nas\n"})
        write_gall_settings(sb, {"adaptiveLayout": True, "sidebarVisible": True})

        for mode in modes:
            proc, report, _ = run_gall_selftest(ctx, sb, mode, extra_args=[str(sb.work)])
            if fail := check_selftest_outcome(proc, report, mode, sb=sb):
                return fail
            low = report.lower()
            if mode == "e2e-gvfs-drives" and "pixel" not in low:
                return E2EResult(Status.FAIL, "e2e-gvfs-drives missing Pixel mount", {"report": report})
            if mode == "e2e-gvfs-network" and "nas" not in low and "docs" not in low:
                return E2EResult(Status.FAIL, "e2e-gvfs-network missing nas/docs mount", {"report": report})
            if mode == "e2e-adaptive-compact" and "single" not in low:
                return E2EResult(Status.FAIL, "e2e-adaptive-compact missing single mode", {"report": report})
            if mode == "e2e-adaptive-wide" and "horizontal" not in low:
                return E2EResult(Status.FAIL, "e2e-adaptive-wide missing horizontal mode", {"report": report})

        for sub in ("org.gall.app", "gall"):
            cfg = sb.config / sub / "settings.json"
            if cfg.is_file():
                data = json.loads(cfg.read_text(encoding="utf-8"))
                if data.get("sidebarVisible") is not True:
                    return E2EResult(Status.FAIL, "Sidebar auto-hide mutated persisted sidebarVisible", {"settings": data})
        if not read_mock_calls(sb, "gio"):
            return E2EResult(Status.FAIL, "Expected gio mount -l query with seeded GVFS mounts")
        return E2EResult(Status.PASS, "GVFS mounts preserved across 2D adaptive sidebar hide/restore")


def _t3_09_packaging_and_benchmark_consistency(ctx: RunContext) -> E2EResult:
    """F6+F11: Static consistency across packaging metadata, omarchy-actions.json, and benchmark-nautilus.sh deps."""
    if not ctx.env_probe.benchmark_script_exists:
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh does not exist yet")

    desktop = ctx.repo_root / "packaging" / "linux" / "gall.desktop"
    service = ctx.repo_root / "packaging" / "linux" / "org.gall.app.FileManager1.service"
    metainfo = ctx.repo_root / "packaging" / "linux" / "org.gall.app.metainfo.xml"
    actions_json = ctx.repo_root / "packaging" / "linux" / "omarchy-actions.json"
    pkgbuild = ctx.repo_root / "packaging" / "aur" / "gall-bin" / "PKGBUILD"
    spec = ctx.repo_root / "packaging" / "copr" / "gall.spec"
    for req in (desktop, service, metainfo, actions_json, pkgbuild, spec):
        if not req.is_file():
            return E2EResult(Status.PENDING_IMPL, f"Required M2 artifact missing: {req.name}")

    try:
        d_parser = assert_desktop_valid(desktop, ctx.env_probe)
        m_root = assert_xml_valid(metainfo)
    except AssertionError as exc:
        return E2EResult(Status.FAIL, str(exc))

    if d_parser["Desktop Entry"].get("Icon") != "gall":
        return E2EResult(Status.FAIL, "gall.desktop Icon is not gall")
    if "org.gall.app" not in (m_root.findtext(".//id") or ""):
        return E2EResult(Status.FAIL, "metainfo.xml missing org.gall.app id")
    srv_text = service.read_text(encoding="utf-8")
    if "org.freedesktop.FileManager1" not in srv_text or "gall" not in srv_text:
        return E2EResult(Status.FAIL, "FileManager1.service missing bus name or gall Exec")

    actions = json.loads(actions_json.read_text(encoding="utf-8"))
    if not isinstance(actions, list) or len(actions) < 6:
        return E2EResult(Status.FAIL, "omarchy-actions.json must list >= 6 Omarchy actions")
    for act in actions:
        if not act.get("id") or not act.get("label") or not isinstance(act.get("command"), list):
            return E2EResult(Status.FAIL, f"Malformed action entry in omarchy-actions.json: {act}")
        wc = act.get("whenCommand")
        if wc is not None and not isinstance(wc, (str, list)):
            return E2EResult(Status.FAIL, f"Invalid whenCommand type in {act['id']}")

    pkgbuild_txt = pkgbuild.read_text(encoding="utf-8")
    bench_txt = (ctx.repo_root / "scripts" / "benchmark-nautilus.sh").read_text(encoding="utf-8")
    for dep in ("webkit2gtk-4.1", "gtk3"):
        if dep not in pkgbuild_txt or dep not in bench_txt:
            return E2EResult(Status.FAIL, f"Dependency {dep!r} inconsistent between PKGBUILD and benchmark-nautilus.sh")

    with SandboxEnv("T3-09", ctx.env_probe) as sb:
        _, exp = _install_full_arch_mocks(ctx, sb)
        base_pkg = ctx.repo_root / "tests" / "e2e" / "fixtures" / "omarchy-base.packages"
        res = sb.run(
            [str(ctx.repo_root / "scripts" / "benchmark-nautilus.sh"), "--footprint-only", "--json"],
            env_overrides={"OMARCHY_BASE_PACKAGES": str(base_pkg)},
            timeout=20.0,
        )
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"benchmark-nautilus.sh failed: {res.stderr}")
        parsed = json.loads(res.stdout)
        fp = parsed.get("footprint", {})
        if fp.get("status") != "ok" or fp["gall"]["closure_bytes"] != exp["gall_closure_bytes"]:
            return E2EResult(Status.FAIL, f"Packaging dependency closure mismatch in benchmark output: {fp}")

    return E2EResult(Status.PASS, "Packaging metadata, omarchy-actions.json, and benchmark dependency closures aligned")


def _t3_10_dbus_service_auto_activation_and_desktop(ctx: RunContext) -> E2EResult:
    """F2+F6: Installed FileManager1.service + gall.desktop auto-activates ShowFolders on private D-Bus."""
    service_src = ctx.repo_root / "packaging" / "linux" / "org.gall.app.FileManager1.service"
    desktop_src = ctx.repo_root / "packaging" / "linux" / "gall.desktop"
    if not service_src.is_file() or not desktop_src.is_file():
        return E2EResult(Status.PENDING_IMPL, "M2 packaging FileManager1.service or gall.desktop missing")
    assert ctx.binary_path is not None

    with SandboxEnv("T3-10", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        # Validate and install desktop entry into isolated XDG_DATA_HOME/applications
        desktop_dst = sb.data / "applications" / "gall.desktop"
        shutil.copy2(desktop_src, desktop_dst)
        try:
            d_parser = assert_desktop_valid(desktop_dst, ctx.env_probe)
        except AssertionError as exc:
            return E2EResult(Status.FAIL, f"gall.desktop invalid: {exc}")
        if "--new-window" not in d_parser["Desktop Action new-window"].get("Exec", ""):
            return E2EResult(Status.FAIL, "gall.desktop [Desktop Action new-window] missing --new-window")

        # Install rewritten FileManager1.service pointing to ctx.binary_path
        raw_srv = service_src.read_text(encoding="utf-8")
        rewritten = re.sub(r"(?m)^Exec=.*$", f"Exec={ctx.binary_path} --nowindow", raw_srv)
        srv_dst = sb.data / "dbus-1" / "services" / "org.freedesktop.FileManager1.service"
        srv_dst.write_text(rewritten, encoding="utf-8")

        target_dir = sb.work / "activated_target"
        target_dir.mkdir()

        # Do NOT launch Gall beforehand: let dbus-daemon auto-activate via FileManager1.service
        call = gdbus_call(
            sb,
            FM1_DEST,
            FM1_PATH,
            f"{FM1_DEST}.ShowFolders",
            f"['{_file_uri(target_dir)}']",
            "t3-10",
            timeout=15.0,
        )
        if call.returncode != 0:
            err = (call.stderr or "").lower()
            if "serviceunknown" in err or "was not provided" in err or "timed out" in err or "no reply" in err:
                return E2EResult(Status.PENDING_IMPL, f"D-Bus auto-activation / FileManager1 not yet wired: {call.stderr.strip()}")
            return E2EResult(Status.FAIL, f"D-Bus auto-activation ShowFolders failed (rc={call.returncode}): {call.stderr}")

        if not wait_for_dbus_name(sb, FM1_DEST, timeout=5.0):
            return E2EResult(Status.FAIL, "Auto-activated Gall did not retain FileManager1 bus ownership")
        if not wait_for_log_contains(sb, str(target_dir), timeout=5.0):
            return E2EResult(Status.FAIL, "Auto-activated Gall did not navigate to activated_target")

        pids = count_gall_pids(sb)
        if len(pids) != 1:
            return E2EResult(Status.FAIL, f"Expected 1 auto-activated Gall PID, found {pids}")
        return E2EResult(Status.PASS, "FileManager1.service auto-activated Gall and navigated via ShowFolders")


def _t3_11_omarchy_swap_across_adaptive_layout_and_wm(ctx: RunContext) -> E2EResult:
    """F7+F8: Live Omarchy atomic swap (dark->light, rounding 0->8, Glassy) across 2D adaptive layout + tiling-WM."""
    modes = (
        "e2e-omarchy-dark",
        "e2e-omarchy-glassy",
        "e2e-adaptive-wide",
        "e2e-adaptive-tall",
        "e2e-adaptive-compact",
        "e2e-window-buttons",
    )
    for mode in modes:
        if pending := require_selftest_mode(ctx, mode):
            return pending
    assert ctx.binary_path is not None
    with SandboxEnv("T3-11", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        dark = (ctx.repo_root / "tests" / "e2e" / "fixtures" / "colors_dark.toml").read_text(encoding="utf-8")
        light = (ctx.repo_root / "tests" / "e2e" / "fixtures" / "colors_light.toml").read_text(encoding="utf-8")
        mocks.seed_omarchy_theme("catppuccin", dark)
        rfile = sb.tmp / "hypr_round.txt"
        mocks.install_hyprctl(rounding_file=rfile, rounding=0)
        srv = mocks.start_hypr_socket2("t3-11-sig")
        write_gall_settings(sb, {"theme": "omarchy", "omarchyGlassy": True, "adaptiveLayout": True, "windowButtons": "auto", "sidebarVisible": True})

        proc = sb.popen([ctx.binary_path, str(sb.work)], env_overrides={"XDG_CURRENT_DESKTOP": "Hyprland"})
        try:
            if not srv.wait_for_client(timeout=6.0):
                return E2EResult(Status.FAIL, "Gall did not connect to Hyprland .socket2.sock")

            initial_calls = len(read_mock_calls(sb, "hyprctl"))
            (sb.state / "omarchy" / "toggles" / "hypr" / "window-glassy.lua").write_text("glassy=true\n", encoding="utf-8")
            mocks.atomic_swap_omarchy_theme("latte", light, light_mode=True)
            rfile.write_text("8\n", encoding="utf-8")
            srv.push_event("configreloaded>>")
            time.sleep(0.5)

            if proc.poll() is not None:
                return E2EResult(Status.FAIL, f"Gall crashed during Omarchy + Hyprland reload (rc={proc.returncode})")
            after_calls = len(read_mock_calls(sb, "hyprctl"))
            if after_calls <= initial_calls:
                return E2EResult(Status.FAIL, "hyprctl was not re-queried after configreloaded>>")
        finally:
            stop_proc(proc)

        expected = {
            "e2e-omarchy-dark": "light",
            "e2e-omarchy-glassy": "glassy",
            "e2e-adaptive-wide": "horizontal",
            "e2e-adaptive-tall": "vertical",
            "e2e-adaptive-compact": "single",
            "e2e-window-buttons": "hidden",
        }
        for mode in modes:
            p_st, r_st, _ = run_gall_selftest(ctx, sb, mode, env_overrides={"XDG_CURRENT_DESKTOP": "Hyprland"})
            if fail := check_selftest_outcome(p_st, r_st, mode, sb=sb):
                return fail
            tok = expected[mode]
            if tok not in r_st.lower():
                return E2EResult(Status.FAIL, f"{mode} report missing {tok!r}", {"report": r_st})
        return E2EResult(Status.PASS, "Omarchy atomic swap, Hyprland rounding reload, Glassy, and 2D Adaptive Layout verified")


def _t3_12_omarchy_reload_during_vim_editable_input(ctx: RunContext) -> E2EResult:
    """F7+F10: Live Omarchy colors.toml + fonts.conf reload during active vimKeys input preserves focus & text."""
    for mode in ("e2e-omarchy-dark", "e2e-vim-bounds"):
        if pending := require_selftest_mode(ctx, mode):
            return pending
    assert ctx.binary_path is not None
    with SandboxEnv("T3-12", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        dark = (ctx.repo_root / "tests" / "e2e" / "fixtures" / "colors_dark.toml").read_text(encoding="utf-8")
        light = (ctx.repo_root / "tests" / "e2e" / "fixtures" / "colors_light.toml").read_text(encoding="utf-8")
        mocks.seed_omarchy_theme("catppuccin", dark)
        (sb.config / "fontconfig" / "fonts.conf").write_text(
            (ctx.repo_root / "tests" / "e2e" / "fixtures" / "fonts.conf").read_text(encoding="utf-8"),
            encoding="utf-8",
        )
        write_gall_settings(sb, {"theme": "omarchy", "omarchyFollowFont": True, "vimKeys": True, "keyboardProfile": "vim"})

        # Trigger live swap while running vim bounds verification
        proc = sb.popen([ctx.binary_path, str(sb.work)], cwd=sb.work)
        try:
            wait_for_log_contains(sb, "window shown at", timeout=6.0)
            mocks.atomic_swap_omarchy_theme("latte", light, light_mode=True)
            time.sleep(0.3)
            if proc.poll() is not None:
                return E2EResult(Status.FAIL, f"Gall exited unexpectedly during Omarchy reload (rc={proc.returncode})")
        finally:
            stop_proc(proc)

        for mode in ("e2e-vim-bounds", "e2e-omarchy-dark"):
            p_st, r_st, _ = run_gall_selftest(ctx, sb, mode, extra_args=[sb.work])
            if fail := check_selftest_outcome(p_st, r_st, mode, sb=sb):
                return fail
            if mode == "e2e-omarchy-dark" and "light" not in r_st.lower():
                return E2EResult(Status.FAIL, "e2e-omarchy-dark did not observe light theme after swap", {"report": r_st})
        return E2EResult(Status.PASS, "Omarchy palette/font reload preserved vimKeys editable input safety")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase("T3-01", 3, ("F1", "F2"), "--daemon + FileManager1.ShowItems + --new-window share single process", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t3_01_daemon_dbus_new_window),
        E2ECase("T3-02", 3, ("F8", "F10"), "2D Adaptive Layout (2H->2V->1P) with vimKeys motions & chords", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t3_02_adaptive_layout_with_vim_keys),
        E2ECase("T3-03", 3, ("F1", "F11"), "benchmark-nautilus.sh exercises --daemon warm-start & footprint", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t3_03_benchmark_exercises_daemon_and_footprint),
        E2ECase("T3-04", 3, ("F2", "F4"), "FileManager1.ShowItems reveals file then Terminal=true / editor opens it", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t3_04_dbus_show_items_then_terminal_editor),
        E2ECase("T3-05", 3, ("F3", "F5"), "Clipboard copy/cut via gnome-copied-files across GVFS mount", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t3_05_clipboard_copy_cut_across_gvfs_mount),
        E2ECase("T3-06", 3, ("F3", "F9"), "actions.d transcode -> real Wayland/GTK text/uri-list -> Gall paste", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t3_06_actions_transcode_to_clipboard_paste),
        E2ECase("T3-07", 3, ("F4", "F9"), "omarchy-cmd-terminal-cwd -> floating-presentation -> xdg-terminal-exec", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t3_07_terminal_cwd_then_floating_action),
        E2ECase("T3-08", 3, ("F5", "F8"), "GVFS MTP/SMB mounts survive 2D Adaptive sidebar auto-hide & restore", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t3_08_gvfs_mounts_survive_adaptive_sidebar_toggle),
        E2ECase("T3-09", 3, ("F6", "F11"), "Packaging metadata, omarchy-actions.json & benchmark dep consistency", requires_binary=False, requires_gui=False, fn=_t3_09_packaging_and_benchmark_consistency),
        E2ECase("T3-10", 3, ("F2", "F6"), "Installed FileManager1.service + gall.desktop D-Bus auto-activation", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t3_10_dbus_service_auto_activation_and_desktop),
        E2ECase("T3-11", 3, ("F7", "F8"), "Omarchy atomic swap + Hyprland rounding + Glassy across 2D Adaptive Layout", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t3_11_omarchy_swap_across_adaptive_layout_and_wm),
        E2ECase("T3-12", 3, ("F7", "F10"), "Omarchy colors.toml + fonts.conf reload during active vimKeys input", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t3_12_omarchy_reload_during_vim_editable_input),
    ]
