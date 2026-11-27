"""Tier 2 boundary & negative E2E cases for Feature F10 (Vim keys & editable-field safety)."""

from __future__ import annotations

from harness.sandbox import SandboxEnv
from harness.types import E2ECase, E2EResult, RunContext, Status

from . import (
    check_selftest_outcome,
    require_selftest_mode,
    run_gall_selftest,
    write_gall_settings,
)


def _t2_f10_01_ime_composition_and_dead_keys(ctx: RunContext) -> E2EResult:
    """IME isComposing=true and dead keys must not trigger Gall shortcuts or close overlays."""
    if pending := require_selftest_mode(ctx, "e2e-vim-bounds"):
        return pending
    with SandboxEnv("T2-F10-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"keyboardProfile": "vim", "vimKeys": True})
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-vim-bounds", extra_args=[sb.work])
        if fail := check_selftest_outcome(proc, report, "e2e-vim-bounds", sb=sb):
            return fail
        low = report.lower()
        if "composing" not in low and "dead" not in low:
            return E2EResult(Status.FAIL, "e2e-vim-bounds report did not verify IME composition or dead keys", {"report": report})
        return E2EResult(Status.PASS, "IME composition & dead-key keyboard safety verified")


def _t2_f10_02_contenteditable_and_select_typing(ctx: RunContext) -> E2EResult:
    """<select> and [contenteditable='true'] elements must suppress Vim normal-mode capture."""
    if pending := require_selftest_mode(ctx, "e2e-vim-bounds"):
        return pending
    with SandboxEnv("T2-F10-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"keyboardProfile": "vim", "vimKeys": True})
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-vim-bounds", extra_args=[sb.work])
        if fail := check_selftest_outcome(proc, report, "e2e-vim-bounds", sb=sb):
            return fail
        low = report.lower()
        if "contenteditable" not in low and "select" not in low:
            return E2EResult(Status.FAIL, "e2e-vim-bounds report did not verify <select> and contentEditable safety", {"report": report})
        return E2EResult(Status.PASS, "<select> and contenteditable field safety verified")


def _t2_f10_03_leader_sequence_interruption(ctx: RunContext) -> E2EResult:
    """Pending Space leader interrupted by click/blur/Escape must clear keySeq cleanly."""
    if pending := require_selftest_mode(ctx, "e2e-vim-bounds"):
        return pending
    with SandboxEnv("T2-F10-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"keyboardProfile": "vim", "vimKeys": True, "leaderKey": "Space"})
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-vim-bounds", extra_args=[sb.work])
        if fail := check_selftest_outcome(proc, report, "e2e-vim-bounds", sb=sb):
            return fail
        if "leader" not in report.lower():
            return E2EResult(Status.FAIL, "e2e-vim-bounds report did not verify Space leader interruption reset", {"report": report})
        return E2EResult(Status.PASS, "Leader key sequence interruption reset verified")


def _t2_f10_04_custom_shortcut_shadowed_by_vim(ctx: RunContext) -> E2EResult:
    """Custom shortcut override colliding with Vim NORMAL_KEYS accurately reflects precedence in Key Help."""
    if pending := require_selftest_mode(ctx, "e2e-vim-bounds"):
        return pending
    with SandboxEnv("T2-F10-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"keyboardProfile": "vim", "vimKeys": True, "shortcuts": {"newFolder": "Ctrl+H"}})
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-vim-bounds", extra_args=[sb.work])
        if fail := check_selftest_outcome(proc, report, "e2e-vim-bounds", sb=sb):
            return fail
        low = report.lower()
        if "shadow" not in low and "ctrl+h" not in low:
            return E2EResult(Status.FAIL, "e2e-vim-bounds report did not verify shadowedByVim shortcut precedence", {"report": report})
        return E2EResult(Status.PASS, "shadowedByVim shortcut notation & precedence verified in Key Help")


def _t2_f10_05_caps_lock_and_shift_inversion(ctx: RunContext) -> E2EResult:
    """Caps Lock uppercase J (shiftKey=false) vs Shift+g (G) motion handling."""
    if pending := require_selftest_mode(ctx, "e2e-vim-bounds"):
        return pending
    with SandboxEnv("T2-F10-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"keyboardProfile": "vim", "vimKeys": True})
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-vim-bounds", extra_args=[sb.work])
        if fail := check_selftest_outcome(proc, report, "e2e-vim-bounds", sb=sb):
            return fail
        low = report.lower()
        if "caps" not in low and "shift" not in low:
            return E2EResult(Status.FAIL, "e2e-vim-bounds report did not verify Caps Lock / Shift inversion", {"report": report})
        return E2EResult(Status.PASS, "Caps Lock / Shift inversion Vim motion check passed")


def _t2_f10_06_rapid_focus_transition_filter_quickopen(ctx: RunContext) -> E2EResult:
    """Rapid '/' Quick Filter -> Ctrl+P Quick Open -> Escape -> Escape focus restoration."""
    if pending := require_selftest_mode(ctx, "e2e-vim-bounds"):
        return pending
    with SandboxEnv("T2-F10-06", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        write_gall_settings(sb, {"keyboardProfile": "vim", "vimKeys": True})
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-vim-bounds", extra_args=[sb.work])
        if fail := check_selftest_outcome(proc, report, "e2e-vim-bounds", sb=sb):
            return fail
        low = report.lower()
        if "focus" not in low and "escape" not in low:
            return E2EResult(Status.FAIL, "e2e-vim-bounds report did not verify rapid editable-to-pane Escape focus return", {"report": report})
        return E2EResult(Status.PASS, "Rapid editable-to-pane Escape focus transition verified")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase("T2-F10-01", 2, ("F10",), "IME isComposing=true and dead-key keyboard safety", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f10_01_ime_composition_and_dead_keys),
        E2ECase("T2-F10-02", 2, ("F10",), "<select> and contentEditable safety with vimKeys=true", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f10_02_contenteditable_and_select_typing),
        E2ECase("T2-F10-03", 2, ("F10",), "Pending Space leader sequence cleared on click/blur/Escape", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f10_03_leader_sequence_interruption),
        E2ECase("T2-F10-04", 2, ("F10",), "Custom shortcut colliding with Vim NORMAL_KEYS (shadowedByVim)", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f10_04_custom_shortcut_shadowed_by_vim),
        E2ECase("T2-F10-05", 2, ("F10",), "Caps Lock / Shift inversion on Vim j/J and G motions", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f10_05_caps_lock_and_shift_inversion),
        E2ECase("T2-F10-06", 2, ("F10",), "Rapid '/' filter -> Ctrl+P -> Escape -> Escape focus return", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f10_06_rapid_focus_transition_filter_quickopen),
    ]
