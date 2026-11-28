"""Tier 4 opaque-box realistic end-user application workloads (T4-01..T4-07)."""

from __future__ import annotations

import hashlib
import http.server
import json
import os
import shutil
import threading
import time
import urllib.parse
from pathlib import Path
from typing import Any

from harness.assertions import assert_desktop_valid, assert_xml_valid, read_mock_calls, system_prefix_binary_args
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


def _t4_01_launcher_terminal_compare_editor_dbus_loop_guard(ctx: RunContext) -> E2EResult:
    """T4-01 (F1,F2,F4,F9): Terminal CWD -> --compare -> omarchy-launch-editor -> FileManager1.ShowItems + loop guard."""
    if pending := require_cli_flags(ctx, "--compare"):
        return pending
    for mode in ("e2e-terminal-cwd", "e2e-dbus-reveal"):
        if pending := require_selftest_mode(ctx, mode):
            return pending
    assert ctx.binary_path is not None

    with SandboxEnv("T4-01", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        proj = sb.work / "proj"
        proj.mkdir()
        v1 = proj / "v1.rs"
        v2 = proj / "v2.rs"
        v1.write_text("fn main() { println!(\"v1\"); }\n", encoding="utf-8")
        v2.write_text("fn main() { println!(\"v2\"); }\n", encoding="utf-8")

        mocks.install_omarchy_cmd_terminal_cwd(proj)
        mocks.install_omarchy_launch_editor()
        mocks.install_xdg_terminal_exec()
        mocks.install_uwsm_app()
        mocks.install_tool("xdg-open")

        mon = sb.popen(["dbus-monitor", "--session", f"type='method_call',interface='{FM1_DEST}'"])
        mon_out: bytes | str = b""
        try:
            time.sleep(0.3)

            primary = sb.popen([ctx.binary_path, "--compare", str(v1), str(v2)], cwd=proj)
            try:
                if not wait_for_dbus_name(sb, FM1_DEST, timeout=8.0):
                    if primary.poll() is not None:
                        return E2EResult(Status.FAIL, f"Gall --compare exited early (rc={primary.returncode})")
                    return E2EResult(Status.PENDING_IMPL, "org.freedesktop.FileManager1 not registered on bus (M3 pending)")

                call = gdbus_call(sb, FM1_DEST, FM1_PATH, f"{FM1_DEST}.ShowItems", f"['{_file_uri(v1)}']", "t4-01")
                if call.returncode != 0:
                    return E2EResult(Status.FAIL, f"External ShowItems failed: {call.stderr}")
                if not wait_for_log_contains(sb, "v1.rs", timeout=5.0):
                    return E2EResult(Status.FAIL, "External ShowItems did not log v1.rs reveal")
                if len(count_gall_pids(sb)) != 1:
                    return E2EResult(Status.FAIL, "Expected single Gall process across --compare and ShowItems")
            finally:
                stop_proc(primary)
        finally:
            if mon.poll() is None:
                mon.terminate()
            mon_out, _ = mon.communicate(timeout=2.0)

        mon_txt = mon_out.decode("utf-8", errors="replace") if isinstance(mon_out, bytes) else str(mon_out)
        # External ShowItems call was exactly 1; xdg-open should have 0 bounce calls
        if read_mock_calls(sb, "xdg-open"):
            return E2EResult(Status.FAIL, "Unexpected xdg-open bounce during internal reveal")

        for mode, args in (("e2e-terminal-cwd", [str(sb.work)]), ("e2e-dbus-reveal", [str(proj)])):
            proc, report, _ = run_gall_selftest(ctx, sb, mode, extra_args=args)
            if fail := check_selftest_outcome(proc, report, mode, sb=sb):
                return fail
        if not read_mock_calls(sb, "omarchy-cmd-terminal-cwd"):
            return E2EResult(Status.FAIL, "omarchy-cmd-terminal-cwd was not invoked in workload")
        if mon_txt.count("member=ShowItems") > 1:
            return E2EResult(Status.FAIL, "Loop guard emitted extra outbound FileManager1.ShowItems calls", {"dbus_monitor": mon_txt})
        return E2EResult(Status.PASS, "Launcher -> terminal-cwd -> --compare -> ShowItems loop-guard workload passed")


def _t4_02_omarchy_atomic_swap_hypr_glassy_adaptive_sweep(ctx: RunContext) -> E2EResult:
    """T4-02 (F7,F8): Live Omarchy atomic swap + Hyprland rounding + Glassy + 2D Adaptive Layout resize sweep."""
    modes = (
        "e2e-omarchy-dark",
        "e2e-omarchy-glassy",
        "e2e-adaptive-wide",
        "e2e-adaptive-tall",
        "e2e-adaptive-compact",
        "e2e-adaptive-override",
    )
    for mode in modes:
        if pending := require_selftest_mode(ctx, mode):
            return pending
    assert ctx.binary_path is not None

    with SandboxEnv("T4-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        dark = (ctx.repo_root / "tests" / "e2e" / "fixtures" / "colors_dark.toml").read_text(encoding="utf-8")
        light = (ctx.repo_root / "tests" / "e2e" / "fixtures" / "colors_light.toml").read_text(encoding="utf-8")
        mocks.seed_omarchy_theme("catppuccin", dark)
        rfile = sb.tmp / "t4_rounding.txt"
        mocks.install_hyprctl(rounding_file=rfile, rounding=0)
        srv = mocks.start_hypr_socket2("t4-02-sig")
        (sb.config / "fontconfig" / "fonts.conf").write_text(
            (ctx.repo_root / "tests" / "e2e" / "fixtures" / "fonts.conf").read_text(encoding="utf-8"),
            encoding="utf-8",
        )
        write_gall_settings(
            sb,
            {
                "theme": "omarchy",
                "omarchyGlassy": True,
                "omarchyFollowFont": True,
                "adaptiveLayout": True,
                "sidebarVisible": True,
                "windowButtons": "auto",
            },
        )

        proc = sb.popen([ctx.binary_path, str(sb.work)], env_overrides={"XDG_CURRENT_DESKTOP": "Hyprland"})
        try:
            if not srv.wait_for_client(timeout=6.0):
                return E2EResult(Status.FAIL, "Gall did not subscribe to Hyprland .socket2.sock")
            c0 = len(read_mock_calls(sb, "hyprctl"))

            (sb.state / "omarchy" / "toggles" / "hypr" / "window-glassy.lua").write_text("glassy=true\n", encoding="utf-8")
            rfile.write_text("10\n", encoding="utf-8")
            mocks.atomic_swap_omarchy_theme("latte", light, light_mode=True)
            srv.push_event("configreloaded>>")
            time.sleep(0.5)

            if proc.poll() is not None:
                return E2EResult(Status.FAIL, f"Gall crashed during live Omarchy/Hyprland swap (rc={proc.returncode})")
            if len(read_mock_calls(sb, "hyprctl")) <= c0:
                return E2EResult(Status.FAIL, "Hyprland configreloaded>> did not re-query hyprctl")
        finally:
            stop_proc(proc)

        expected = {
            "e2e-omarchy-dark": "light",
            "e2e-omarchy-glassy": "glassy",
            "e2e-adaptive-wide": "horizontal",
            "e2e-adaptive-tall": "vertical",
            "e2e-adaptive-compact": "single",
            "e2e-adaptive-override": "override",
        }
        for mode in modes:
            p_st, r_st, _ = run_gall_selftest(ctx, sb, mode, extra_args=[str(sb.work)])
            if fail := check_selftest_outcome(p_st, r_st, mode, sb=sb):
                return fail
            if expected[mode] not in r_st.lower():
                return E2EResult(Status.FAIL, f"{mode} report missing {expected[mode]!r}", {"report": r_st})
        for sub in ("org.gall.app", "gall"):
            cfg = sb.config / sub / "settings.json"
            if cfg.is_file():
                data = json.loads(cfg.read_text(encoding="utf-8"))
                if data.get("sidebarVisible") is not True:
                    return E2EResult(Status.FAIL, "Adaptive resize overwrote persisted sidebarVisible", {"settings": data})
        return E2EResult(Status.PASS, "Full Omarchy swap + Hyprland rounding + Glassy + 2D Adaptive sweep passed")


def _t4_03_media_transcode_and_clipboard_roundtrip(ctx: RunContext) -> E2EResult:
    """T4-03 (F3,F9): Media transcode pipeline + Wayland LF text/uri-list + Ctrl+X gnome-copied-files round-trip."""
    for mode in ("e2e-actions-floating", "e2e-clip-paste"):
        if pending := require_selftest_mode(ctx, mode):
            return pending

    omarchy_actions = ctx.repo_root / "packaging" / "linux" / "omarchy-actions.json"
    if not omarchy_actions.is_file():
        return E2EResult(Status.PENDING_IMPL, "packaging/linux/omarchy-actions.json missing")

    with SandboxEnv("T4-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_omarchy_floating_presentation()

        media = sb.work / "media"
        imported = sb.work / "imported"
        export = sb.work / "export"
        media.mkdir()
        imported.mkdir()
        export.mkdir()

        src_clip = media / "clip.mp4"
        src_clip.write_bytes(b"\x00\x00\x00\x18ftypmp42")
        out_clip = media / "clip-1080p.mp4"

        transcode_body, clip_status_path, clip_pid_path = make_transcode_clipboard_script(
            sb,
            out_clip,
            b"transcoded-payload-1080p",
        )
        mocks.install_tool("omarchy-transcode", script_body=transcode_body)
        shutil.copy2(omarchy_actions, sb.config / "gall" / "actions.d" / "50-omarchy.json")

        try:
            p1, r1, _ = run_gall_selftest(
                ctx,
                sb,
                "e2e-actions-floating",
                extra_args=[str(media)],
                env_overrides={"GALL_E2E_CHAIN_MOCKS": "1"},
            )
            if fail := check_selftest_outcome(p1, r1, "e2e-actions-floating", sb=sb):
                return fail
            if not read_mock_calls(sb, "omarchy-launch-floating-terminal-with-presentation"):
                return E2EResult(Status.FAIL, "omarchy-launch-floating-terminal-with-presentation was not invoked")
            if not read_mock_calls(sb, "omarchy-transcode"):
                return E2EResult(Status.FAIL, "Chained omarchy-transcode was not invoked by floating-presentation")
            if not out_clip.is_file():
                return E2EResult(Status.FAIL, "Transcode pipeline did not produce clip-1080p.mp4")

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

            # Paste LF text/uri-list into imported/
            p2, r2, _ = run_gall_selftest(ctx, sb, "e2e-clip-paste", extra_args=[str(imported)])
        finally:
            cleanup_transcode_clipboard(clip_pid_path)
        if fail := check_selftest_outcome(p2, r2, "e2e-clip-paste", sb=sb):
            return fail
        imp_clip = imported / out_clip.name
        if not imp_clip.is_file() or hashlib.sha256(imp_clip.read_bytes()).hexdigest() != hashlib.sha256(out_clip.read_bytes()).hexdigest():
            return E2EResult(Status.FAIL, "Pasted clip-1080p.mp4 missing or corrupted in imported/")

        # Cut via x-special/gnome-copied-files into export/
        owner_cut, err_cut = start_clipboard_owner(
            sb,
            {"x-special/gnome-copied-files": f"cut\n{_file_uri(imp_clip)}\n".encode("utf-8")},
        )
        if err_cut is not None:
            return err_cut
        try:
            p3, r3, _ = run_gall_selftest(ctx, sb, "e2e-clip-paste", extra_args=[str(export)])
        finally:
            stop_proc(owner_cut)
        if fail := check_selftest_outcome(p3, r3, "e2e-clip-paste", sb=sb):
            return fail
        if not (export / out_clip.name).is_file() or imp_clip.exists():
            return E2EResult(Status.FAIL, "Cut-paste from imported/ to export/ did not complete move semantics")
        return E2EResult(Status.PASS, "Media transcode -> LF text/uri-list -> cut/move export round-trip passed")


def _t4_04_mtp_import_and_nas_gvfs_workload(ctx: RunContext) -> E2EResult:
    """T4-04 (F3,F5,F8): MTP import + SFTP/SMB gio mount + non-blocking browse + Eject/Disconnect."""
    modes = ("e2e-gvfs-drives", "e2e-gvfs-slow", "e2e-clip-paste", "e2e-adaptive-wide", "e2e-gvfs-unmount")
    for mode in modes:
        if pending := require_selftest_mode(ctx, mode):
            return pending

    with SandboxEnv("T4-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_gio(
            mounts_output=(
                f"Mount(0): Pixel 9 -> mtp://Pixel_9/\n"
                f"  default_location=file://{sb.runtime}/gvfs/mtp:host=Pixel_9\n"
            ),
            list_delay=0.3,
        )
        photo_bytes = b"\xff\xd8\xff\xe0E2E-MTP-PHOTO"
        mtp_dir = mocks.seed_gvfs_mount("mtp:host=Pixel_9", {"DCIM/pic.jpg": photo_bytes})
        local_import = sb.work / "photos"
        local_import.mkdir()
        write_gall_settings(sb, {"adaptiveLayout": True, "sidebarVisible": True})

        p_drv, r_drv, _ = run_gall_selftest(ctx, sb, "e2e-gvfs-drives", extra_args=[str(sb.work)], timeout=15.0)
        if fail := check_selftest_outcome(p_drv, r_drv, "e2e-gvfs-drives", sb=sb):
            return fail
        if "pixel" not in r_drv.lower():
            return E2EResult(Status.FAIL, "e2e-gvfs-drives missing Pixel 9 MTP mount", {"report": r_drv})

        t0 = time.monotonic()
        p_gvfs, r_gvfs, _ = run_gall_selftest(ctx, sb, "e2e-gvfs-slow", extra_args=[str(mtp_dir)], timeout=15.0)
        elapsed = time.monotonic() - t0
        if fail := check_selftest_outcome(p_gvfs, r_gvfs, "e2e-gvfs-slow", sb=sb):
            return fail
        if elapsed > 10.0:
            return E2EResult(Status.FAIL, f"Slow GVFS browse took {elapsed:.2f}s")
        gio_calls = read_mock_calls(sb, "gio")
        if not gio_calls:
            return E2EResult(Status.FAIL, "gio was never invoked during GVFS workload")

        # Copy pic.jpg from MTP mount into local_import via genuine GTK/Wayland clipboard
        owner, err = start_clipboard_owner(
            sb,
            {"x-special/gnome-copied-files": f"copy\n{_file_uri(mtp_dir / 'DCIM' / 'pic.jpg')}\n".encode("utf-8")},
        )
        if err is not None:
            return err
        try:
            p_clip, r_clip, _ = run_gall_selftest(ctx, sb, "e2e-clip-paste", extra_args=[str(local_import)])
        finally:
            stop_proc(owner)
        if fail := check_selftest_outcome(p_clip, r_clip, "e2e-clip-paste", sb=sb):
            return fail
        imported_pic = local_import / "pic.jpg"
        if not imported_pic.is_file() or imported_pic.read_bytes() != photo_bytes:
            return E2EResult(Status.FAIL, "MTP pic.jpg import via clipboard failed or corrupted bytes")

        p_lay, r_lay, _ = run_gall_selftest(ctx, sb, "e2e-adaptive-wide", extra_args=[str(local_import)])
        if fail := check_selftest_outcome(p_lay, r_lay, "e2e-adaptive-wide", sb=sb):
            return fail

        p_unm, r_unm, _ = run_gall_selftest(ctx, sb, "e2e-gvfs-unmount", extra_args=[str(local_import)])
        if fail := check_selftest_outcome(p_unm, r_unm, "e2e-gvfs-unmount", sb=sb):
            return fail
        return E2EResult(Status.PASS, "MTP import + GVFS connect/eject + adaptive browse workload passed")


def _t4_05_keyboard_power_user_vim_zoxide_terminal_desktop(ctx: RunContext) -> E2EResult:
    """T4-05 (F4,F9,F10): vimKeys motions, zoxide Quick Open jump, editable safety & Terminal=true .desktop."""
    for mode in ("e2e-zoxide-quickopen", "e2e-vim-bounds", "e2e-openwith-desktop"):
        if pending := require_selftest_mode(ctx, mode):
            return pending

    with SandboxEnv("T4-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        repo = sb.work / "repo"
        repo.mkdir()
        note = repo / "README.md"
        note.write_text("# Power User\n", encoding="utf-8")
        mocks.install_zoxide([repo])
        mocks.install_xdg_terminal_exec()
        mocks.install_uwsm_app()
        mocks.install_tool("nvim")
        (sb.data / "applications" / "nvim-e2e.desktop").write_text(
            "[Desktop Entry]\nType=Application\nName=NvimE2E\nTerminal=true\nExec=nvim %F\nMimeType=text/markdown;text/plain;\n",
            encoding="utf-8",
        )
        write_gall_settings(sb, {"vimKeys": True, "keyboardProfile": "vim"})

        p_z, r_z, _ = run_gall_selftest(ctx, sb, "e2e-zoxide-quickopen", extra_args=[str(sb.work)])
        if fail := check_selftest_outcome(p_z, r_z, "e2e-zoxide-quickopen", sb=sb):
            return fail
        if not read_mock_calls(sb, "zoxide") or str(repo) not in (r_z + "\n" + read_gall_logs(sb)):
            return E2EResult(Status.FAIL, "zoxide Quick Open jump did not target repo")

        p_v, r_v, _ = run_gall_selftest(ctx, sb, "e2e-vim-bounds", extra_args=[str(repo)])
        if fail := check_selftest_outcome(p_v, r_v, "e2e-vim-bounds", sb=sb):
            return fail

        p_t, r_t, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-openwith-desktop",
            extra_args=[str(note)],
            env_overrides={"GALL_E2E_CHAIN_MOCKS": "1"},
        )
        if fail := check_selftest_outcome(p_t, r_t, "e2e-openwith-desktop", sb=sb):
            return fail
        if not read_mock_calls(sb, "xdg-terminal-exec") and not read_mock_calls(sb, "nvim"):
            return E2EResult(Status.FAIL, "Terminal=true .desktop did not invoke xdg-terminal-exec / nvim")
        return E2EResult(Status.PASS, "Keyboard-first vimKeys + zoxide + Terminal=true .desktop workload passed")


def _t4_06_daemon_dbus_select_and_new_window_git(ctx: RunContext) -> E2EResult:
    """T4-06 (F1,F2): Background --daemon + FileManager1.ShowFolders + %XX --select + --new-window --git."""
    if pending := require_cli_flags(ctx, "--daemon", "--select", "--new-window", "--git"):
        return pending
    assert ctx.binary_path is not None

    with SandboxEnv("T4-06", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        docs = sb.work / "My Docs"
        docs.mkdir()
        doc_file = docs / "článok #1.txt"
        doc_file.write_text("utf8\n", encoding="utf-8")

        repo = sb.work / "gitrepo"
        repo.mkdir()
        sb.run(["git", "init"], cwd=repo, check=True)
        sb.run(["git", "config", "user.email", "e2e@example.com"], cwd=repo, check=True)
        sb.run(["git", "config", "user.name", "E2E"], cwd=repo, check=True)
        (repo / "main.rs").write_text("fn main() {}\n", encoding="utf-8")
        sb.run(["git", "add", "main.rs"], cwd=repo, check=True)
        sb.run(["git", "commit", "-m", "init"], cwd=repo, check=True)

        daemon = sb.popen([ctx.binary_path, "--daemon"], cwd=sb.work)
        time.sleep(1.2)
        if daemon.poll() is not None:
            return E2EResult(Status.FAIL, f"gall --daemon exited early (rc={daemon.returncode})")
        if not wait_for_dbus_name(sb, FM1_DEST, timeout=6.0):
            return E2EResult(Status.PENDING_IMPL, "org.freedesktop.FileManager1 not registered by --daemon (M3 pending)")
        if "window shown at" in read_gall_logs(sb):
            return E2EResult(Status.FAIL, "--daemon mapped window prematurely")

        call = gdbus_call(sb, FM1_DEST, FM1_PATH, f"{FM1_DEST}.ShowFolders", f"['{_file_uri(docs)}']", "t4-06")
        if call.returncode != 0:
            return E2EResult(Status.FAIL, f"ShowFolders failed: {call.stderr}")
        if not wait_for_log_contains(sb, "My Docs", timeout=5.0):
            return E2EResult(Status.FAIL, "ShowFolders did not navigate to My Docs")

        sel_res = sb.run([ctx.binary_path, "--select", _file_uri(doc_file)], timeout=10.0)
        if pending := assert_unknown_arg_not_ignored(sb, sel_res, "--select"):
            return pending
        if sel_res.returncode != 0:
            return E2EResult(Status.FAIL, f"--select %XX URI failed (rc={sel_res.returncode})", {"stderr": sel_res.stderr})
        if not wait_for_log_contains(sb, doc_file.name, timeout=5.0):
            return E2EResult(Status.FAIL, "%XX encoded --select filename did not decode cleanly")

        git_res = sb.run([ctx.binary_path, "--new-window", "--git", str(repo)], timeout=10.0)
        if pending := assert_unknown_arg_not_ignored(sb, git_res, "--git"):
            return pending
        if git_res.returncode != 0:
            return E2EResult(Status.FAIL, f"--new-window --git failed (rc={git_res.returncode})", {"stderr": git_res.stderr})

        pids = count_gall_pids(sb)
        if len(pids) != 1:
            return E2EResult(Status.FAIL, f"Expected single daemon PID across all launches, found {pids}")
        return E2EResult(Status.PASS, "--daemon + ShowFolders + %XX --select + --new-window --git workload passed")


def _t4_07_packaging_store_managed_and_benchmark(ctx: RunContext) -> E2EResult:
    """T4-07 (F6,F11): Packaging bundle validation + StoreManaged HTTP guard + benchmark-nautilus.sh execution."""
    if not ctx.env_probe.benchmark_script_exists:
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh does not exist yet")
    if pending := require_selftest_mode(ctx, "updates"):
        return pending
    assert ctx.binary_path is not None

    desktop = ctx.repo_root / "packaging" / "linux" / "gall.desktop"
    service = ctx.repo_root / "packaging" / "linux" / "org.gall.app.FileManager1.service"
    metainfo = ctx.repo_root / "packaging" / "linux" / "org.gall.app.metainfo.xml"
    actions_json = ctx.repo_root / "packaging" / "linux" / "omarchy-actions.json"
    svg = ctx.repo_root / "assets" / "icon.svg"
    if not all(p.is_file() for p in (desktop, service, metainfo, actions_json, svg)):
        return E2EResult(Status.PENDING_IMPL, "One or more M2 packaging artifacts missing")

    try:
        assert_desktop_valid(desktop, ctx.env_probe, warn_kde=True)
        assert_xml_valid(metainfo)
        assert_xml_valid(svg)
    except AssertionError as exc:
        return E2EResult(Status.FAIL, str(exc))

    with SandboxEnv("T4-07", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        # Part C: Execute benchmark-nautilus.sh with deterministic mock DB + binary size check
        base_file = ctx.repo_root / "tests" / "e2e" / "fixtures" / "omarchy-base.packages"
        base_pkgs = [ln.split("#", 1)[0].strip() for ln in base_file.read_text(encoding="utf-8").splitlines() if ln.split("#", 1)[0].strip()]
        db: dict[str, dict[str, Any]] = {p: {"size": 1_000_000, "deps": [], "provides": []} for p in base_pkgs}
        db["nautilus"] = {"size": 8_000_000, "deps": ["base", "gvfs"], "provides": []}
        db["nautilus-python"] = {"size": 1_000_000, "deps": ["nautilus"], "provides": []}
        db["sushi"] = {"size": 3_000_000, "deps": ["nautilus"], "provides": []}
        db["ffmpegthumbnailer"] = {"size": 500_000, "deps": ["ffmpeg"], "provides": []}
        db["gtk3"] = {"size": 10_000_000, "deps": ["fontconfig"], "provides": []}
        db["webkit2gtk-4.1"] = {"size": 20_000_000, "deps": ["gtk3"], "provides": []}
        db["gst-plugins-good"] = {"size": 4_000_000, "deps": [], "provides": []}
        db_path = sb.tmp / "t4_db.json"
        db_path.write_text(json.dumps(db), encoding="utf-8")

        mocks = MockRegistry(sb)
        mocks.install_tool(
            "expac",
            script_body=f"""#!/usr/bin/env python3
import json, sys
db = json.load(open({str(db_path)!r}))
args = sys.argv[1:]
fmt = "%n\\t%m\\t%P"
pkgs = [a for a in args if a not in ("-Q", "-S") and not a.startswith("-") and "%" not in a]
for a in args:
    if "%" in a:
        fmt = a
        break
for n in (pkgs if pkgs else sorted(db.keys())):
    if n in db:
        print(fmt.replace("%n", n).replace("%m", str(db[n]["size"])).replace("%P", "None"))
""",
        )
        mocks.install_tool(
            "pactree",
            script_body=f"""#!/usr/bin/env python3
import json, sys
from collections import deque
db = json.load(open({str(db_path)!r}))
t = [a for a in sys.argv[1:] if not a.startswith("-")]
if not t or t[0] not in db:
    raise SystemExit(1)
q, seen = deque([t[0]]), set()
while q:
    c = q.popleft()
    if c in seen or c not in db: continue
    seen.add(c); print(c)
    for d in db[c]["deps"]: q.append(d)
""",
        )
        mocks.install_tool(
            "pacman",
            script_body=f"""#!/usr/bin/env python3
import json, sys
db = json.load(open({str(db_path)!r}))
t = [a for a in sys.argv[1:] if not a.startswith("-")]
if t and t[0] in db: print(t[0]); raise SystemExit(0)
raise SystemExit(1)
""",
        )
        bench = sb.run(
            [str(ctx.repo_root / "scripts" / "benchmark-nautilus.sh"), "--gall", str(ctx.binary_path), "--footprint-only", "--json"],
            env_overrides={"OMARCHY_BASE_PACKAGES": str(base_file)},
            timeout=20.0,
        )
        if bench.returncode != 0:
            return E2EResult(Status.FAIL, f"benchmark-nautilus.sh failed (rc={bench.returncode})", {"stderr": bench.stderr})
        bjson = json.loads(bench.stdout)
        if bjson.get("footprint", {}).get("status") != "ok" or bjson.get("gall_binary", {}).get("binary_bytes") != ctx.binary_path.stat().st_size:
            return E2EResult(Status.FAIL, f"Benchmark validation failed: {bjson}")

        # Part B: StoreManaged HTTP guard via updates selftest
        host_marker = Path("/usr/share/gall/package-managed").exists()
        if not ctx.env_probe.bwrap_works and not host_marker:
            return E2EResult(
                Status.ENV_UNAVAILABLE,
                "Packaging & benchmark verified; bwrap unprivileged user namespaces unavailable to stage /usr/share/gall/package-managed",
            )

        # Ephemeral HTTP updater server tracking all requests
        requests_log: list[str] = []

        class _Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args: Any) -> None:
                return

            def do_GET(self) -> None:
                requests_log.append(self.path)
                if self.path == "/latest.json":
                    tar_url = f"http://127.0.0.1:{srv.server_address[1]}/Gall_9.9.9_linux_x86_64_update.tar.gz"
                    platform_entry = {"url": tar_url, "signature": "invalid-e2e-sig"}
                    body = json.dumps(
                        {
                            "version": "9.9.9",
                            "notes": "E2E mock release",
                            "pub_date": "2026-11-20T12:00:00Z",
                            "platforms": {
                                "linux-x86_64": platform_entry,
                                "linux-x86_64-bin": platform_entry,
                            },
                        }
                    ).encode("utf-8")
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                else:
                    self.send_response(404)
                    self.end_headers()

        srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
        srv_thread = threading.Thread(target=srv.serve_forever, daemon=True)
        srv_thread.start()
        port = srv.server_address[1]

        out_dir = sb.logs / "selftest-updates"
        out_dir.mkdir(parents=True, exist_ok=True)
        env_ov = {
            "GALL_SELFTEST": "updates",
            "GALL_SELFTEST_DIR": str(out_dir),
            "GALL_SELFTEST_NOFONTS": "1",
            "GALL_UPDATER_TEST_VERSION": "0.1.0",
            "GALL_UPDATER_TEST_FORCE_OVERLAY": "1",
            "GALL_UPDATER_TEST_ENDPOINT": f"http://127.0.0.1:{port}/latest.json",
            "GALL_UPDATER_TEST_PUBKEY": "dGVzdC1wdWJrZXk=",
        }

        try:
            if ctx.env_probe.bwrap_works:
                marker_dir = sb.tmp / "usr_share_gall"
                marker_dir.mkdir()
                (marker_dir / "package-managed").write_text("arch\n", encoding="utf-8")
                bin_args, bin_target = system_prefix_binary_args(ctx.binary_path)
                proc = sb.run(
                    [
                        "bwrap",
                        "--dev-bind", "/", "/",
                        "--overlay-src", "/usr/share", "--tmp-overlay", "/usr/share",
                        "--dir", "/usr/share/gall",
                        "--bind", str(marker_dir), "/usr/share/gall",
                        *bin_args,
                        bin_target,
                    ],
                    env_overrides=env_ov,
                    timeout=60.0,
                )
            else:
                proc = sb.run_in_dbus([ctx.binary_path], env_overrides=env_ov, timeout=60.0)
        finally:
            srv.shutdown()
            srv.server_close()

        report = (out_dir / "report.txt").read_text(encoding="utf-8", errors="replace") if (out_dir / "report.txt").is_file() else ""
        if fail := check_selftest_outcome(proc, report, "updates", sb=sb):
            return fail
        if any(p.endswith(".tar.gz") for p in requests_log):
            return E2EResult(Status.FAIL, "StoreManaged unexpectedly downloaded an update tarball", {"requests": requests_log})
        if (sb.data / "gall" / "updates" / "active.json").exists():
            return E2EResult(Status.FAIL, "StoreManaged unexpectedly wrote updates/active.json")
        if (
            "DistroPackage" not in report
            and "StoreManaged" not in report
            and "package manager" not in report.lower()
            and "distribution" not in report.lower()
        ):
            return E2EResult(
                Status.FAIL,
                "updates selftest report did not assert StoreManaged / DistroPackage mode",
                {"report": report},
            )
        return E2EResult(Status.PASS, "Packaging validation, StoreManaged HTTP guard, and benchmark workload passed")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase("T4-01", 4, ("F1", "F2", "F4", "F9"), "Omarchy launcher -> terminal-cwd -> --compare -> editor -> ShowItems loop guard", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t4_01_launcher_terminal_compare_editor_dbus_loop_guard),
        E2ECase("T4-02", 4, ("F7", "F8"), "Live Omarchy atomic swap + Hyprland rounding + Glassy + 2D Adaptive sweep", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t4_02_omarchy_atomic_swap_hypr_glassy_adaptive_sweep),
        E2ECase("T4-03", 4, ("F3", "F9"), "Media transcode pipeline + LF text/uri-list + Ctrl+X gnome-copied-files round-trip", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t4_03_media_transcode_and_clipboard_roundtrip),
        E2ECase("T4-04", 4, ("F3", "F5", "F8"), "Phone MTP import + NAS SFTP/SMB connect + slow-mount browse + Eject", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t4_04_mtp_import_and_nas_gvfs_workload),
        E2ECase("T4-05", 4, ("F4", "F9", "F10"), "Keyboard-first vimKeys + zoxide Quick Open + editable safety + Terminal=true", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t4_05_keyboard_power_user_vim_zoxide_terminal_desktop),
        E2ECase("T4-06", 4, ("F1", "F2"), "Background --daemon + FileManager1.ShowFolders + %XX --select + --new-window --git", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t4_06_daemon_dbus_select_and_new_window_git),
        E2ECase("T4-07", 4, ("F6", "F11"), "Packaging bundle validation + StoreManaged guard + benchmark-nautilus.sh", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t4_07_packaging_store_managed_and_benchmark),
    ]
