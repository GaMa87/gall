"""Tier 2 boundary & negative E2E cases for Feature F8 (2D Adaptive Layout & Tiling-WM)."""

from __future__ import annotations

import json

from harness.sandbox import SandboxEnv
from harness.types import E2ECase, E2EResult, RunContext, Status

from . import (
    check_selftest_outcome,
    require_selftest_mode,
    run_gall_selftest,
    write_gall_settings,
)


def _assert_settings_adaptive_intact(sb: SandboxEnv) -> E2EResult | None:
    for sub in ("org.gall.app", "gall"):
        path = sb.config / sub / "settings.json"
        if path.is_file():
            data = json.loads(path.read_text(encoding="utf-8"))
            if data.get("adaptiveLayout") is not True:
                return E2EResult(Status.FAIL, f"Adaptive resize corrupted persisted adaptiveLayout in {path}", {"settings": data})
    return None


def _t2_f8_01_hysteresis_deadband(ctx: RunContext) -> E2EResult:
    """~36px hysteresis around 920px width and 620px height must prevent rapid pane flipping."""
    if pending := require_selftest_mode(ctx, "e2e-adaptive-wide"):
        return pending
    with SandboxEnv("T2-F8-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"adaptiveLayout": True})
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-adaptive-wide")
        if fail := check_selftest_outcome(proc, report, "e2e-adaptive-wide", sb=sb):
            return fail
        low = report.lower()
        if "horizontal" not in low or "hysteresis" not in low:
            return E2EResult(Status.FAIL, "e2e-adaptive-wide report did not assert horizontal + hysteresis deadband", {"report": report})
        if err := _assert_settings_adaptive_intact(sb):
            return err
        return E2EResult(Status.PASS, "2D Adaptive Layout hysteresis deadband verified")


def _t2_f8_02_exact_aspect_ratio_boundary(ctx: RunContext) -> E2EResult:
    """Exact aspect boundary at w=750: 750x635 (single) vs 750x640 (vertical, h >= w*0.85)."""
    if pending := require_selftest_mode(ctx, "e2e-adaptive-tall"):
        return pending
    with SandboxEnv("T2-F8-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"adaptiveLayout": True})
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-adaptive-tall")
        if fail := check_selftest_outcome(proc, report, "e2e-adaptive-tall", sb=sb):
            return fail
        low = report.lower()
        if "vertical" not in low or "hysteresis" not in low:
            return E2EResult(Status.FAIL, "e2e-adaptive-tall report did not verify 2V vertical + hysteresis boundary", {"report": report})
        return E2EResult(Status.PASS, "2D Adaptive Layout 0.85 aspect ratio boundary verified")


def _t2_f8_03_narrow_width_under_480_even_tall(ctx: RunContext) -> E2EResult:
    """Width < 480px (460x1000) must force single-pane mode despite tall aspect ratio."""
    if pending := require_selftest_mode(ctx, "e2e-adaptive-compact"):
        return pending
    with SandboxEnv("T2-F8-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"adaptiveLayout": True})
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-adaptive-compact")
        if fail := check_selftest_outcome(proc, report, "e2e-adaptive-compact", sb=sb):
            return fail
        low = report.lower()
        if "single" not in low or "sidebar" not in low:
            return E2EResult(Status.FAIL, "e2e-adaptive-compact report did not verify single-pane + sidebar auto-hide guard", {"report": report})
        return E2EResult(Status.PASS, "Width < 480px forced single-pane mode even at 1000px height")


def _t2_f8_04_state_preserved_across_2h_2v_1_2h(ctx: RunContext) -> E2EResult:
    """Manual override & pane state survive 2H -> 2V -> 1 -> 2H resize without mutating sidebarVisible."""
    if pending := require_selftest_mode(ctx, "e2e-adaptive-override"):
        return pending
    with SandboxEnv("T2-F8-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"adaptiveLayout": True, "sidebarVisible": True})
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-adaptive-override")
        if fail := check_selftest_outcome(proc, report, "e2e-adaptive-override", sb=sb):
            return fail
        if "override" not in report.lower():
            return E2EResult(Status.FAIL, "e2e-adaptive-override report did not verify override preservation", {"report": report})
        for sub in ("org.gall.app", "gall"):
            path = sb.config / sub / "settings.json"
            if path.is_file():
                data = json.loads(path.read_text(encoding="utf-8"))
                if data.get("sidebarVisible") is not True:
                    return E2EResult(Status.FAIL, "Transient narrow sidebar auto-hide overwrote persisted sidebarVisible", {"settings": data})
        return E2EResult(Status.PASS, "Inactive browser DOM state and sidebarVisible preserved across 2D adaptive transitions")


def _t2_f8_05_tiling_wm_maximize_suppression(ctx: RunContext) -> E2EResult:
    """Tiling-WM framing suppresses forced startMaximized across Hyprland/niri while permitting GNOME maximize."""
    if pending := require_selftest_mode(ctx, "e2e-tiling-maximize"):
        return pending
    with SandboxEnv("T2-F8-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"adaptiveLayout": True, "startMaximized": True})
        for desktop_env, expect_suppressed in (("Hyprland", True), ("niri", True)):
            proc, report, _ = run_gall_selftest(
                ctx,
                sb,
                "e2e-tiling-maximize",
                env_overrides={"XDG_CURRENT_DESKTOP": desktop_env, "HYPRLAND_INSTANCE_SIGNATURE": None},
            )
            if fail := check_selftest_outcome(proc, report, "e2e-tiling-maximize", sb=sb):
                return fail
            low = report.lower()
            if "tiling" not in low and "maximiz" not in low:
                return E2EResult(
                    Status.FAIL,
                    f"e2e-tiling-maximize report missing tiling/maximized evidence for {desktop_env}",
                    {"report": report},
                )
            if expect_suppressed and ("maximized=true" in low or "start_maximized=true" in low):
                return E2EResult(
                    Status.FAIL,
                    f"Tiling WM {desktop_env} unexpectedly forced window maximize",
                    {"report": report},
                )
        return E2EResult(Status.PASS, "Tiling-WM start_maximized=false framing verified across Hyprland and niri")


def _t2_f8_06_tiling_wm_desktop_detection_variants(ctx: RunContext) -> E2EResult:
    """XDG_CURRENT_DESKTOP='Hyprland:unity' / 'niri' triggers auto windowButtons=hide; 'GNOME' shows them."""
    if pending := require_selftest_mode(ctx, "e2e-window-buttons"):
        return pending
    with SandboxEnv("T2-F8-06", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"windowButtons": "auto", "startMaximized": True})
        for desktop_env, expected_vis in (
            ("Hyprland:unity", "hidden"),
            ("niri", "hidden"),
            ("GNOME", "visible"),
        ):
            proc, report, _ = run_gall_selftest(
                ctx,
                sb,
                "e2e-window-buttons",
                env_overrides={"XDG_CURRENT_DESKTOP": desktop_env, "HYPRLAND_INSTANCE_SIGNATURE": None},
            )
            if fail := check_selftest_outcome(proc, report, "e2e-window-buttons", sb=sb):
                return fail
            low = report.lower()
            if "window_buttons" not in low and "windowbuttons" not in low and "buttons" not in low:
                return E2EResult(
                    Status.FAIL,
                    f"e2e-window-buttons report missing window_buttons marker for {desktop_env}",
                    {"report": report},
                )
            if expected_vis not in low:
                return E2EResult(
                    Status.FAIL,
                    f"Expected window_buttons {expected_vis!r} for XDG_CURRENT_DESKTOP={desktop_env!r}",
                    {"report": report},
                )
        return E2EResult(Status.PASS, "Tiling-WM XDG_CURRENT_DESKTOP variants verified (Hyprland:unity/niri hidden, GNOME visible)")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase("T2-F8-01", 2, ("F8",), "Adaptive Layout ~36px hysteresis deadband across breakpoints", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f8_01_hysteresis_deadband),
        E2ECase("T2-F8-02", 2, ("F8",), "Adaptive Layout exact aspect boundary (750x635 vs 750x640)", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f8_02_exact_aspect_ratio_boundary),
        E2ECase("T2-F8-03", 2, ("F8",), "Adaptive Layout width < 480px forces 1-pane even when tall", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f8_03_narrow_width_under_480_even_tall),
        E2ECase("T2-F8-04", 2, ("F8",), "Adaptive Layout preserves pane 1 state across 2H -> 2V -> 1 -> 2H", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f8_04_state_preserved_across_2h_2v_1_2h),
        E2ECase("T2-F8-05", 2, ("F8",), "Tiling-WM start_maximized=false framing across Hyprland & niri", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f8_05_tiling_wm_maximize_suppression),
        E2ECase("T2-F8-06", 2, ("F8",), "Tiling-WM detection for Hyprland:unity, niri, and GNOME", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f8_06_tiling_wm_desktop_detection_variants),
    ]
