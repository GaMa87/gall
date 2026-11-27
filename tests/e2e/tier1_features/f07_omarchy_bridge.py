"""Tier 1 E2E tests for F7: Omarchy live bridge (colors.toml, rounding, fontconfig, glassy, inactive guard)."""

from __future__ import annotations

import shutil
import threading
import time

from harness import E2ECase, E2EResult, MockRegistry, RunContext, SandboxEnv, Status, read_mock_calls

from . import (
    parse_selftest_failures,
    read_gall_log,
    require_selftest_mode,
    run_gall_selftest,
    selftest_mode_pending,
    wait_for_gall_log_contains,
    write_gall_settings,
)


def _fixtures_dir(ctx: RunContext):
    return ctx.repo_root / "tests" / "e2e" / "fixtures"


def _t1_f7_01_dark_colors_toml_startup(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-omarchy-dark"):
        return pending
    with SandboxEnv("T1-F7-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        dark_toml = (_fixtures_dir(ctx) / "colors_dark.toml").read_text(encoding="utf-8")
        mocks.seed_omarchy_theme(name="catppuccin", colors_toml=dark_toml)
        mocks.install_hyprctl(rounding=0)
        srv = mocks.start_hypr_socket2()
        write_gall_settings(sb, {"theme": "omarchy"})

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-omarchy-dark", cli_args=[sb.work])
        srv.close()
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-omarchy-dark", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-omarchy-dark not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-omarchy-dark failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "dark" not in report.lower() or "#1e1e2e" not in report.lower():
            return E2EResult(Status.FAIL, "Dark colors.toml tokens did not resolve in UI", {"report": report})
        return E2EResult(Status.PASS, "Dark colors.toml applied data-theme='dark' and Catppuccin Mocha tokens")


def _t1_f7_02_atomic_theme_swap_to_light(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-omarchy-swap"):
        return pending
    with SandboxEnv("T1-F7-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        dark_toml = (_fixtures_dir(ctx) / "colors_dark.toml").read_text(encoding="utf-8")
        light_toml = (_fixtures_dir(ctx) / "colors_light.toml").read_text(encoding="utf-8")
        mocks.seed_omarchy_theme(name="catppuccin", colors_toml=dark_toml)
        mocks.install_hyprctl(rounding=0)
        srv = mocks.start_hypr_socket2()
        write_gall_settings(sb, {"theme": "omarchy"})

        trigger_file = sb.tmp / "swap_done"

        def _swap_later() -> None:
            wait_for_gall_log_contains(sb, "window shown", timeout=10.0)
            time.sleep(0.3)
            mocks.atomic_swap_omarchy_theme(name="catppuccin-latte", colors_toml=light_toml, light_mode=True)
            trigger_file.write_text("swapped\n", encoding="utf-8")

        t = threading.Thread(target=_swap_later, daemon=True)
        t.start()
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-omarchy-swap", cli_args=[sb.work])
        t.join(timeout=2.0)
        srv.close()
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-omarchy-swap", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-omarchy-swap not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-omarchy-swap failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "light" not in report.lower() or "#eff1f5" not in report.lower():
            return E2EResult(Status.FAIL, "Atomic swap to light colors.toml did not propagate live", {"report": report})
        return E2EResult(Status.PASS, "Atomic directory swap updated Omarchy theme live from dark to light")


def _t1_f7_03_hyprland_rounding_live_reload(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-omarchy-rounding"):
        return pending
    with SandboxEnv("T1-F7-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        dark_toml = (_fixtures_dir(ctx) / "colors_dark.toml").read_text(encoding="utf-8")
        mocks.seed_omarchy_theme(name="catppuccin", colors_toml=dark_toml)
        rfile = sb.tmp / "hypr_rounding.txt"
        mocks.install_hyprctl(rounding_file=rfile, rounding=0)
        srv = mocks.start_hypr_socket2()
        write_gall_settings(sb, {"theme": "omarchy"})

        def _push_reload() -> None:
            if srv.wait_for_client(timeout=8.0):
                time.sleep(0.5)
                rfile.write_text("8\n", encoding="utf-8")
                srv.push_event("configreloaded>>")

        t = threading.Thread(target=_push_reload, daemon=True)
        t.start()
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-omarchy-rounding", cli_args=[sb.work])
        t.join(timeout=2.0)
        srv.close()
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-omarchy-rounding", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-omarchy-rounding not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-omarchy-rounding failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        rounding_calls = [
            c for c in read_mock_calls(sb, "hyprctl") if "decoration:rounding" in " ".join(c.get("argv", []))
        ]
        if len(rounding_calls) < 2:
            return E2EResult(
                Status.FAIL,
                f"Expected >= 2 hyprctl getoption decoration:rounding calls (startup + configreloaded>>), got {len(rounding_calls)}",
                {"calls": rounding_calls},
            )
        if "8px" not in report:
            return E2EResult(Status.FAIL, "Hyprland configreloaded>> did not update --radius to 8px", {"report": report})
        return E2EResult(Status.PASS, "Hyprland .socket2.sock configreloaded>> updated --radius from 0px to 8px")


def _t1_f7_04_fontconfig_monospace_watch(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-omarchy-font"):
        return pending
    with SandboxEnv("T1-F7-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        dark_toml = (_fixtures_dir(ctx) / "colors_dark.toml").read_text(encoding="utf-8")
        mocks.seed_omarchy_theme(name="catppuccin", colors_toml=dark_toml)
        mocks.install_hyprctl(rounding=0)
        srv = mocks.start_hypr_socket2()

        fonts_conf = sb.config / "fontconfig" / "fonts.conf"
        shutil.copy2(_fixtures_dir(ctx) / "fonts.conf", fonts_conf)
        write_gall_settings(sb, {"theme": "omarchy", "omarchyFollowFont": True})

        def _update_font() -> None:
            wait_for_gall_log_contains(sb, "window shown", timeout=10.0)
            time.sleep(0.3)
            updated = fonts_conf.read_text(encoding="utf-8").replace(
                "JetBrainsMono Nerd Font", "FiraCode Nerd Font"
            )
            fonts_conf.write_text(updated, encoding="utf-8")

        t = threading.Thread(target=_update_font, daemon=True)
        t.start()
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-omarchy-font", cli_args=[sb.work])
        t.join(timeout=2.0)
        srv.close()
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-omarchy-font", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-omarchy-font not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-omarchy-font failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "FiraCode" not in report:
            return E2EResult(Status.FAIL, "Monospace font update to FiraCode Nerd Font not reflected", {"report": report})
        return E2EResult(Status.PASS, "fontconfig monospace font reload reflected without restart")


def _t1_f7_05_glassy_mode_and_crisp_text(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-omarchy-glassy"):
        return pending
    with SandboxEnv("T1-F7-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        dark_toml = (_fixtures_dir(ctx) / "colors_dark.toml").read_text(encoding="utf-8")
        mocks.seed_omarchy_theme(name="catppuccin", colors_toml=dark_toml)
        mocks.install_hyprctl(rounding=0)
        srv = mocks.start_hypr_socket2()

        glassy_flag = sb.state / "omarchy" / "toggles" / "hypr" / "window-glassy.lua"
        glassy_flag.write_text("-- glassy enabled\n", encoding="utf-8")
        write_gall_settings(sb, {"theme": "omarchy", "omarchyGlassy": True})

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-omarchy-glassy", cli_args=[sb.work])
        srv.close()
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-omarchy-glassy", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-omarchy-glassy not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-omarchy-glassy failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "omarchy-glassy" not in report:
            return E2EResult(Status.FAIL, "Expected .omarchy-glassy class when window-glassy.lua exists", {"report": report})
        return E2EResult(Status.PASS, "Glassy mode enabled .omarchy-glassy while preserving crisp opaque text")


def _t1_f7_06_zero_overhead_inactive_guard(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-omarchy-inactive"):
        return pending
    with SandboxEnv("T1-F7-06", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_hyprctl(rounding=8)
        # Remove omarchy state directory completely so non-Omarchy host is simulated
        shutil.rmtree(sb.state / "omarchy", ignore_errors=True)

        rc, out, err, report = run_gall_selftest(
            ctx,
            sb,
            "e2e-omarchy-inactive",
            cli_args=[sb.work],
            env_overrides={"HYPRLAND_INSTANCE_SIGNATURE": None},
        )
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-omarchy-inactive", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-omarchy-inactive not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-omarchy-inactive failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        hypr_calls = read_mock_calls(sb, "hyprctl")
        if hypr_calls:
            return E2EResult(Status.FAIL, "hyprctl was invoked despite Omarchy/Hyprland being inactive", {"hypr_calls": hypr_calls})
        return E2EResult(Status.PASS, "Non-Omarchy host kept bridge inactive with 0 hyprctl calls")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase(
            id="T1-F7-01",
            tier=1,
            features=("F7",),
            title="Dark colors.toml resolves data-theme='dark' and Catppuccin palette tokens",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f7_01_dark_colors_toml_startup,
        ),
        E2ECase(
            id="T1-F7-02",
            tier=1,
            features=("F7",),
            title="Atomic theme swap transitions dark to light live without restart",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f7_02_atomic_theme_swap_to_light,
        ),
        E2ECase(
            id="T1-F7-03",
            tier=1,
            features=("F7",),
            title="Hyprland .socket2.sock configreloaded>> reloads decoration:rounding live",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f7_03_hyprland_rounding_live_reload,
        ),
        E2ECase(
            id="T1-F7-04",
            tier=1,
            features=("F7",),
            title="fontconfig monospace font change updates live without restart",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f7_04_fontconfig_monospace_watch,
        ),
        E2ECase(
            id="T1-F7-05",
            tier=1,
            features=("F7",),
            title="window-glassy.lua enables .omarchy-glassy while keeping text opaque",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f7_05_glassy_mode_and_crisp_text,
        ),
        E2ECase(
            id="T1-F7-06",
            tier=1,
            features=("F7",),
            title="Zero-overhead inactive guard makes 0 hyprctl calls on non-Omarchy systems",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f7_06_zero_overhead_inactive_guard,
        ),
    ]
