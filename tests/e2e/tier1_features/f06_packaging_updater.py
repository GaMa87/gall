"""Tier 1 E2E tests for F6: Desktop metadata, SVG, AppStream, packaging, and StoreManaged updater."""

from __future__ import annotations

import configparser
import http.server
import json
import shutil
import struct
import threading
from pathlib import Path

from harness import (
    E2ECase,
    E2EResult,
    MockRegistry,
    RunContext,
    SandboxEnv,
    Status,
    assert_desktop_valid,
    assert_xml_valid,
)

from harness.assertions import system_prefix_binary_args

from . import (
    parse_selftest_failures,
    read_gall_log,
    require_selftest_mode,
    run_gall_selftest,
    selftest_mode_pending,
)


def _t1_f6_01_gall_desktop_validation(ctx: RunContext) -> E2EResult:
    desktop = ctx.repo_root / "packaging" / "linux" / "gall.desktop"
    if not desktop.is_file():
        return E2EResult(Status.PENDING_IMPL, "packaging/linux/gall.desktop missing")

    raw = desktop.read_text(encoding="utf-8")
    if "{{" in raw or "}}" in raw:
        return E2EResult(Status.FAIL, "gall.desktop still contains unrendered template placeholders")

    parser = assert_desktop_valid(desktop, ctx.env_probe)
    entry = parser["Desktop Entry"]

    expected = {
        "Type": "Application",
        "Name": "Gall",
        "Exec": "gall %U",
        "Icon": "gall",
        "StartupWMClass": "gall",
        "Terminal": "false",
    }
    for k, v in expected.items():
        if entry.get(k) != v:
            return E2EResult(Status.FAIL, f"gall.desktop [{k}] expected {v!r}, got {entry.get(k)!r}")

    # gall.desktop intentionally omits DBusActivatable (non-reverse-DNS desktop ID)
    if entry.get("DBusActivatable") == "true":
        return E2EResult(
            Status.FAIL,
            "gall.desktop must omit DBusActivatable=true (D-Bus activation uses org.gall.app.FileManager1.service)",
        )

    if "inode/directory;" not in entry.get("MimeType", ""):
        return E2EResult(Status.FAIL, f"MimeType missing inode/directory;: {entry.get('MimeType')!r}")
    if "new-window;" not in entry.get("Actions", ""):
        return E2EResult(Status.FAIL, f"Actions missing new-window;: {entry.get('Actions')!r}")
    if not parser.has_section("Desktop Action new-window"):
        return E2EResult(Status.FAIL, "Missing [Desktop Action new-window] section")
    action_exec = parser["Desktop Action new-window"].get("Exec", "")
    if "--new-window" not in action_exec:
        return E2EResult(Status.FAIL, f"new-window action Exec missing --new-window: {action_exec!r}")

    if (ctx.repo_root / "packaging" / "linux" / "Gall.desktop").exists():
        return E2EResult(Status.FAIL, "Stale uppercase packaging/linux/Gall.desktop exists")

    return E2EResult(Status.PASS, "gall.desktop passes desktop-file-validate with StartupWMClass=gall, Icon=gall, and new-window action")


def _t1_f6_02_filemanager1_service(ctx: RunContext) -> E2EResult:
    service = ctx.repo_root / "packaging" / "linux" / "org.gall.app.FileManager1.service"
    if not service.is_file():
        return E2EResult(Status.PENDING_IMPL, "packaging/linux/org.gall.app.FileManager1.service missing")

    parser = configparser.RawConfigParser(interpolation=None)
    parser.optionxform = str
    parser.read(service, encoding="utf-8")
    if not parser.has_section("D-BUS Service"):
        return E2EResult(Status.FAIL, "Missing [D-BUS Service] section in FileManager1.service")

    sec = parser["D-BUS Service"]
    if sec.get("Name") != "org.freedesktop.FileManager1":
        return E2EResult(Status.FAIL, f"Unexpected D-Bus Name: {sec.get('Name')!r}")
    exec_line = sec.get("Exec", "")
    if "gall" not in exec_line or ("--nowindow" not in exec_line and "--daemon" not in exec_line):
        return E2EResult(
            Status.FAIL,
            f"Invalid Exec line in FileManager1.service (expected gall with --nowindow/--daemon): {exec_line!r}",
        )

    return E2EResult(
        Status.PASS,
        f"org.gall.app.FileManager1.service valid (Name=org.freedesktop.FileManager1, Exec={exec_line})",
    )


def _resolve_svg_path(repo_root: Path) -> Path | None:
    direct = repo_root / "packaging" / "linux" / "gall.svg"
    if direct.is_file():
        return direct
    tauri_conf = repo_root / "src-tauri" / "tauri.conf.json"
    if tauri_conf.is_file():
        try:
            cfg = json.loads(tauri_conf.read_text(encoding="utf-8"))
            deb_files = cfg.get("bundle", {}).get("linux", {}).get("deb", {}).get("files", {})
            rel = deb_files.get("/usr/share/icons/hicolor/scalable/apps/gall.svg")
            if rel:
                cand = (tauri_conf.parent / rel).resolve()
                if cand.is_file():
                    return cand
        except Exception:
            pass
    return None


def _t1_f6_03_gall_svg_validation_and_raster(ctx: RunContext) -> E2EResult:
    svg_path = _resolve_svg_path(ctx.repo_root)
    if svg_path is None:
        return E2EResult(Status.PENDING_IMPL, "No gall.svg source icon found in packaging/linux or tauri.conf.json")

    root = assert_xml_valid(svg_path)
    if not root.tag.endswith("svg"):
        return E2EResult(Status.FAIL, f"Root XML tag is not <svg>: {root.tag}")
    if not root.attrib.get("viewBox", "").strip():
        return E2EResult(Status.FAIL, f"SVG missing non-empty viewBox attribute: {svg_path}")

    if not ctx.env_probe.rsvg_convert:
        return E2EResult(Status.ENV_UNAVAILABLE, "rsvg-convert not installed to rasterize SVG")

    with SandboxEnv("T1-F6-03", ctx.env_probe) as sb:
        out_png = sb.tmp / "gall-128.png"
        res = sb.run(["rsvg-convert", "-w", "128", "-h", "128", str(svg_path), "-o", str(out_png)])
        if res.returncode != 0 or not out_png.is_file():
            return E2EResult(Status.FAIL, f"rsvg-convert failed (rc={res.returncode})", {"stderr": res.stderr})
        data = out_png.read_bytes()
        if len(data) < 200 or data[:8] != b"\x89PNG\r\n\x1a\n":
            return E2EResult(Status.FAIL, "Rasterized output is not a valid PNG")
        w, h = struct.unpack(">II", data[16:24])
        if (w, h) != (128, 128):
            return E2EResult(Status.FAIL, f"Expected 128x128 PNG, got {w}x{h}")

    return E2EResult(Status.PASS, f"Validated {svg_path.name} XML/viewBox and rasterized 128x128 PNG cleanly")


def _t1_f6_04_appstream_and_packaging_consistency(ctx: RunContext) -> E2EResult:
    metainfo = ctx.repo_root / "packaging" / "linux" / "org.gall.app.metainfo.xml"
    if not metainfo.is_file():
        return E2EResult(Status.FAIL, "packaging/linux/org.gall.app.metainfo.xml missing")

    root = assert_xml_valid(metainfo)
    id_el = root.find("id")
    if id_el is None or id_el.text != "org.gall.app":
        return E2EResult(Status.FAIL, f"Expected <id>org.gall.app</id>, got {id_el.text if id_el is not None else None!r}")

    launchables = [el.text for el in root.findall("launchable") if el.attrib.get("type") == "desktop-id"]
    if launchables != ["gall.desktop"]:
        return E2EResult(Status.FAIL, f"Expected <launchable type='desktop-id'>gall.desktop</launchable>, got {launchables}")

    mediatypes = [el.text for el in root.findall("./provides/mediatype")]
    if "inode/directory" not in mediatypes:
        return E2EResult(Status.FAIL, f"AppStream <provides> missing inode/directory mediatype: {mediatypes}")

    dbus_provides = [el.text for el in root.findall("./provides/dbus")]
    if "org.freedesktop.FileManager1" not in dbus_provides:
        return E2EResult(Status.FAIL, f"AppStream <provides> missing FileManager1 D-Bus entry: {dbus_provides}")

    tauri_conf = ctx.repo_root / "src-tauri" / "tauri.conf.json"
    cfg = json.loads(tauri_conf.read_text(encoding="utf-8"))
    linux_bundle = cfg.get("bundle", {}).get("linux", {})
    for fmt in ("deb", "rpm"):
        sec = linux_bundle.get(fmt, {})
        if sec.get("desktopTemplate") != "../packaging/linux/gall.desktop":
            return E2EResult(Status.FAIL, f"tauri.conf.json {fmt}.desktopTemplate mismatch: {sec.get('desktopTemplate')!r}")
        files = sec.get("files", {})
        for dest in (
            "/usr/share/metainfo/org.gall.app.metainfo.xml",
            "/usr/share/icons/hicolor/scalable/apps/gall.svg",
            "/usr/share/dbus-1/services/org.gall.app.FileManager1.service",
            "/usr/share/gall/actions.d/50-omarchy.json",
        ):
            rel = files.get(dest)
            if not rel:
                return E2EResult(Status.FAIL, f"tauri.conf.json {fmt}.files missing {dest}")
            resolved = (tauri_conf.parent / rel).resolve()
            if not resolved.is_file():
                return E2EResult(Status.FAIL, f"tauri.conf.json {fmt}.files target missing on disk: {resolved}")

    actions_pkg = ctx.repo_root / "packaging" / "linux" / "omarchy-actions.json"
    if not actions_pkg.is_file():
        return E2EResult(Status.PENDING_IMPL, "packaging/linux/omarchy-actions.json missing")
    actions_data = json.loads(actions_pkg.read_text(encoding="utf-8"))
    if not isinstance(actions_data, list) or len(actions_data) < 5:
        return E2EResult(Status.FAIL, "packaging/linux/omarchy-actions.json invalid or empty")
    for act in actions_data:
        if "labelMulti" in act:
            return E2EResult(Status.FAIL, f"omarchy-actions.json uses deprecated labelMulti in {act.get('id')}")

    for pkg_file in (
        ctx.repo_root / "packaging" / "aur" / "gall-bin" / "PKGBUILD",
        ctx.repo_root / "packaging" / "copr" / "gall.spec",
    ):
        text = pkg_file.read_text(encoding="utf-8")
        for needle in ("gall.desktop", "gall.svg", "org.gall.app.FileManager1.service", "50-omarchy.json"):
            if needle not in text:
                return E2EResult(Status.FAIL, f"{pkg_file.name} missing reference to {needle}")

    return E2EResult(Status.PASS, "AppStream, tauri.conf.json, omarchy-actions.json, PKGBUILD, and gall.spec aligned")


def _t1_f6_05_store_managed_updater(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "updates"):
        return pending
    host_marker = Path("/usr/share/gall/package-managed").exists()
    if not ctx.env_probe.bwrap_works and not host_marker:
        return E2EResult(
            Status.ENV_UNAVAILABLE,
            "bwrap unprivileged user namespaces unavailable to stage /usr/share/gall/package-managed",
        )

    with SandboxEnv("T1-F6-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        requests_seen: list[str] = []

        class _Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self) -> None:  # noqa: N802
                requests_seen.append(self.path)
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

            def log_message(self, format: str, *args: object) -> None:
                pass

        srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Handler)
        thread = threading.Thread(target=srv.serve_forever, daemon=True)
        thread.start()
        try:
            endpoint = f"http://127.0.0.1:{srv.server_address[1]}/latest.json"
            env_ov = {
                "GALL_UPDATER_TEST_VERSION": "0.1.0",
                "GALL_UPDATER_TEST_FORCE_OVERLAY": "1",
                "GALL_UPDATER_TEST_ENDPOINT": endpoint,
            }

            if ctx.env_probe.bwrap_works:
                assert ctx.binary_path is not None
                marker = sb.tmp / "package-managed"
                marker.write_text("distro\n", encoding="utf-8")
                out_dir = sb.tmp / "selftest-updates"
                out_dir.mkdir(parents=True, exist_ok=True)
                env_ov.update(
                    {
                        "GALL_SELFTEST": "updates",
                        "GALL_SELFTEST_DIR": str(out_dir),
                        "GALL_SELFTEST_CAPTURE": "webview",
                    }
                )
                bin_args, bin_target = system_prefix_binary_args(ctx.binary_path)
                bwrap_cmd = [
                    "bwrap",
                    "--ro-bind", "/", "/",
                    "--dev-bind", "/dev", "/dev",
                    "--proc", "/proc",
                    "--bind", "/tmp", "/tmp",
                    "--overlay-src", "/usr/share", "--tmp-overlay", "/usr/share",
                    "--dir", "/usr/share/gall",
                    "--ro-bind", str(marker), "/usr/share/gall/package-managed",
                    *bin_args,
                    bin_target,
                    str(sb.work),
                ]
                res = sb.run_in_dbus(bwrap_cmd, env_overrides=env_ov, timeout=45.0)
                rc, err = res.returncode, res.stderr or ""
                report_path = out_dir / "report.txt"
                report = report_path.read_text(encoding="utf-8", errors="replace") if report_path.is_file() else ""
            else:
                rc, out, err, report = run_gall_selftest(
                    ctx,
                    sb,
                    "updates",
                    cli_args=[sb.work],
                    env_overrides=env_ov,
                )
        finally:
            srv.shutdown()
            srv.server_close()

        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--updates", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "updates selftest mode unavailable")
        if rc != 0:
            return E2EResult(Status.FAIL, f"updates selftest failed (rc={rc})", {"stderr": err, "log": log[-1000:]})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        tarball_gets = [p for p in requests_seen if p.endswith(".tar.gz")]
        if tarball_gets:
            return E2EResult(
                Status.FAIL,
                f"Updater downloaded overlay tarball despite package-managed guard: {tarball_gets}",
            )
        active_json = sb.data / "gall" / "updates" / "active.json"
        if active_json.exists():
            return E2EResult(Status.FAIL, "Updater created active.json despite package-managed guard")

        combined = (report + "\n" + log).lower()
        if (
            "distribution" not in combined
            and "package manager" not in combined
            and "storemanaged" not in combined
            and "distropackage" not in combined
        ):
            return E2EResult(Status.FAIL, "Updater did not report StoreManaged / DistroPackage mode", {"report": report})
        return E2EResult(Status.PASS, "Updater reports StoreManaged / DistroPackage mode with 0 overlay downloads")


def _t1_f6_06_update_desktop_database_mime(ctx: RunContext) -> E2EResult:
    desktop = ctx.repo_root / "packaging" / "linux" / "gall.desktop"
    if not desktop.is_file():
        return E2EResult(Status.PENDING_IMPL, "packaging/linux/gall.desktop missing")
    if not ctx.env_probe.update_desktop_database:
        return E2EResult(Status.ENV_UNAVAILABLE, "update-desktop-database not installed")

    with SandboxEnv("T1-F6-06", ctx.env_probe) as sb:
        # GLib GDesktopAppInfo requires Exec=gall to exist on PATH when resolving gall.desktop
        MockRegistry(sb).install_tool("gall")
        apps_dir = sb.data / "applications"
        apps_dir.mkdir(parents=True, exist_ok=True)
        shutil.copy2(desktop, apps_dir / "gall.desktop")
        (sb.config / "mimeapps.list").write_text(
            "[Default Applications]\ninode/directory=gall.desktop;\n",
            encoding="utf-8",
        )

        res = sb.run(["update-desktop-database", str(apps_dir)])
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"update-desktop-database failed (rc={res.returncode})", {"stderr": res.stderr})

        cache = apps_dir / "mimeinfo.cache"
        if not cache.is_file():
            return E2EResult(Status.FAIL, "update-desktop-database did not generate mimeinfo.cache")
        cache_text = cache.read_text(encoding="utf-8")
        if "inode/directory=gall.desktop;" not in cache_text:
            return E2EResult(Status.FAIL, "mimeinfo.cache missing inode/directory=gall.desktop;", {"cache": cache_text})

        if ctx.env_probe.gio:
            mime_res = sb.run(["gio", "mime", "inode/directory"])
            if mime_res.returncode != 0 or "Default application for" not in (mime_res.stdout or "") or "gall.desktop" not in (mime_res.stdout or "").splitlines()[0]:
                return E2EResult(
                    Status.FAIL,
                    f"gio mime inode/directory did not resolve gall.desktop as default: {mime_res.stdout!r}",
                    {"stderr": mime_res.stderr},
                )

    return E2EResult(Status.PASS, "update-desktop-database and gio mime registered inode/directory=gall.desktop cleanly")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase(
            id="T1-F6-01",
            tier=1,
            features=("F6",),
            title="gall.desktop validates cleanly with StartupWMClass=gall, Icon=gall, and new-window action",
            requires_binary=False,
            requires_gui=False,
            requires_dbus=False,
            fn=_t1_f6_01_gall_desktop_validation,
        ),
        E2ECase(
            id="T1-F6-02",
            tier=1,
            features=("F6",),
            title="org.gall.app.FileManager1.service defines valid D-Bus activation entry",
            requires_binary=False,
            requires_gui=False,
            requires_dbus=False,
            fn=_t1_f6_02_filemanager1_service,
        ),
        E2ECase(
            id="T1-F6-03",
            tier=1,
            features=("F6",),
            title="Scalable SVG icon validates XML/viewBox and rasterizes to 128x128 PNG",
            requires_binary=False,
            requires_gui=False,
            requires_dbus=False,
            fn=_t1_f6_03_gall_svg_validation_and_raster,
        ),
        E2ECase(
            id="T1-F6-04",
            tier=1,
            features=("F6",),
            title="AppStream metainfo, tauri.conf.json, omarchy-actions.json, PKGBUILD, and gall.spec align",
            requires_binary=False,
            requires_gui=False,
            requires_dbus=False,
            fn=_t1_f6_04_appstream_and_packaging_consistency,
        ),
        E2ECase(
            id="T1-F6-05",
            tier=1,
            features=("F6",),
            title="Updater reports StoreManaged / DistroPackage mode when package-managed",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f6_05_store_managed_updater,
        ),
        E2ECase(
            id="T1-F6-06",
            tier=1,
            features=("F6",),
            title="update-desktop-database registers inode/directory=gall.desktop;",
            requires_binary=False,
            requires_gui=False,
            requires_dbus=False,
            fn=_t1_f6_06_update_desktop_database_mime,
        ),
    ]
