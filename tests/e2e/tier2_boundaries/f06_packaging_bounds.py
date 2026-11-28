"""Tier 2 boundary & negative E2E cases for Feature F6 (Desktop metadata & StoreManaged)."""

from __future__ import annotations

import configparser
import json
import re
from pathlib import Path

from harness.assertions import assert_desktop_valid, assert_xml_valid, system_prefix_binary_args
from harness.sandbox import SandboxEnv
from harness.types import E2ECase, E2EResult, RunContext, Status

from . import (
    check_m2_desktop_updated,
    check_selftest_outcome,
    require_selftest_mode,
    run_gall_selftest,
)


def _check_svg_safety(svg_path: Path) -> list[str]:
    issues: list[str] = []
    text = svg_path.read_text(encoding="utf-8", errors="replace")
    lower = text.lower()
    if "<script" in lower:
        issues.append("contains forbidden <script> element")
    if re.search(r"""\bon[a-z]+\s*=""", lower):
        issues.append("contains forbidden inline event handler attribute")
    if re.search(r"""(?:href|xlink:href)\s*=\s*["']\s*https?://""", lower):
        issues.append("contains forbidden external http(s) href")
    root = assert_xml_valid(svg_path)
    tag_name = root.tag.split("}", 1)[-1]
    if tag_name != "svg":
        issues.append(f"root element is <{tag_name}>, expected <svg>")
    if not root.attrib.get("viewBox"):
        issues.append("missing viewBox attribute on <svg>")
    return issues


def _t2_f6_01_strict_desktop_spec_warn_kde(ctx: RunContext) -> E2EResult:
    """Strict desktop-file-validate --warn-kde on gall.desktop plus negative validation control."""
    ready, reason = check_m2_desktop_updated(ctx.repo_root)
    if not ready:
        return E2EResult(Status.PENDING_IMPL, reason)
    if not ctx.env_probe.desktop_file_validate:
        return E2EResult(Status.ENV_UNAVAILABLE, "desktop-file-validate not installed")

    desktop_path = ctx.repo_root / "packaging" / "linux" / "gall.desktop"
    try:
        parser = assert_desktop_valid(desktop_path, ctx.env_probe, warn_kde=True)
    except AssertionError as exc:
        return E2EResult(Status.FAIL, str(exc))

    entry = parser["Desktop Entry"]
    if "Encoding" in entry:
        return E2EResult(Status.FAIL, "gall.desktop contains deprecated Encoding key")
    if entry.get("DBusActivatable", "").lower() == "true":
        return E2EResult(Status.FAIL, "gall.desktop must not set DBusActivatable=true without org.freedesktop.Application")
    if entry.get("StartupWMClass") != "gall":
        return E2EResult(Status.FAIL, f"Expected StartupWMClass=gall, got {entry.get('StartupWMClass')!r}")
    if entry.get("Icon") != "gall":
        return E2EResult(Status.FAIL, f"Expected Icon=gall, got {entry.get('Icon')!r}")
    if "new-window" not in entry.get("Actions", ""):
        return E2EResult(Status.FAIL, f"Expected Actions=new-window;, got {entry.get('Actions')!r}")
    if "FileManager" not in entry.get("Categories", ""):
        return E2EResult(Status.FAIL, f"Missing FileManager in Categories: {entry.get('Categories')!r}")
    if not parser.has_section("Desktop Action new-window"):
        return E2EResult(Status.FAIL, "Missing [Desktop Action new-window] section")
    action_exec = parser["Desktop Action new-window"].get("Exec", "")
    if "--new-window" not in action_exec:
        return E2EResult(Status.FAIL, f"[Desktop Action new-window] Exec missing --new-window: {action_exec!r}")

    with SandboxEnv("T2-F6-01-neg", ctx.env_probe) as sb:
        broken = sb.tmp / "broken.desktop"
        broken.write_text("[Desktop Entry]\nName=Broken\nEncoding=UTF-8\n", encoding="utf-8")
        try:
            assert_desktop_valid(broken, ctx.env_probe, warn_kde=True)
            return E2EResult(Status.FAIL, "Negative control broken.desktop unexpectedly passed validation")
        except AssertionError:
            pass

    return E2EResult(Status.PASS, "gall.desktop passed desktop-file-validate --warn-kde and strict key checks")


def _t2_f6_02_no_stale_uppercase_desktop_refs(ctx: RunContext) -> E2EResult:
    """Ensure metainfo.xml, tauri.conf.json, PKGBUILD, and gall.spec align on lowercase gall.desktop."""
    ready, reason = check_m2_desktop_updated(ctx.repo_root)
    if not ready:
        return E2EResult(Status.PENDING_IMPL, reason)

    metainfo = ctx.repo_root / "packaging" / "linux" / "org.gall.app.metainfo.xml"
    try:
        root = assert_xml_valid(metainfo)
    except AssertionError as exc:
        return E2EResult(Status.FAIL, str(exc))

    launchables = [el.text.strip() for el in root.findall(".//launchable") if el.text]
    if "gall.desktop" not in launchables or any("Gall.desktop" in item for item in launchables):
        return E2EResult(Status.FAIL, f"metainfo.xml launchable must be gall.desktop, got {launchables}")

    tauri_conf = ctx.repo_root / "src-tauri" / "tauri.conf.json"
    if not tauri_conf.is_file():
        return E2EResult(Status.FAIL, "Missing tauri.conf.json")
    conf_raw = tauri_conf.read_text(encoding="utf-8")
    if "Gall.desktop" in conf_raw:
        return E2EResult(Status.FAIL, "Stale Gall.desktop reference found in tauri.conf.json")
    conf = json.loads(conf_raw)
    linux_bundle = conf.get("bundle", {}).get("linux", {})
    for pkg_fmt in ("deb", "rpm"):
        tmpl = linux_bundle.get(pkg_fmt, {}).get("desktopTemplate", "")
        if not tmpl.endswith("/gall.desktop") and tmpl != "gall.desktop":
            return E2EResult(Status.FAIL, f"tauri.conf.json bundle.linux.{pkg_fmt}.desktopTemplate={tmpl!r} does not target gall.desktop")

    pkgbuild = ctx.repo_root / "packaging" / "aur" / "gall-bin" / "PKGBUILD"
    pkg_text = pkgbuild.read_text(encoding="utf-8")
    if "/usr/share/applications/gall.desktop" not in pkg_text:
        return E2EResult(Status.FAIL, "PKGBUILD does not reference /usr/share/applications/gall.desktop")

    spec_file = ctx.repo_root / "packaging" / "copr" / "gall.spec"
    spec_text = spec_file.read_text(encoding="utf-8")
    files_idx = spec_text.find("%files")
    if files_idx == -1 or "%{_datadir}/applications/gall.desktop" not in spec_text[files_idx:]:
        return E2EResult(Status.FAIL, "gall.spec %files does not list %{_datadir}/applications/gall.desktop")
    if "%{_datadir}/applications/Gall.desktop" in spec_text[files_idx:]:
        return E2EResult(Status.FAIL, "gall.spec %files contains stale Gall.desktop")

    service = ctx.repo_root / "packaging" / "linux" / "org.gall.app.FileManager1.service"
    srv_parser = configparser.RawConfigParser(interpolation=None)
    srv_parser.optionxform = str
    srv_parser.read(service, encoding="utf-8")
    if not srv_parser.has_section("D-BUS Service"):
        return E2EResult(Status.FAIL, "FileManager1.service missing [D-BUS Service]")
    if srv_parser["D-BUS Service"].get("Name") != "org.freedesktop.FileManager1":
        return E2EResult(Status.FAIL, "FileManager1.service Name mismatch")
    if "gall" not in srv_parser["D-BUS Service"].get("Exec", ""):
        return E2EResult(Status.FAIL, "FileManager1.service Exec does not invoke gall")

    return E2EResult(Status.PASS, "All Linux packaging configs consistently align on gall.desktop and org.gall.app")


def _t2_f6_03_package_managed_overrides_overlay(ctx: RunContext) -> E2EResult:
    """/usr/share/gall/package-managed marker must force StoreManaged even with test overlay env vars."""
    if pending := require_selftest_mode(ctx, "updates"):
        return pending
    if not ctx.env_probe.bwrap_works:
        return E2EResult(
            Status.ENV_UNAVAILABLE,
            "bwrap unprivileged user namespaces unavailable to stage /usr/share/gall/package-managed",
        )
    assert ctx.binary_path is not None
    with SandboxEnv("T2-F6-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        marker_dir = sb.tmp / "usr_share_gall"
        marker_dir.mkdir(parents=True)
        (marker_dir / "package-managed").write_text("arch\n", encoding="utf-8")

        out_dir = sb.logs / "selftest-updates"
        out_dir.mkdir(parents=True)
        bin_args, bin_target = system_prefix_binary_args(ctx.binary_path)
        cmd = [
            "bwrap",
            "--dev-bind", "/", "/",
            "--overlay-src", "/usr/share", "--tmp-overlay", "/usr/share",
            "--dir", "/usr/share/gall",
            "--bind", str(marker_dir), "/usr/share/gall",
            *bin_args,
            bin_target,
        ]
        proc = sb.run(
            cmd,
            env_overrides={
                "GALL_SELFTEST": "updates",
                "GALL_SELFTEST_DIR": str(out_dir),
                "GALL_SELFTEST_NOFONTS": "1",
                "GALL_UPDATER_TEST_FORCE_OVERLAY": "1",
                "GALL_UPDATER_TEST_PUBKEY": "dGVzdC1wdWJrZXk=",
                "GALL_UPDATER_TEST_ENDPOINT": "http://127.0.0.1:9/latest.json",
            },
            timeout=60.0,
        )
        report = (out_dir / "report.txt").read_text(encoding="utf-8", errors="replace") if (out_dir / "report.txt").is_file() else ""
        if fail := check_selftest_outcome(proc, report, "updates", sb=sb):
            return fail
        if "DistroPackage" not in report and "package manager" not in report.lower() and "StoreManaged" not in report:
            return E2EResult(Status.FAIL, "StoreManaged/DistroPackage did not override test overlay env vars", {"report": report})
        return E2EResult(Status.PASS, "package-managed marker forced StoreManaged over test overlay flags")


def _t2_f6_04_non_managed_regression(ctx: RunContext) -> E2EResult:
    """Standard debug build without package-managed marker must not falsely report StoreManaged."""
    if pending := require_selftest_mode(ctx, "updates"):
        return pending
    if Path("/usr/share/gall/package-managed").exists():
        return E2EResult(Status.ENV_UNAVAILABLE, "Host already has /usr/share/gall/package-managed installed")
    with SandboxEnv("T2-F6-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        proc, report, _ = run_gall_selftest(ctx, sb, "updates")
        if fail := check_selftest_outcome(proc, report, "updates", sb=sb):
            return fail
        if "DistroPackage" in report or "managed by your distribution" in report.lower():
            return E2EResult(Status.FAIL, "Standard non-managed build falsely reported DistroPackage", {"report": report})
        return E2EResult(Status.PASS, "Standard build does not falsely claim DistroPackage/StoreManaged")


def _t2_f6_05_svg_safety_and_multi_size_raster(ctx: RunContext) -> E2EResult:
    """Validate gall SVG safety (no script/remote href) and rasterize at 16x16 & 512x512."""
    svg_path = ctx.repo_root / "packaging" / "linux" / "gall.svg"
    if not svg_path.is_file():
        tauri_conf = ctx.repo_root / "src-tauri" / "tauri.conf.json"
        if tauri_conf.is_file():
            conf = json.loads(tauri_conf.read_text(encoding="utf-8"))
            files_map = conf.get("bundle", {}).get("linux", {}).get("deb", {}).get("files", {})
            mapped = files_map.get("/usr/share/icons/hicolor/scalable/apps/gall.svg")
            if mapped:
                cand = (tauri_conf.parent / mapped).resolve()
                if cand.is_file():
                    svg_path = cand

    if not svg_path.is_file():
        return E2EResult(Status.PENDING_IMPL, "Neither packaging/linux/gall.svg nor mapped icon.svg exists yet")

    issues = _check_svg_safety(svg_path)
    if issues:
        return E2EResult(Status.FAIL, f"SVG safety/schema check failed for {svg_path}: {'; '.join(issues)}")

    if not ctx.env_probe.rsvg_convert:
        return E2EResult(Status.ENV_UNAVAILABLE, "rsvg-convert is not installed")

    with SandboxEnv("T2-F6-05", ctx.env_probe) as sb:
        bad_svg = sb.tmp / "evil.svg"
        bad_svg.write_text('<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><script>alert(1)</script></svg>', encoding="utf-8")
        if not _check_svg_safety(bad_svg):
            return E2EResult(Status.FAIL, "SVG safety checker failed to reject <script> tag")

        for size in (16, 512):
            out_png = sb.tmp / f"gall-{size}.png"
            res = sb.run(["rsvg-convert", "-w", str(size), "-h", str(size), str(svg_path), "-o", str(out_png)])
            if res.returncode != 0 or not out_png.is_file():
                return E2EResult(Status.FAIL, f"rsvg-convert failed at {size}x{size}: {res.stderr}")
            data = out_png.read_bytes()
            if not data.startswith(b"\x89PNG\r\n\x1a\n") or len(data) < 64:
                return E2EResult(Status.FAIL, f"Invalid PNG produced by rsvg-convert at {size}x{size}")

    return E2EResult(Status.PASS, f"{svg_path.name} passed XML/safety checks and rasterized cleanly at 16x16 and 512x512")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase("T2-F6-01", 2, ("F6",), "Strict desktop-file-validate --warn-kde on gall.desktop", requires_binary=False, requires_gui=False, fn=_t2_f6_01_strict_desktop_spec_warn_kde),
        E2ECase("T2-F6-02", 2, ("F6",), "Zero stale Gall.desktop refs across metainfo, tauri.conf, PKGBUILD, spec", requires_binary=False, requires_gui=False, fn=_t2_f6_02_no_stale_uppercase_desktop_refs),
        E2ECase("T2-F6-03", 2, ("F6",), "package-managed marker overrides test overlay updater config", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f6_03_package_managed_overrides_overlay),
        E2ECase("T2-F6-04", 2, ("F6",), "Non-managed build does not falsely report StoreManaged", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f6_04_non_managed_regression),
        E2ECase("T2-F6-05", 2, ("F6",), "gall.svg safety (no script/remote href) and 16/512 PNG rasterization", requires_binary=False, requires_gui=False, fn=_t2_f6_05_svg_safety_and_multi_size_raster),
    ]
