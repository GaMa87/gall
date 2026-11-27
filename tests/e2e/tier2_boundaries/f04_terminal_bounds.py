"""Tier 2 boundary & negative E2E cases for Feature F4 (Terminal, Editor, Terminal=true, uwsm-app)."""

from __future__ import annotations

import os

from harness.assertions import read_mock_calls
from harness.mocks import MockRegistry
from harness.sandbox import SandboxEnv
from harness.types import E2ECase, E2EResult, RunContext, Status

from . import (
    check_selftest_outcome,
    make_restricted_bin,
    read_gall_logs,
    require_selftest_mode,
    run_gall_selftest,
)


def _t2_f4_01_spaces_quotes_leading_dash(ctx: RunContext) -> E2EResult:
    """Paths with spaces, single quotes, $() and leading '-' must pass intact without shell split."""
    if pending := require_selftest_mode(ctx, "e2e-editor-omarchy"):
        return pending
    with SandboxEnv("T2-F4-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_xdg_terminal_exec()
        mocks.install_uwsm_app()
        mocks.install_omarchy_launch_editor()
        d = sb.work / "dir with spaces"
        d.mkdir()
        odd = d / "-odd 'name' $(pwn).txt"
        odd.write_text("odd\n", encoding="utf-8")

        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-editor-omarchy", extra_args=[str(odd)])
        if fail := check_selftest_outcome(proc, report, "e2e-editor-omarchy", sb=sb):
            return fail
        if "editor" not in report.lower():
            return E2EResult(Status.FAIL, "e2e-editor-omarchy report missing editor marker", {"report": report})
        if (sb.work / "pwn").exists() or (d / "pwn").exists():
            return E2EResult(Status.FAIL, "Shell injection occurred when opening odd filename")
        calls = read_mock_calls(sb)
        if not any(odd.name in arg for c in calls for arg in c.get("argv", [])):
            return E2EResult(Status.FAIL, "Mock terminal/editor did not receive intact unsplit odd filename", {"calls": calls})
        return E2EResult(Status.PASS, "Odd filename with spaces, quotes, and leading '-' preserved intact")


def _t2_f4_02_terminal_env_fallback_and_missing(ctx: RunContext) -> E2EResult:
    """$TERMINAL fallback when xdg-terminal-exec absent, and graceful error when no terminal exists."""
    if pending := require_selftest_mode(ctx, "e2e-terminal-script"):
        return pending
    with SandboxEnv("T2-F4-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        rbin = make_restricted_bin(
            sb,
            exclude={
                "xdg-terminal-exec",
                "kitty",
                "alacritty",
                "foot",
                "wezterm",
                "ghostty",
                "gnome-terminal",
                "konsole",
                "xterm",
            },
            name="rbin_noterm",
        )
        mocks = MockRegistry(sb)
        mocks.install_tool("mock-term")

        proc, report, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-terminal-script",
            extra_args=[str(sb.work)],
            env_overrides={"TERMINAL": "mock-term", "PATH": f"{sb.mock_bin}:{rbin}"},
        )
        if fail := check_selftest_outcome(proc, report, "e2e-terminal-script", sb=sb):
            return fail
        if "terminal" not in report.lower():
            return E2EResult(Status.FAIL, "e2e-terminal-script report missing terminal marker on $TERMINAL fallback", {"report": report})
        calls = read_mock_calls(sb, cmd="mock-term")
        if not calls:
            return E2EResult(Status.FAIL, "$TERMINAL=mock-term was not used as fallback", {"calls": read_mock_calls(sb)})

        # Now remove mock-term and unset TERMINAL: must return clean error without crash/panic
        (sb.mock_bin / "mock-term").unlink()
        proc2, report2, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-terminal-script",
            extra_args=[str(sb.work)],
            env_overrides={"TERMINAL": None, "PATH": str(rbin)},
        )
        if fail := check_selftest_outcome(proc2, report2, "e2e-terminal-script", sb=sb):
            return fail
        combined2 = (report2 + "\n" + read_gall_logs(sb)).lower()
        if "terminal" not in report2.lower() or not any(
            tok in combined2 for tok in ("no terminal", "missing", "not found", "unavailable", "error", "failed", "cannot", "absent")
        ):
            return E2EResult(
                Status.FAIL,
                "Missing-terminal run did not record expected terminal error/unavailable evidence in report",
                {"report": report2},
            )
        return E2EResult(Status.PASS, "$TERMINAL fallback and missing-terminal guard verified")


def _t2_f4_03_parameterized_editor_and_desktop_fields(ctx: RunContext) -> E2EResult:
    """Parameterized $EDITOR='nvim -u NONE' and .desktop %f/%F/%u/%% field codes preserve argv."""
    for mode in ("e2e-editor-tui", "e2e-openwith-desktop"):
        if pending := require_selftest_mode(ctx, mode):
            return pending
    with SandboxEnv("T2-F4-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_xdg_terminal_exec()
        mocks.install_tool("nvim")
        mocks.install_tool("mock-gui")
        desktop = sb.data / "applications" / "param-test.desktop"
        desktop.write_text(
            "[Desktop Entry]\nType=Application\nName=ParamTest\n"
            'Exec=mock-gui "--opt=a b" %% %u\nMimeType=text/plain;\n',
            encoding="utf-8",
        )
        note = sb.work / "note.txt"
        note.write_text("hi\n", encoding="utf-8")

        p1, r1, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-editor-tui",
            extra_args=[str(note)],
            env_overrides={"EDITOR": "nvim -u NONE", "GALL_E2E_CHAIN_MOCKS": "1"},
        )
        if fail := check_selftest_outcome(p1, r1, "e2e-editor-tui", sb=sb):
            return fail
        if "tui" not in r1.lower() and "editor" not in r1.lower():
            return E2EResult(Status.FAIL, "e2e-editor-tui report missing tui/editor marker", {"report": r1})
        term_calls = read_mock_calls(sb, cmd="xdg-terminal-exec")
        nvim_calls = read_mock_calls(sb, cmd="nvim")
        if not (term_calls or nvim_calls):
            return E2EResult(
                Status.FAIL,
                "Expected xdg-terminal-exec/nvim mock call for parameterized $EDITOR='nvim -u NONE'",
                {"calls": read_mock_calls(sb)},
            )

        p2, r2, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-openwith-desktop",
            extra_args=[str(note)],
            env_overrides={"GALL_E2E_CHAIN_MOCKS": "1"},
        )
        if fail := check_selftest_outcome(p2, r2, "e2e-openwith-desktop", sb=sb):
            return fail
        if "openwith" not in r2.lower() and "desktop" not in r2.lower():
            return E2EResult(Status.FAIL, "e2e-openwith-desktop report missing openwith/desktop marker", {"report": r2})
        gui_calls = read_mock_calls(sb, cmd="mock-gui")
        if not gui_calls:
            return E2EResult(
                Status.FAIL,
                "Expected mock-gui call for .desktop Exec field expansion",
                {"calls": read_mock_calls(sb)},
            )
        argv = gui_calls[0].get("argv", [])
        if "--opt=a b" not in argv or "%" not in argv:
            return E2EResult(Status.FAIL, "Desktop field expansion corrupted quoted arg or %%", {"argv": argv})
        return E2EResult(Status.PASS, "Parameterized $EDITOR and .desktop field code expansion verified")


def _t2_f4_04_hidden_nodisplay_tryexec_desktop(ctx: RunContext) -> E2EResult:
    """NoDisplay=true, Hidden=true, and missing TryExec .desktop files must be excluded from Open With."""
    if pending := require_selftest_mode(ctx, "e2e-openwith-desktop"):
        return pending
    with SandboxEnv("T2-F4-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_tool("bad-hidden-app")
        mocks.install_tool("good-visible-app")
        apps = sb.data / "applications"
        (apps / "nodisplay.desktop").write_text(
            "[Desktop Entry]\nType=Application\nName=NoDisplayApp\nNoDisplay=true\nExec=bad-hidden-app %f\nMimeType=text/plain;\n",
            encoding="utf-8",
        )
        (apps / "hidden.desktop").write_text(
            "[Desktop Entry]\nType=Application\nName=HiddenApp\nHidden=true\nExec=bad-hidden-app %f\nMimeType=text/plain;\n",
            encoding="utf-8",
        )
        (apps / "badtryexec.desktop").write_text(
            "[Desktop Entry]\nType=Application\nName=MissingTryExec\nTryExec=nonexistent-bin-e2e\nExec=bad-hidden-app %f\nMimeType=text/plain;\n",
            encoding="utf-8",
        )
        (apps / "goodvisible.desktop").write_text(
            "[Desktop Entry]\nType=Application\nName=GoodVisibleApp\nExec=good-visible-app %f\nMimeType=text/plain;\n",
            encoding="utf-8",
        )
        note = sb.work / "check.txt"
        note.write_text("check\n", encoding="utf-8")
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-openwith-desktop", extra_args=[str(note)])
        if fail := check_selftest_outcome(proc, report, "e2e-openwith-desktop", sb=sb):
            return fail
        if "openwith" not in report.lower() and "desktop" not in report.lower():
            return E2EResult(Status.FAIL, "e2e-openwith-desktop report missing openwith/desktop marker", {"report": report})
        if read_mock_calls(sb, cmd="bad-hidden-app"):
            return E2EResult(Status.FAIL, "Hidden/NoDisplay/missing-TryExec .desktop entry was unexpectedly launched")
        for forbidden in ("NoDisplayApp", "HiddenApp", "MissingTryExec"):
            if forbidden in report:
                return E2EResult(Status.FAIL, f"Excluded desktop entry {forbidden} surfaced in Open With report", {"report": report})
        return E2EResult(Status.PASS, "NoDisplay, Hidden, and missing TryExec desktop entries ignored")


def _t2_f4_05_non_executable_uwsm_app_stub(ctx: RunContext) -> E2EResult:
    """Non-executable uwsm-app stub (chmod 644) ahead on PATH must not cause EACCES; launches app directly."""
    if pending := require_selftest_mode(ctx, "e2e-openwith-desktop"):
        return pending
    with SandboxEnv("T2-F4-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_tool("mock-gui")
        uwsm_stub = sb.mock_bin / "uwsm-app"
        uwsm_stub.write_text("#!/bin/sh\nexit 99\n", encoding="utf-8")
        os.chmod(uwsm_stub, 0o644)

        (sb.data / "applications" / "gui-app.desktop").write_text(
            "[Desktop Entry]\nType=Application\nName=MockGui\nTerminal=false\nExec=mock-gui %F\nMimeType=text/plain;\n",
            encoding="utf-8",
        )
        note = sb.work / "sample.txt"
        note.write_text("hello\n", encoding="utf-8")
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-openwith-desktop", extra_args=[str(note)])
        if fail := check_selftest_outcome(proc, report, "e2e-openwith-desktop", sb=sb):
            return fail
        if "openwith" not in report.lower() and "desktop" not in report.lower():
            return E2EResult(Status.FAIL, "e2e-openwith-desktop report missing openwith/desktop marker", {"report": report})
        gui_calls = read_mock_calls(sb, cmd="mock-gui")
        if not gui_calls:
            return E2EResult(Status.FAIL, "mock-gui was not launched directly when uwsm-app was chmod 644")
        if read_mock_calls(sb, cmd="uwsm-app"):
            return E2EResult(Status.FAIL, "Non-executable uwsm-app stub should not have executed")
        return E2EResult(Status.PASS, "Non-executable uwsm-app (0644) bypassed cleanly without EACCES", {"calls": gui_calls})


def get_cases() -> list[E2ECase]:
    return [
        E2ECase("T2-F4-01", 2, ("F4",), "Terminal/editor with spaces, quotes, $(), and leading '-' filename", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f4_01_spaces_quotes_leading_dash),
        E2ECase("T2-F4-02", 2, ("F4",), "$TERMINAL fallback and missing-terminal graceful error", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f4_02_terminal_env_fallback_and_missing),
        E2ECase("T2-F4-03", 2, ("F4",), "Parameterized $EDITOR and .desktop %f/%F/%u/%% field codes", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f4_03_parameterized_editor_and_desktop_fields),
        E2ECase("T2-F4-04", 2, ("F4",), "Exclude NoDisplay=true, Hidden=true, and missing TryExec entries", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f4_04_hidden_nodisplay_tryexec_desktop),
        E2ECase("T2-F4-05", 2, ("F4",), "Non-executable uwsm-app stub (chmod 644) bypassed without EACCES", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f4_05_non_executable_uwsm_app_stub),
    ]
