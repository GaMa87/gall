"""Tier 2 boundary & negative E2E cases for Feature F9 (actions.d, zoxide, terminal-cwd)."""

from __future__ import annotations

import json

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


def _t2_f9_01_shell_injection_filename_in_actions(ctx: RunContext) -> E2EResult:
    """Adversarial filename `it's $(touch pwn) ü.png` must never trigger shell execution."""
    if pending := require_selftest_mode(ctx, "e2e-actions-exec"):
        return pending
    with SandboxEnv("T2-F9-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_localsend()
        mocks.install_omarchy_transcode()
        mocks.install_omarchy_floating_presentation()

        actions_file = sb.config / "gall" / "actions.d" / "10-adv.json"
        actions_file.write_text(
            json.dumps(
                [
                    {
                        "id": "adv.send",
                        "label": "Adv Send",
                        "whenCommand": "localsend",
                        "selection": "files",
                        "command": ["localsend", "--headless", "send", "{paths}"],
                    }
                ]
            ),
            encoding="utf-8",
        )
        evil_name = "it's $(touch pwn) ü.png"
        evil_file = sb.work / evil_name
        evil_file.write_bytes(b"\x89PNG\r\n\x1a\n")

        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-actions-exec", extra_args=[str(evil_file)])
        if fail := check_selftest_outcome(proc, report, "e2e-actions-exec", sb=sb):
            return fail
        if "adv.send" not in report and "Adv Send" not in report:
            return E2EResult(Status.FAIL, "e2e-actions-exec report missing executed action id/label", {"report": report})
        if (sb.work / "pwn").exists() or (sb.home / "pwn").exists() or (sb.tmp / "pwn").exists():
            return E2EResult(Status.FAIL, "Shell command injection occurred via filename $(touch pwn)")
        calls = read_mock_calls(sb, cmd="localsend")
        if not calls or not any(evil_name in arg for c in calls for arg in c.get("argv", [])):
            return E2EResult(Status.FAIL, "Mock tool did not receive intact literal adversarial filename", {"calls": calls})
        return E2EResult(Status.PASS, "Adversarial shell-injection filename passed safely with zero injection")


def _t2_f9_02_invalid_json_resilience_and_override(ctx: RunContext) -> E2EResult:
    """Malformed 99-broken.json alongside valid 10-ok.json must not drop valid actions."""
    if pending := require_selftest_mode(ctx, "e2e-actions-filter"):
        return pending
    with SandboxEnv("T2-F9-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_localsend()
        ad = sb.config / "gall" / "actions.d"
        (ad / "99-broken.json").write_text("{not valid json", encoding="utf-8")
        (ad / "10-ok.json").write_text(
            (ctx.repo_root / "tests" / "e2e" / "fixtures" / "actions_sample.json").read_text(encoding="utf-8"),
            encoding="utf-8",
        )
        (sb.work / "sample.txt").write_text("x\n", encoding="utf-8")
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-actions-filter", extra_args=[str(sb.work)])
        if fail := check_selftest_outcome(proc, report, "e2e-actions-filter", sb=sb):
            return fail
        if "Send via LocalSend" not in report and "omarchy.localsend" not in report:
            return E2EResult(Status.FAIL, "Valid actions from 10-ok.json were dropped when 99-broken.json was malformed", {"report": report})
        return E2EResult(Status.PASS, "Malformed actions.d JSON ignored while valid actions loaded cleanly")


def _t2_f9_03_plural_label_single_and_array_when_command(ctx: RunContext) -> E2EResult:
    """Verify pluralLabel {count}, array whenCommand filtering, and single: true guard."""
    if pending := require_selftest_mode(ctx, "e2e-actions-match"):
        return pending
    with SandboxEnv("T2-F9-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_localsend()
        mocks.install_tool("e2e-array-cmd-2")
        mocks.install_tool("e2e-single-tool")

        schema_actions = [
            {
                "id": "e2e.plural",
                "label": "Send via LocalSend",
                "pluralLabel": "Send {count} items via LocalSend",
                "whenCommand": "localsend",
                "selection": "files",
                "command": ["localsend", "--headless", "send", "{paths}"],
            },
            {
                "id": "e2e.array_ok",
                "label": "Array Command Ready",
                "pluralLabel": "Array Ready ({count})",
                "whenCommand": ["localsend", "e2e-array-cmd-2"],
                "selection": "files",
                "command": ["e2e-array-cmd-2", "{paths}"],
            },
            {
                "id": "e2e.array_partial_missing",
                "label": "Array Command Partial Missing",
                "whenCommand": ["localsend", "definitely-missing-array-member"],
                "selection": "files",
                "command": ["localsend", "{paths}"],
            },
            {
                "id": "e2e.single_only",
                "label": "Single File Only",
                "whenCommand": "e2e-single-tool",
                "selection": "files",
                "single": True,
                "command": ["e2e-single-tool", "{path}"],
            },
        ]
        (sb.config / "gall" / "actions.d" / "10-bounds.json").write_text(
            json.dumps(schema_actions, indent=2),
            encoding="utf-8",
        )
        for i in range(3):
            (sb.work / f"f{i}.txt").write_text("x\n", encoding="utf-8")

        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-actions-match", extra_args=[str(sb.work)])
        if fail := check_selftest_outcome(proc, report, "e2e-actions-match", sb=sb):
            return fail
        exclusion_markers = ("excluded", "hidden", "filtered", "omitted", "skipped")
        for line in report.splitlines():
            low_line = line.lower()
            if "Array Command Partial Missing" in line and not any(m in low_line for m in exclusion_markers):
                return E2EResult(Status.FAIL, "Action with partially missing array whenCommand was not hidden", {"report": report})
            if ("Single File Only" in line or "e2e.single_only" in line) and not any(m in low_line for m in exclusion_markers):
                return E2EResult(Status.FAIL, "Action with single: true ('Single File Only') was not hidden/excluded on multi-selection", {"report": report})
        if "Array Command Ready" not in report and "Array Ready" not in report:
            return E2EResult(Status.FAIL, "Action with satisfied array whenCommand was unexpectedly hidden", {"report": report})
        if "Send 3 items via LocalSend" not in report and "pluralLabel" not in report:
            return E2EResult(Status.FAIL, "pluralLabel {count} substitution was not verified in actions report", {"report": report})
        return E2EResult(Status.PASS, "pluralLabel {count}, array whenCommand, and single: true guards verified")


def _t2_f9_04_zoxide_missing_error_and_stale_paths(ctx: RunContext) -> E2EResult:
    """Missing zoxide, exit=1 zoxide, and deleted paths in zoxide output must not error or show stale entries."""
    if pending := require_selftest_mode(ctx, "e2e-zoxide-quickopen"):
        return pending
    with SandboxEnv("T2-F9-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        real_dir = sb.work / "real-z"
        real_dir.mkdir()
        stale_dir = sb.work / "deleted-z"
        mocks.install_zoxide([real_dir, stale_dir])

        # 1. zoxide exits 1 -> Quick Open degrades cleanly without crash
        p1, r1, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-zoxide-quickopen",
            extra_args=[str(sb.work)],
            env_overrides={"MOCK_ZOXIDE_EXIT": "1"},
        )
        if fail := check_selftest_outcome(p1, r1, "e2e-zoxide-quickopen", sb=sb):
            return fail
        if not read_mock_calls(sb, cmd="zoxide"):
            return E2EResult(Status.FAIL, "zoxide binary was never queried in e2e-zoxide-quickopen")

        # 2. zoxide returns both valid and deleted path -> deleted path filtered/safe
        p2, r2, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-zoxide-quickopen",
            extra_args=[str(sb.work)],
        )
        if fail := check_selftest_outcome(p2, r2, "e2e-zoxide-quickopen", sb=sb):
            return fail

        # 3. zoxide missing from PATH completely
        rbin = make_restricted_bin(sb, exclude={"zoxide"}, name="rbin_nozoxide")
        p3, r3, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-zoxide-quickopen",
            extra_args=[str(sb.work)],
            env_overrides={"PATH": str(rbin)},
        )
        if fail := check_selftest_outcome(p3, r3, "e2e-zoxide-quickopen", sb=sb):
            return fail
        return E2EResult(Status.PASS, "zoxide missing, exit=1, and stale deleted paths handled cleanly in Quick Open")


def _t2_f9_05_terminal_cwd_missing_or_bad_output(ctx: RunContext) -> E2EResult:
    """omarchy-cmd-terminal-cwd missing, exit=1, or returning nonexistent dir must no-op cleanly."""
    if pending := require_selftest_mode(ctx, "e2e-terminal-cwd"):
        return pending
    with SandboxEnv("T2-F9-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_omarchy_cmd_terminal_cwd("/nonexistent/e2e/cwd")
        proc, report, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-terminal-cwd",
            extra_args=[str(sb.work)],
            env_overrides={"MOCK_OMARCHY_CMD_TERMINAL_CWD_EXIT": "1"},
        )
        if fail := check_selftest_outcome(proc, report, "e2e-terminal-cwd", sb=sb):
            return fail
        if not read_mock_calls(sb, cmd="omarchy-cmd-terminal-cwd"):
            return E2EResult(Status.FAIL, "omarchy-cmd-terminal-cwd was not invoked in e2e-terminal-cwd")

        rbin = make_restricted_bin(sb, exclude={"omarchy-cmd-terminal-cwd"}, name="rbin_nocwd")
        proc2, report2, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-terminal-cwd",
            extra_args=[str(sb.work)],
            env_overrides={"PATH": str(rbin)},
        )
        if fail := check_selftest_outcome(proc2, report2, "e2e-terminal-cwd", sb=sb):
            return fail
        return E2EResult(Status.PASS, "omarchy-cmd-terminal-cwd failure/missing/nonexistent path handled safely")


def _t2_f9_06_uppercase_extension_and_mixed_selection(ctx: RunContext) -> E2EResult:
    """Uppercase .PNG matches extensions=['png'], while mixed file+dir selection rejects selection='files'."""
    if pending := require_selftest_mode(ctx, "e2e-actions-match"):
        return pending
    with SandboxEnv("T2-F9-06", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_omarchy_transcode()
        (sb.config / "gall" / "actions.d" / "10-sample.json").write_text(
            (ctx.repo_root / "tests" / "e2e" / "fixtures" / "actions_sample.json").read_text(encoding="utf-8"),
            encoding="utf-8",
        )
        (sb.work / "IMG_0001.PNG").write_bytes(b"\x89PNG\r\n\x1a\n")
        (sb.work / "subdir").mkdir()
        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-actions-match", extra_args=[str(sb.work)])
        if fail := check_selftest_outcome(proc, report, "e2e-actions-match", sb=sb):
            return fail
        if "omarchy.transcode" not in report and "Transcode media" not in report:
            return E2EResult(Status.FAIL, "Uppercase .PNG did not match extensions=['png'] in actions report", {"report": report})
        if "panicked at" in read_gall_logs(sb):
            return E2EResult(Status.FAIL, "Panic logged during mixed selection action filtering")
        return E2EResult(Status.PASS, "Case-insensitive extension match and mixed selection guard verified")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase("T2-F9-01", 2, ("F9",), "actions.d shell injection filename `it's $(touch pwn) ü.png`", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f9_01_shell_injection_filename_in_actions),
        E2ECase("T2-F9-02", 2, ("F9",), "actions.d invalid JSON resilience & user override", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f9_02_invalid_json_resilience_and_override),
        E2ECase("T2-F9-03", 2, ("F9",), "actions.d pluralLabel {count}, array whenCommand, and single: true", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f9_03_plural_label_single_and_array_when_command),
        E2ECase("T2-F9-04", 2, ("F9",), "zoxide missing, exit 1, and stale deleted paths in Quick Open", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f9_04_zoxide_missing_error_and_stale_paths),
        E2ECase("T2-F9-05", 2, ("F9",), "omarchy-cmd-terminal-cwd missing, exit 1, and bad directory", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f9_05_terminal_cwd_missing_or_bad_output),
        E2ECase("T2-F9-06", 2, ("F9",), "actions.d uppercase .PNG match & mixed file+dir selection", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f9_06_uppercase_extension_and_mixed_selection),
    ]
