"""Tier 1 E2E tests for F8: 2D Adaptive Layout & Tiling-WM window framing."""

from __future__ import annotations

import json

from harness import E2ECase, E2EResult, MockRegistry, RunContext, SandboxEnv, Status

from . import (
    parse_selftest_failures,
    read_gall_log,
    require_selftest_mode,
    run_gall_selftest,
    selftest_mode_pending,
    write_gall_settings,
)


def _t1_f8_01_wide_horizontal_two_panes(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-adaptive-wide"):
        return pending
    with SandboxEnv("T1-F8-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"adaptiveLayout": True})
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-adaptive-wide", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-adaptive-wide", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-adaptive-wide not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-adaptive-wide failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "horizontal" not in report.lower():
            return E2EResult(Status.FAIL, "Wide 1200x800 did not resolve to horizontal 2-pane mode", {"report": report})
        return E2EResult(Status.PASS, "Wide window resolved to 2 horizontal panes with centered PanelMap")


def _t1_f8_02_tall_vertical_two_panes(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-adaptive-tall"):
        return pending
    with SandboxEnv("T1-F8-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"adaptiveLayout": True})
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-adaptive-tall", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-adaptive-tall", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-adaptive-tall not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-adaptive-tall failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "vertical" not in report.lower():
            return E2EResult(Status.FAIL, "Tall half-tile 700x900 did not resolve to vertical stacked 2-pane mode", {"report": report})
        return E2EResult(Status.PASS, "Tall 700x900 half-tile stacked 2 panes vertically without layout errors")


def _t1_f8_03_compact_single_and_sidebar_autohide(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-adaptive-compact"):
        return pending
    with SandboxEnv("T1-F8-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        cfg_path = write_gall_settings(sb, {"adaptiveLayout": True, "sidebarVisible": True})
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-adaptive-compact", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-adaptive-compact", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-adaptive-compact not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-adaptive-compact failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "single" not in report.lower():
            return E2EResult(Status.FAIL, "Compact 640x480 did not resolve to single-pane mode", {"report": report})
        saved = json.loads(cfg_path.read_text(encoding="utf-8"))
        if saved.get("sidebarVisible") is not True:
            return E2EResult(Status.FAIL, f"Transient narrow sidebar auto-hide mutated persisted settings.json: {saved}")
        return E2EResult(Status.PASS, "Compact quarter-tile collapsed to 1 pane and auto-hid sidebar transiently")


def _t1_f8_04_manual_override_and_disabled_adaptive(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-adaptive-override"):
        return pending
    with SandboxEnv("T1-F8-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"adaptiveLayout": False, "sidebarVisible": True})
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-adaptive-override", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-adaptive-override", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-adaptive-override not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-adaptive-override failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "override" not in report.lower() and "classic" not in report.lower() and "adaptivelayout" not in report.lower():
            return E2EResult(Status.FAIL, "Manual override / adaptiveLayout=false not evidenced in report", {"report": report})
        return E2EResult(Status.PASS, "Manual classic/collapse/sidebar override and adaptiveLayout=false respected")


def _t1_f8_05_tiling_wm_no_forced_maximize(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-tiling-maximize"):
        return pending
    with SandboxEnv("T1-F8-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_hyprctl(rounding=0)
        write_gall_settings(sb, {"startMaximized": True})

        rc, out, err, report = run_gall_selftest(
            ctx,
            sb,
            "e2e-tiling-maximize",
            cli_args=[sb.work],
            env_overrides={"HYPRLAND_INSTANCE_SIGNATURE": "e2e-sig", "XDG_CURRENT_DESKTOP": "Hyprland"},
        )
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-tiling-maximize", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-tiling-maximize not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-tiling-maximize failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "maximiz" not in report.lower() and "tiling" not in report.lower():
            return E2EResult(Status.FAIL, "Tiling WM maximize suppression not evidenced in report", {"report": report})
        return E2EResult(Status.PASS, "Tiling WM environment did not force maximize at startup")


def _t1_f8_06_configurable_window_buttons(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-window-buttons"):
        return pending
    with SandboxEnv("T1-F8-06", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        for mode in ("auto", "always", "never"):
            write_gall_settings(sb, {"windowButtons": mode})
            rc, out, err, report = run_gall_selftest(
                ctx,
                sb,
                "e2e-window-buttons",
                cli_args=[sb.work],
                env_overrides={"HYPRLAND_INSTANCE_SIGNATURE": "e2e-sig", "XDG_CURRENT_DESKTOP": "Hyprland"},
            )
            log = read_gall_log(sb)
            if selftest_mode_pending(ctx, "--e2e-window-buttons", report, log) and rc == 0 and not report:
                return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-window-buttons not yet registered")
            if rc != 0:
                return E2EResult(Status.FAIL, f"windowButtons={mode} failed (rc={rc})", {"stderr": err})
            failures = parse_selftest_failures(report)
            if failures:
                return E2EResult(Status.FAIL, f"windowButtons={mode} failures: {failures}", {"report": report})
            if mode not in report.lower():
                return E2EResult(Status.FAIL, f"windowButtons={mode} state not evidenced in report", {"report": report})

        return E2EResult(Status.PASS, "windowButtons auto/always/never behaved properly under tiling WM")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase(
            id="T1-F8-01",
            tier=1,
            features=("F8",),
            title="Wide window (1200x800) uses 2 horizontal side-by-side panes",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f8_01_wide_horizontal_two_panes,
        ),
        E2ECase(
            id="T1-F8-02",
            tier=1,
            features=("F8",),
            title="Tall half-tile (700x900) switches to 2 vertical stacked panes",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f8_02_tall_vertical_two_panes,
        ),
        E2ECase(
            id="T1-F8-03",
            tier=1,
            features=("F8",),
            title="Compact quarter-tile (640x480) switches to 1 pane and transiently auto-hides sidebar",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f8_03_compact_single_and_sidebar_autohide,
        ),
        E2ECase(
            id="T1-F8-04",
            tier=1,
            features=("F8",),
            title="Manual pane collapse, sidebar toggle override, and adaptiveLayout=false respected",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f8_04_manual_override_and_disabled_adaptive,
        ),
        E2ECase(
            id="T1-F8-05",
            tier=1,
            features=("F8",),
            title="Tiling WM suppresses forced maximize on startup",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f8_05_tiling_wm_no_forced_maximize,
        ),
        E2ECase(
            id="T1-F8-06",
            tier=1,
            features=("F8",),
            title="windowButtons setting (auto/always/never) controls titlebar controls visibility",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f8_06_configurable_window_buttons,
        ),
    ]
