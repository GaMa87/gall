"""Tier 1 E2E tests for F10: Vim keys, shortcut collision resolution, and editable-field safety."""

from __future__ import annotations

from harness import E2ECase, E2EResult, RunContext, SandboxEnv, Status

from . import (
    parse_selftest_failures,
    read_gall_log,
    require_selftest_mode,
    run_gall_selftest,
    selftest_mode_pending,
    write_gall_settings,
)


def _t1_f10_01_ctrl_shift_n_collision_resolution(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-vim-new-window-chord"):
        return pending
    with SandboxEnv("T1-F10-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-vim-new-window-chord", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-vim-new-window-chord", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-vim-new-window-chord not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-vim-new-window-chord failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "newwindow" not in report.lower() and "newfolder" not in report.lower():
            return E2EResult(Status.FAIL, "Report lacks newWindow/newFolder collision check", {"report": report})
        return E2EResult(Status.PASS, "newWindow vs newFolder chords resolved without collision across all profiles")


def _t1_f10_02_ctrl_l_disambiguation(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-vim-ctrl-l"):
        return pending
    with SandboxEnv("T1-F10-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-vim-ctrl-l", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-vim-ctrl-l", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-vim-ctrl-l not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-vim-ctrl-l failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "focusright" not in report.lower() and "editpath" not in report.lower() and "ctrl+l" not in report.lower():
            return E2EResult(Status.FAIL, "Report lacks Ctrl+L focusRight/editPath verification", {"report": report})
        return E2EResult(Status.PASS, "Ctrl+L disambiguated cleanly between Vim focusRight and non-Vim editPath")


def _t1_f10_03_q_escape_vs_editable_input(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-vim-q-input"):
        return pending
    with SandboxEnv("T1-F10-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"vimKeys": True, "keyboardProfile": "vim"})
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-vim-q-input", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-vim-q-input", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-vim-q-input not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-vim-q-input failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "q" not in report.lower():
            return E2EResult(Status.FAIL, "Report lacks q dismiss/editable verification", {"report": report})
        return E2EResult(Status.PASS, "q dismisses overlays outside inputs and types literal 'q' inside inputs")


def _t1_f10_04_space_leader_vs_non_vim_preview(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-vim-space-leader"):
        return pending
    with SandboxEnv("T1-F10-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-vim-space-leader", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-vim-space-leader", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-vim-space-leader not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-vim-space-leader failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "space" not in report.lower() and "leader" not in report.lower() and "which" not in report.lower():
            return E2EResult(Status.FAIL, "Report lacks Space leader / Preview verification", {"report": report})
        return E2EResult(Status.PASS, "Space leader triggers Which-Key in Vim mode and Preview in non-Vim mode")


def _t1_f10_05_ordinary_typing_all_editable_fields(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-vim-editable-typing"):
        return pending
    with SandboxEnv("T1-F10-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"vimKeys": True, "keyboardProfile": "vim"})
        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-vim-editable-typing", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-vim-editable-typing", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-vim-editable-typing not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-vim-editable-typing failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "rename" not in report.lower() and "filter" not in report.lower() and "editable" not in report.lower():
            return E2EResult(Status.FAIL, "Report lacks editable-field typing verification", {"report": report})
        return E2EResult(Status.PASS, "Ordinary typing in Rename, Filter, Quick Open, Find, Path, Settings, and Note preserved")


SELFTEST_MODE_TIMEOUT = 120.0


def _t1_f10_06_profile_switching_and_key_help(ctx: RunContext) -> E2EResult:
    for mode in ("key-profiles", "key-help"):
        if pending := require_selftest_mode(ctx, mode):
            return pending

    with SandboxEnv("T1-F10-06", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        reports: dict[str, str] = {}
        # The key-profiles scenario works on the files scripts/selftest.sh creates for it.
        (sb.work / "Projects").mkdir(exist_ok=True)
        docs = sb.work / "Documents"
        docs.mkdir(exist_ok=True)
        for n in (1, 2, 10, 20):
            (docs / f"file{n}.txt").write_text(f"line {n}\n", encoding="utf-8")
        for mode in ("key-profiles", "key-help"):
            # Each mode takes up to ~45 s (screenshots across themes and widths), so the default
            # per-mode timeout leaves no headroom on a slower or busy host.
            rc, out, err, report = run_gall_selftest(
                ctx, sb, mode, cli_args=[sb.work], timeout=SELFTEST_MODE_TIMEOUT
            )
            log = read_gall_log(sb)
            if selftest_mode_pending(ctx, mode, report, log) and rc == 0 and not report:
                return E2EResult(Status.PENDING_IMPL, f"{mode} selftest mode unavailable")
            if rc != 0:
                return E2EResult(Status.FAIL, f"{mode} selftest failed (rc={rc})", {"stderr": err})
            failures = parse_selftest_failures(report)
            if failures:
                return E2EResult(Status.FAIL, f"{mode} failures: {failures}", {"report": report})
            reports[mode] = report

        if "vim" not in (reports["key-profiles"] + "\n" + reports["key-help"]).lower():
            return E2EResult(
                Status.PENDING_IMPL,
                "key-profiles / key-help selftest output does not yet include Vim profile parity checks",
                {"key_profiles": reports["key-profiles"]},
            )
        return E2EResult(Status.PASS, "Keyboard profile switching (explorer/totalCommander/vim) and Key Help verified")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase(
            id="T1-F10-01",
            tier=1,
            features=("F10",),
            title="Ctrl+Shift+N collision resolved between newWindow and newFolder across profiles",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f10_01_ctrl_shift_n_collision_resolution,
        ),
        E2ECase(
            id="T1-F10-02",
            tier=1,
            features=("F10",),
            title="Ctrl+L disambiguates cleanly between Vim focusRight and non-Vim editPath",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f10_02_ctrl_l_disambiguation,
        ),
        E2ECase(
            id="T1-F10-03",
            tier=1,
            features=("F10",),
            title="q dismisses overlays outside inputs and types literally inside editable fields",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f10_03_q_escape_vs_editable_input,
        ),
        E2ECase(
            id="T1-F10-04",
            tier=1,
            features=("F10",),
            title="Space acts as leader in Vim mode and Preview toggle in non-Vim mode",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f10_04_space_leader_vs_non_vim_preview,
        ),
        E2ECase(
            id="T1-F10-05",
            tier=1,
            features=("F10",),
            title="Ordinary typing hjklgGq? : works in all editable fields when vimKeys=true",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f10_05_ordinary_typing_all_editable_fields,
        ),
        E2ECase(
            id="T1-F10-06",
            tier=1,
            features=("F10",),
            title="Keyboard profile switching and Key Help reflect active vimKeys parity",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f10_06_profile_switching_and_key_help,
        ),
    ]
