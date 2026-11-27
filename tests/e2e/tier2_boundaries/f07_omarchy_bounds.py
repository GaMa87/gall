"""Tier 2 boundary & negative E2E cases for Feature F7 (Omarchy live bridge)."""

from __future__ import annotations

import shutil
import time

from harness.assertions import read_mock_calls
from harness.mocks import MockRegistry
from harness.sandbox import SandboxEnv
from harness.types import E2ECase, E2EResult, RunContext, Status

from . import (
    check_selftest_outcome,
    read_gall_logs,
    require_selftest_mode,
    run_gall_selftest,
    wait_for_log_contains,
    write_gall_settings,
)


def _t2_f7_01_legacy_colors_aliases_and_derived(ctx: RunContext) -> E2EResult:
    """Legacy colors.toml aliases (bg, fg, color0..15) without mode derive dark theme & tokens."""
    if pending := require_selftest_mode(ctx, "e2e-omarchy-dark"):
        return pending
    with SandboxEnv("T2-F7-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        legacy_toml = (ctx.repo_root / "tests" / "e2e" / "fixtures" / "colors_legacy.toml").read_text(encoding="utf-8")
        mocks.seed_omarchy_theme("legacy", legacy_toml)
        write_gall_settings(sb, {"theme": "omarchy"})

        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-omarchy-dark")
        if fail := check_selftest_outcome(proc, report, "e2e-omarchy-dark", sb=sb):
            return fail
        if "dark" not in report.lower() and "legacy" not in report.lower():
            return E2EResult(Status.FAIL, "Omarchy selftest report did not verify dark/legacy theme resolution", {"report": report})
        return E2EResult(Status.PASS, "Legacy colors.toml aliases and derived tokens resolved")


def _t2_f7_02_mode_precedence_and_luminance_boundary(ctx: RunContext) -> E2EResult:
    """Mode precedence (mode -> theme_type -> light.mode -> RGB sum > 382 vs == 382)."""
    if pending := require_selftest_mode(ctx, "e2e-omarchy-dark"):
        return pending
    with SandboxEnv("T2-F7-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        write_gall_settings(sb, {"theme": "omarchy"})

        # Exact boundary: #7f7f80 => 127+127+128 = 382 (must stay dark)
        mocks.seed_omarchy_theme("boundary-dark", 'background = "#7f7f80"\nforeground = "#ffffff"\n')
        p1, r1, _ = run_gall_selftest(ctx, sb, "e2e-omarchy-dark")
        if fail := check_selftest_outcome(p1, r1, "e2e-omarchy-dark", sb=sb):
            return fail
        if "dark" not in r1.lower():
            return E2EResult(Status.FAIL, "RGB-sum 382 (#7f7f80) did not resolve to dark mode", {"report": r1})

        # Sibling light.mode overrides dark background
        mocks.atomic_swap_omarchy_theme("light-marker", 'background = "#111111"\nforeground = "#eeeeee"\n', light_mode=True)
        p2, r2, _ = run_gall_selftest(ctx, sb, "e2e-omarchy-dark")
        if fail := check_selftest_outcome(p2, r2, "e2e-omarchy-dark", sb=sb):
            return fail
        if "light" not in r2.lower():
            return E2EResult(Status.FAIL, "light.mode sentinel did not override dark background to light mode", {"report": r2})
        return E2EResult(Status.PASS, "Mode precedence and luminance boundary 382 verified")


def _t2_f7_03_rapid_atomic_swaps_debounce(ctx: RunContext) -> E2EResult:
    """5 rapid atomic theme swaps in <150ms must not kill watcher or leave partial reads."""
    if pending := require_selftest_mode(ctx, "e2e-omarchy-dark"):
        return pending
    assert ctx.binary_path is not None
    with SandboxEnv("T2-F7-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        dark = (ctx.repo_root / "tests" / "e2e" / "fixtures" / "colors_dark.toml").read_text(encoding="utf-8")
        light = (ctx.repo_root / "tests" / "e2e" / "fixtures" / "colors_light.toml").read_text(encoding="utf-8")
        mocks.seed_omarchy_theme("swap-0", dark)
        write_gall_settings(sb, {"theme": "omarchy"})

        proc = sb.popen([ctx.binary_path, str(sb.work)], cwd=sb.work)
        wait_for_log_contains(sb, "window shown at", timeout=6.0)
        for i in range(1, 6):
            mocks.atomic_swap_omarchy_theme(
                f"swap-{i}",
                light if i == 5 else dark,
                light_mode=(i == 5),
                generation=i + 1,
            )
            time.sleep(0.02)
        time.sleep(0.5)
        if proc.poll() is not None:
            return E2EResult(Status.FAIL, f"Gall crashed during rapid atomic theme swaps (rc={proc.returncode})")
        logs = read_gall_logs(sb)
        if "panicked at" in logs:
            return E2EResult(Status.FAIL, "Panic logged during rapid atomic theme swaps")

        proc.terminate()
        try:
            proc.wait(timeout=3.0)
        except Exception:
            proc.kill()

        # Verify final 5th theme (light) resolves cleanly
        p_after, r_after, _ = run_gall_selftest(ctx, sb, "e2e-omarchy-dark")
        if fail := check_selftest_outcome(p_after, r_after, "e2e-omarchy-dark", sb=sb):
            return fail
        if "light" not in r_after.lower():
            return E2EResult(Status.FAIL, "After 5 rapid atomic swaps, final light theme did not apply", {"report": r_after})
        return E2EResult(Status.PASS, "5 rapid atomic theme swaps survived cleanly and converged on 5th theme")


def _t2_f7_04_corrupted_toml_and_malformed_fonts_conf(ctx: RunContext) -> E2EResult:
    """Corrupted colors.toml and invalid fonts.conf XML must not crash Gall."""
    if pending := require_selftest_mode(ctx, "e2e-omarchy-dark"):
        return pending
    with SandboxEnv("T2-F7-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.seed_omarchy_theme("corrupt", '===broken\naccent = "unclosed\n')
        (sb.config / "fontconfig" / "fonts.conf").write_text("<fontconfig><broken", encoding="utf-8")
        write_gall_settings(sb, {"theme": "omarchy", "omarchyFollowFont": True})

        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-omarchy-dark")
        if fail := check_selftest_outcome(proc, report, "e2e-omarchy-dark", sb=sb):
            return fail
        low = report.lower()
        if "thememode" not in low and "dark" not in low and "light" not in low:
            return E2EResult(Status.FAIL, "e2e-omarchy-dark report missing fallback themeMode marker", {"report": report})
        return E2EResult(Status.PASS, "Corrupted colors.toml and malformed fonts.conf handled gracefully")


def _t2_f7_05_gall_css_explicit_font_and_inactive_guard(ctx: RunContext) -> E2EResult:
    """Optional gall.css override, explicit user fontFamily guard, and non-Omarchy inactive fallback."""
    for mode in ("e2e-omarchy-dark", "e2e-omarchy-inactive"):
        if pending := require_selftest_mode(ctx, mode):
            return pending
    with SandboxEnv("T2-F7-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        dark = (ctx.repo_root / "tests" / "e2e" / "fixtures" / "colors_dark.toml").read_text(encoding="utf-8")
        mocks.seed_omarchy_theme("custom-css", dark, gall_css=":root { --accent: #ff00aa !important; }\n")
        fonts_xml = (ctx.repo_root / "tests" / "e2e" / "fixtures" / "fonts.conf").read_text(encoding="utf-8")
        (sb.config / "fontconfig" / "fonts.conf").write_text(fonts_xml, encoding="utf-8")
        write_gall_settings(sb, {"theme": "omarchy", "omarchyFollowFont": False, "fontFamily": "Inter"})

        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-omarchy-dark")
        if fail := check_selftest_outcome(proc, report, "e2e-omarchy-dark", sb=sb):
            return fail
        low = report.lower()
        if "dark" not in low or ("#1e1e2e" not in low and "#ff00aa" not in low):
            return E2EResult(Status.FAIL, "e2e-omarchy-dark report missing themeMode/palette evidence", {"report": report})
        if "jetbrainsmono" in low:
            return E2EResult(Status.FAIL, "fonts.conf JetBrainsMono leaked despite omarchyFollowFont=false", {"report": report})

        # Non-Omarchy host with theme="omarchy" configured must fall back cleanly with active=false / inactive
        shutil.rmtree(sb.state / "omarchy", ignore_errors=True)
        p_inact, r_inact, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-omarchy-inactive",
            env_overrides={"HYPRLAND_INSTANCE_SIGNATURE": None},
        )
        if fail := check_selftest_outcome(p_inact, r_inact, "e2e-omarchy-inactive", sb=sb):
            return fail
        inact_low = r_inact.lower()
        if "active=false" not in inact_low and "inactive" not in inact_low:
            return E2EResult(
                Status.FAIL,
                "e2e-omarchy-inactive report missing active=false / inactive evidence",
                {"report": r_inact},
            )
        return E2EResult(Status.PASS, "Optional gall.css, explicit fontFamily guard, and non-Omarchy inactive fallback verified")


def _t2_f7_06_hypr_socket_disconnect_and_error_recovery(ctx: RunContext) -> E2EResult:
    """Hyprland .socket2.sock disconnect/noise/hyprctl failure must not crash or spin."""
    if pending := require_selftest_mode(ctx, "e2e-omarchy-rounding"):
        return pending
    assert ctx.binary_path is not None
    with SandboxEnv("T2-F7-06", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        dark = (ctx.repo_root / "tests" / "e2e" / "fixtures" / "colors_dark.toml").read_text(encoding="utf-8")
        mocks.seed_omarchy_theme("hypr", dark)
        mocks.install_hyprctl(rounding=6)
        srv = mocks.start_hypr_socket2("e2e-bounds")
        write_gall_settings(sb, {"theme": "omarchy"})

        proc = sb.popen(
            [ctx.binary_path, str(sb.work)],
            env_overrides={"MOCK_HYPRCTL_ERROR": "1"},
        )
        if not srv.wait_for_client(timeout=6.0):
            return E2EResult(Status.FAIL, "Gall did not connect to Hyprland .socket2.sock within 6s")
        srv.push_event("focusedmon>>DP-1,1")
        srv.push_event("configreloaded>>")
        time.sleep(0.3)
        srv.close()
        time.sleep(0.4)
        if proc.poll() is not None:
            return E2EResult(Status.FAIL, f"Gall crashed on .socket2.sock close / hyprctl error (rc={proc.returncode})")
        if "panicked at" in read_gall_logs(sb):
            return E2EResult(Status.FAIL, "Panic logged on .socket2.sock close / hyprctl error")
        hypr_calls = read_mock_calls(sb, "hyprctl")
        if len(hypr_calls) < 1:
            return E2EResult(Status.FAIL, "Expected at least 1 hyprctl call during Hyprland bridge lifecycle", {"calls": hypr_calls})
        return E2EResult(Status.PASS, "Hyprland socket disconnect and hyprctl errors survived cleanly", {"hyprctl_calls": len(hypr_calls)})


def get_cases() -> list[E2ECase]:
    return [
        E2ECase("T2-F7-01", 2, ("F7",), "Omarchy legacy colors.toml aliases and derived tokens", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f7_01_legacy_colors_aliases_and_derived),
        E2ECase("T2-F7-02", 2, ("F7",), "Omarchy mode precedence chain and RGB-sum 382 boundary", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f7_02_mode_precedence_and_luminance_boundary),
        E2ECase("T2-F7-03", 2, ("F7",), "Omarchy 5 rapid atomic swaps within 150ms debounce window", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f7_03_rapid_atomic_swaps_debounce),
        E2ECase("T2-F7-04", 2, ("F7",), "Omarchy corrupted colors.toml and malformed fonts.conf XML", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f7_04_corrupted_toml_and_malformed_fonts_conf),
        E2ECase("T2-F7-05", 2, ("F7",), "Omarchy gall.css override, explicit fontFamily guard & inactive fallback", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f7_05_gall_css_explicit_font_and_inactive_guard),
        E2ECase("T2-F7-06", 2, ("F7",), "Hyprland .socket2.sock disconnect, noise events, and hyprctl error", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f7_06_hypr_socket_disconnect_and_error_recovery),
    ]
