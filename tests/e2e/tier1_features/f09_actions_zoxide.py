"""Tier 1 E2E tests for F9: Extensible actions.d/*.json, zoxide, and omarchy-cmd-terminal-cwd."""

from __future__ import annotations

import json
import shutil
from pathlib import Path

from harness import E2ECase, E2EResult, MockRegistry, RunContext, SandboxEnv, Status, read_mock_calls

from . import (
    parse_selftest_failures,
    read_gall_log,
    require_selftest_mode,
    run_gall_selftest,
    selftest_mode_pending,
)


def _validate_actions_schema(path: Path) -> str | None:
    """Validate actions.json contract aligned with packaging/linux/omarchy-actions.json."""
    if not path.is_file():
        return f"Actions file missing: {path}"
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, list) or not data:
        return f"Expected non-empty JSON array in {path}"
    seen_ids: set[str] = set()
    has_plural = False
    has_array_when = False
    has_single = False
    for act in data:
        if not isinstance(act, dict):
            return f"Action entry is not an object in {path}"
        aid = act.get("id")
        if not isinstance(aid, str) or not aid or aid in seen_ids:
            return f"Invalid or duplicate action id {aid!r} in {path}"
        seen_ids.add(aid)
        if not isinstance(act.get("label"), str) or not act["label"]:
            return f"Missing label in {aid}"
        if "labelMulti" in act:
            return f"Deprecated labelMulti present in {aid} (must use pluralLabel)"
        if "pluralLabel" in act:
            if not isinstance(act["pluralLabel"], str) or "{count}" not in act["pluralLabel"]:
                return f"Invalid pluralLabel in {aid}"
            has_plural = True
        wc = act.get("whenCommand")
        if isinstance(wc, list):
            if not wc or not all(isinstance(c, str) and c for c in wc):
                return f"Invalid array whenCommand in {aid}"
            has_array_when = True
        elif wc is not None and (not isinstance(wc, str) or not wc):
            return f"Invalid string whenCommand in {aid}"
        if act.get("selection") not in ("any", "files", "dirs", "none"):
            return f"Invalid selection in {aid}: {act.get('selection')!r}"
        if "single" in act:
            if not isinstance(act["single"], bool):
                return f"single must be boolean in {aid}"
            if act["single"]:
                has_single = True
        cmd = act.get("command")
        if not isinstance(cmd, list) or not cmd or not all(isinstance(c, str) for c in cmd):
            return f"Invalid command list in {aid}"
    if not (has_plural and has_array_when and has_single):
        return (
            f"{path.name} missing required schema coverage "
            f"(pluralLabel={has_plural}, array whenCommand={has_array_when}, single=True={has_single})"
        )
    return None


def _seed_sample_actions(ctx: RunContext, sb: SandboxEnv) -> None:
    src = ctx.repo_root / "tests" / "e2e" / "fixtures" / "actions_sample.json"
    dst = sb.config / "gall" / "actions.d" / "10-sample.json"
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)


def _t1_f9_01_when_command_filtering(ctx: RunContext) -> E2EResult:
    pkg_actions = ctx.repo_root / "packaging" / "linux" / "omarchy-actions.json"
    sample_actions = ctx.repo_root / "tests" / "e2e" / "fixtures" / "actions_sample.json"
    if not pkg_actions.is_file():
        return E2EResult(Status.PENDING_IMPL, "packaging/linux/omarchy-actions.json missing")
    if err_msg := _validate_actions_schema(pkg_actions):
        return E2EResult(Status.FAIL, err_msg)
    if err_msg := _validate_actions_schema(sample_actions):
        return E2EResult(Status.FAIL, err_msg)

    if pending := require_selftest_mode(ctx, "e2e-actions-filter"):
        return pending
    with SandboxEnv("T1-F9-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_localsend()
        _seed_sample_actions(ctx, sb)
        (sb.work / "sample.txt").write_text("hi\n", encoding="utf-8")

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-actions-filter", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-actions-filter", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-actions-filter not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-actions-filter failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        # omarchy.missing_tool uses array whenCommand=["sh", "definitely-missing-e2e-cmd"];
        # omarchy.dir_tool uses array whenCommand=["e2e-dir-tool", "sh"] (e2e-dir-tool absent here)
        if "Send via LocalSend" not in report or "Missing tool action" in report or "Inspect directory" in report:
            return E2EResult(Status.FAIL, "whenCommand PATH filtering (string & array) inaccurate", {"report": report})
        return E2EResult(Status.PASS, "omarchy-actions.json schema and string/array whenCommand filtering verified")


def _t1_f9_02_selection_mime_extension_filtering(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-actions-match"):
        return pending
    with SandboxEnv("T1-F9-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_omarchy_transcode()
        mocks.install_tool("e2e-dir-tool")
        _seed_sample_actions(ctx, sb)

        (sb.work / "pic.png").write_bytes(b"\x89PNG\r\n\x1a\n")
        (sb.work / "pic2.jpg").write_bytes(b"\xff\xd8\xff\xe0")
        (sb.work / "note.txt").write_text("text\n", encoding="utf-8")
        (sb.work / "subdir").mkdir()
        (sb.work / "subdir2").mkdir()

        # The host may ship a real `localsend` (Omarchy does): hide it, the test needs it absent.
        from tier2_boundaries import make_restricted_bin

        rbin = make_restricted_bin(sb, exclude={"localsend"}, name="rbin_nolocalsend")
        rc, out, err, report = run_gall_selftest(
            ctx,
            sb,
            "e2e-actions-match",
            cli_args=[sb.work],
            env_overrides={"PATH": f"{sb.mock_bin}:{rbin}"},
        )
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-actions-match", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-actions-match not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-actions-match failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if "PASS" not in report:
            return E2EResult(Status.FAIL, "e2e-actions-match report missing PASS marker", {"report": report})
        if "Transcode media" not in report or "Inspect directory" not in report:
            return E2EResult(
                Status.FAIL,
                "e2e-actions-match report missing matched action labels (Transcode media / Inspect directory)",
                {"report": report},
            )
        lower_report = report.lower()
        if not any(tok in lower_report for tok in ("plurallabel", "plural", "media files")):
            return E2EResult(
                Status.FAIL,
                "e2e-actions-match report missing pluralLabel formatting check",
                {"report": report},
            )
        if not any(tok in lower_report for tok in ("single", "excluded", "hidden", "note.txt")):
            return E2EResult(
                Status.FAIL,
                "e2e-actions-match report missing single=true / non-matching exclusion check",
                {"report": report},
            )
        if "Missing tool action" in report or "Send via LocalSend" in report:
            return E2EResult(
                Status.FAIL,
                "e2e-actions-match unexpectedly matched actions whose whenCommand is absent from PATH",
                {"report": report},
            )
        return E2EResult(Status.PASS, "selection, single=true, pluralLabel, mimePrefixes, and extensions filtered accurately")


def _t1_f9_03_placeholder_substitution_no_word_split(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-actions-exec"):
        return pending
    with SandboxEnv("T1-F9-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_localsend()
        _seed_sample_actions(ctx, sb)

        f1 = sb.work / "a b.png"
        f2 = sb.work / "c d.png"
        f1.write_bytes(b"1")
        f2.write_bytes(b"2")

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-actions-exec", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-actions-exec", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-actions-exec not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-actions-exec failed (rc={rc})", {"stderr": err})
        if failures := parse_selftest_failures(report):
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        calls = read_mock_calls(sb, "localsend")
        if not calls:
            return E2EResult(Status.FAIL, "localsend action was not invoked")
        argv = calls[0].get("argv", [])
        if str(f1) not in argv or str(f2) not in argv:
            return E2EResult(Status.FAIL, "Spaced {paths} were split or missing from argv", {"argv": argv})
        return E2EResult(Status.PASS, "{paths} expanded into exact unsplit argv entries")


def _t1_f9_04_floating_presentation_wrapping(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-actions-floating"):
        return pending
    with SandboxEnv("T1-F9-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        mocks.install_omarchy_floating_presentation()
        mocks.install_omarchy_transcode()
        _seed_sample_actions(ctx, sb)
        (sb.work / "clip.mp4").write_bytes(b"\x00\x00")

        rc, out, err, report = run_gall_selftest(
            ctx,
            sb,
            "e2e-actions-floating",
            cli_args=[sb.work],
            env_overrides={"GALL_E2E_CHAIN_MOCKS": "1"},
        )
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-actions-floating", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-actions-floating not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-actions-floating failed (rc={rc})", {"stderr": err})
        if failures := parse_selftest_failures(report):
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        calls = read_mock_calls(sb, "omarchy-launch-floating-terminal-with-presentation")
        if not calls or not any("omarchy-transcode" in " ".join(c.get("argv", [])) for c in calls):
            return E2EResult(Status.FAIL, "floating-presentation wrapper not called with omarchy-transcode", {"calls": calls})
        return E2EResult(Status.PASS, "floating-presentation wrapped omarchy-transcode invocation cleanly")


def _t1_f9_05_zoxide_quick_open(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-zoxide-quickopen"):
        return pending
    with SandboxEnv("T1-F9-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        z_alpha = sb.work / "z-alpha"
        z_beta = sb.work / "z-beta"
        z_alpha.mkdir()
        z_beta.mkdir()
        mocks.install_zoxide([z_alpha, z_beta])

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-zoxide-quickopen", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-zoxide-quickopen", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-zoxide-quickopen not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-zoxide-quickopen failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        if not read_mock_calls(sb, "zoxide"):
            return E2EResult(Status.FAIL, "zoxide query -l was not invoked")
        if str(z_alpha) not in (report + "\n" + log):
            return E2EResult(Status.FAIL, "Quick Open did not navigate to zoxide z-alpha directory", {"report": report})
        return E2EResult(Status.PASS, "zoxide query -l entries surfaced in Quick Open and navigated on Enter")


def _t1_f9_06_omarchy_cmd_terminal_cwd(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-terminal-cwd"):
        return pending
    with SandboxEnv("T1-F9-06", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        mocks = MockRegistry(sb)
        term_cwd = sb.work / "term-cwd"
        term_cwd.mkdir()
        mocks.install_omarchy_cmd_terminal_cwd(term_cwd)

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-terminal-cwd", cli_args=[sb.work])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-terminal-cwd", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-terminal-cwd not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-terminal-cwd failed (rc={rc})", {"stderr": err})
        if failures := parse_selftest_failures(report):
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})
        calls = read_mock_calls(sb, "omarchy-cmd-terminal-cwd")
        if not calls:
            return E2EResult(Status.FAIL, "omarchy-cmd-terminal-cwd was not invoked")
        if str(term_cwd) not in (report + "\n" + log):
            return E2EResult(Status.FAIL, "Gall did not navigate to terminal CWD returned by omarchy-cmd-terminal-cwd", {"report": report})
        return E2EResult(Status.PASS, "omarchy-cmd-terminal-cwd navigated Gall to active terminal folder")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase(
            id="T1-F9-01",
            tier=1,
            features=("F9",),
            title="actions.d schema & whenCommand filters string/array commands on PATH",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f9_01_when_command_filtering,
        ),
        E2ECase(
            id="T1-F9-02",
            tier=1,
            features=("F9",),
            title="actions.d filters by selection, single=true, pluralLabel, mimePrefixes, and extensions",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f9_02_selection_mime_extension_filtering,
        ),
        E2ECase(
            id="T1-F9-03",
            tier=1,
            features=("F9",),
            title="actions.d {path}/{paths}/{dir} substitution preserves filenames with spaces",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f9_03_placeholder_substitution_no_word_split,
        ),
        E2ECase(
            id="T1-F9-04",
            tier=1,
            features=("F9",),
            title="terminal='floating-presentation' wraps command via omarchy-launch-floating-terminal-with-presentation",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f9_04_floating_presentation_wrapping,
        ),
        E2ECase(
            id="T1-F9-05",
            tier=1,
            features=("F9",),
            title="zoxide query -l directories integrate into Quick Open",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f9_05_zoxide_quick_open,
        ),
        E2ECase(
            id="T1-F9-06",
            tier=1,
            features=("F9",),
            title="omarchy-cmd-terminal-cwd navigates active browser to terminal folder",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f9_06_omarchy_cmd_terminal_cwd,
        ),
    ]
