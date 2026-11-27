"""Tier 1 E2E tests for F4: Terminal, Editor, Terminal=true, and uwsm-app."""

from __future__ import annotations

import os

from harness import E2ECase, E2EResult, MockRegistry, RunContext, SandboxEnv, Status, read_mock_calls

from . import (
    parse_selftest_failures,
    read_gall_log,
    require_selftest_mode,
    run_gall_selftest,
    selftest_mode_pending,
)


def _t1_f4_01_xdg_terminal_exec_and_script_argv(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-terminal-script"):
        return pending
    with SandboxEnv("T1-F4-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_xdg_terminal_exec()
        mocks.install_tool("kitty")

        work_dir = sb.work / "proj"
        work_dir.mkdir()
        script = work_dir / "run.sh"
        script.write_text("#!/bin/sh\necho ok\n", encoding="utf-8")
        os.chmod(script, 0o755)

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-terminal-script", cli_args=[work_dir])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-terminal-script", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-terminal-script not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-terminal-script failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        term_calls = read_mock_calls(sb, "xdg-terminal-exec")
        kitty_calls = read_mock_calls(sb, "kitty")
        if not term_calls:
            return E2EResult(Status.FAIL, "xdg-terminal-exec was not prioritized over kitty", {"kitty_calls": kitty_calls})
        if kitty_calls:
            return E2EResult(Status.FAIL, "kitty was invoked despite xdg-terminal-exec being present")

        all_argv = [arg for call in term_calls for arg in call.get("argv", [])]
        if not any(a.startswith("--dir=") for a in all_argv):
            return E2EResult(Status.FAIL, "xdg-terminal-exec missing --dir= flag", {"calls": term_calls})
        for call in term_calls:
            argv = call.get("argv", [])
            if len(argv) >= 2 and "-e" in argv[:3]:
                return E2EResult(Status.FAIL, "xdg-terminal-exec unexpectedly passed -e guard", {"argv": argv})
        return E2EResult(Status.PASS, "xdg-terminal-exec prioritized with --dir= and clean script argv")


def _t1_f4_02_omarchy_launch_editor_priority(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-editor-omarchy"):
        return pending
    with SandboxEnv("T1-F4-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_omarchy_launch_editor()
        mocks.install_tool("code")

        note = sb.work / "note.txt"
        note.write_text("hello editor\n", encoding="utf-8")

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-editor-omarchy", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-editor-omarchy", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-editor-omarchy not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-editor-omarchy failed (rc={rc})", {"stderr": err})
        if failures := parse_selftest_failures(report):
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        calls = read_mock_calls(sb, "omarchy-launch-editor")
        if not calls or not any(str(note) in " ".join(c.get("argv", [])) for c in calls):
            return E2EResult(Status.FAIL, "omarchy-launch-editor was not invoked with note.txt", {"calls": calls})
        return E2EResult(Status.PASS, "omarchy-launch-editor prioritized and invoked with note.txt")


def _t1_f4_03_tui_editor_wrapped_in_terminal(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-editor-tui"):
        return pending
    with SandboxEnv("T1-F4-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_xdg_terminal_exec()
        mocks.install_tool("nvim")

        note = sb.work / "note.txt"
        note.write_text("tui edit\n", encoding="utf-8")

        rc, out, err, report = run_gall_selftest(
            ctx,
            sb,
            "e2e-editor-tui",
            cli_args=[sb.work],
            env_overrides={"EDITOR": "nvim", "GALL_E2E_CHAIN_MOCKS": "1"},
        )
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-editor-tui", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-editor-tui not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-editor-tui failed (rc={rc})", {"stderr": err})
        if failures := parse_selftest_failures(report):
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        term_calls = read_mock_calls(sb, "xdg-terminal-exec")
        if not term_calls or not any("nvim" in " ".join(c.get("argv", [])) for c in term_calls):
            return E2EResult(Status.FAIL, "TUI editor nvim was not wrapped in xdg-terminal-exec", {"term_calls": term_calls})
        nvim_calls = read_mock_calls(sb, "nvim")
        if not nvim_calls or not any(str(note) in " ".join(c.get("argv", [])) for c in nvim_calls):
            return E2EResult(Status.FAIL, "Chained nvim editor call missing note.txt", {"nvim_calls": nvim_calls})
        return E2EResult(Status.PASS, "TUI editor nvim wrapped inside xdg-terminal-exec")


def _t1_f4_04_desktop_terminal_true(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-openwith-desktop"):
        return pending
    with SandboxEnv("T1-F4-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_xdg_terminal_exec()
        mocks.install_tool("mock-tui")

        desktop = sb.data / "applications" / "tui-view.desktop"
        desktop.write_text(
            "[Desktop Entry]\n"
            "Type=Application\n"
            "Name=TUI Viewer\n"
            "Exec=mock-tui %F\n"
            "Terminal=true\n"
            "MimeType=text/plain;\n",
            encoding="utf-8",
        )
        note = sb.work / "note.txt"
        note.write_text("terminal desktop\n", encoding="utf-8")

        rc, out, err, report = run_gall_selftest(
            ctx,
            sb,
            "e2e-openwith-desktop",
            cli_args=[sb.work],
            env_overrides={"GALL_E2E_CHAIN_MOCKS": "1"},
        )
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-openwith-desktop", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-openwith-desktop not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-openwith-desktop failed (rc={rc})", {"stderr": err})
        if failures := parse_selftest_failures(report):
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        term_calls = read_mock_calls(sb, "xdg-terminal-exec")
        if not term_calls or not any("mock-tui" in " ".join(c.get("argv", [])) for c in term_calls):
            return E2EResult(Status.FAIL, "Terminal=true desktop entry did not launch via terminal", {"term_calls": term_calls})
        return E2EResult(Status.PASS, "DesktopEntry Terminal=true launched mock-tui via xdg-terminal-exec")


def _t1_f4_05_uwsm_app_wrapping(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-openwith-desktop"):
        return pending
    with SandboxEnv("T1-F4-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        uwsm_path = mocks.install_uwsm_app()
        mocks.install_tool("mock-gui")

        desktop = sb.data / "applications" / "gui-app.desktop"
        desktop.write_text(
            "[Desktop Entry]\n"
            "Type=Application\n"
            "Name=Mock GUI\n"
            "Exec=mock-gui %F\n"
            "Terminal=false\n"
            "MimeType=text/plain;\n",
            encoding="utf-8",
        )
        note = sb.work / "note.txt"
        note.write_text("uwsm test\n", encoding="utf-8")

        rc, out, err, report = run_gall_selftest(
            ctx,
            sb,
            "e2e-openwith-desktop",
            cli_args=[sb.work],
            env_overrides={"GALL_E2E_CHAIN_MOCKS": "1"},
        )
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-openwith-desktop", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-openwith-desktop not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"With uwsm-app failed (rc={rc})", {"stderr": err})
        if failures := parse_selftest_failures(report):
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        uwsm_calls = read_mock_calls(sb, "uwsm-app")
        if not uwsm_calls or not any("mock-gui" in " ".join(c.get("argv", [])) for c in uwsm_calls):
            return E2EResult(Status.FAIL, "Expected uwsm-app -- mock-gui when uwsm-app is on PATH", {"uwsm_calls": uwsm_calls})

        # Remove uwsm-app from PATH and ensure direct mock-gui execution still works
        uwsm_path.unlink()
        rc2, out2, err2, report2 = run_gall_selftest(
            ctx,
            sb,
            "e2e-openwith-desktop",
            cli_args=[sb.work],
            env_overrides={"PATH": f"{sb.mock_bin}:/usr/bin:/bin"},
        )
        if rc2 != 0:
            return E2EResult(Status.FAIL, f"Without uwsm-app failed (rc={rc2})", {"stderr": err2})
        if failures2 := parse_selftest_failures(report2):
            return E2EResult(Status.FAIL, f"Without uwsm-app selftest failures: {failures2}", {"report": report2})
        gui_calls = read_mock_calls(sb, "mock-gui")
        if not gui_calls:
            return E2EResult(Status.FAIL, "mock-gui was not launched directly when uwsm-app was absent")
        return E2EResult(Status.PASS, "External app wrapped via uwsm-app when present and run directly when absent")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase(
            id="T1-F4-01",
            tier=1,
            features=("F4",),
            title="xdg-terminal-exec priority and clean script_argv without -e guard",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f4_01_xdg_terminal_exec_and_script_argv,
        ),
        E2ECase(
            id="T1-F4-02",
            tier=1,
            features=("F4",),
            title="omarchy-launch-editor prioritized when available on PATH",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f4_02_omarchy_launch_editor_priority,
        ),
        E2ECase(
            id="T1-F4-03",
            tier=1,
            features=("F4",),
            title="TUI editor (nvim) wrapped in configured terminal",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f4_03_tui_editor_wrapped_in_terminal,
        ),
        E2ECase(
            id="T1-F4-04",
            tier=1,
            features=("F4",),
            title="Desktop entry with Terminal=true launches inside terminal",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f4_04_desktop_terminal_true,
        ),
        E2ECase(
            id="T1-F4-05",
            tier=1,
            features=("F4",),
            title="External desktop app wrapped with uwsm-app when on PATH",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f4_05_uwsm_app_wrapping,
        ),
    ]
