"""Tier 2 boundary & negative E2E cases for Feature F1 (CLI & multi-window)."""

from __future__ import annotations

import json
import time
import urllib.parse

from harness.sandbox import SandboxEnv
from harness.types import E2ECase, E2EResult, RunContext, Status

from . import (
    assert_unknown_arg_not_ignored,
    check_selftest_outcome,
    count_gall_pids,
    find_session_files,
    read_gall_logs,
    require_cli_flags,
    require_selftest_mode,
    run_gall_selftest,
    wait_for_dbus_name,
    wait_for_log_contains,
)


def _t2_f1_01_uri_edge_chars(ctx: RunContext) -> E2EResult:
    """file:// URI with #, ?, %25, and UTF-8 %C4%8D must decode literally without double-decode or truncation."""
    if pending := require_cli_flags(ctx, "--select"):
        return pending
    if pending := require_selftest_mode(ctx, "e2e-cli-state"):
        return pending
    with SandboxEnv("T2-F1-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        target = sb.work / "a b#1%2 č.txt"
        target.write_text("uri-edge\n", encoding="utf-8")
        encoded = urllib.parse.quote(str(target), safe="/")
        uri = f"file://{encoded}"

        proc, report, _ = run_gall_selftest(ctx, sb, "e2e-cli-state", extra_args=["--select", uri])
        if pending := assert_unknown_arg_not_ignored(sb, proc, "--select"):
            return pending
        if fail := check_selftest_outcome(proc, report, "e2e-cli-state", sb=sb):
            return fail
        combined = report + "\n" + read_gall_logs(sb)
        if target.name not in combined:
            return E2EResult(
                Status.FAIL,
                f"Decoded filename {target.name!r} missing from CLI state/logs for URI {uri}",
                {"report": report[-1000:]},
            )
        return E2EResult(Status.PASS, "file:// URI with #, %25, and UTF-8 decoded literally", {"uri": uri})


def _t2_f1_02_relative_second_instance_cwd(ctx: RunContext) -> E2EResult:
    """Relative path in second instance must resolve against second instance cwd, not primary cwd."""
    if pending := require_cli_flags(ctx, "--new-window"):
        return pending
    assert ctx.binary_path is not None
    with SandboxEnv("T2-F1-02", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        cwd1 = sb.work / "cwd1"
        cwd2 = sb.work / "cwd2" / "sub"
        cwd1.mkdir(parents=True)
        cwd2.mkdir(parents=True)
        (cwd1 / "sub").mkdir()
        wrong_target = cwd1 / "sub" / "target.txt"
        wrong_target.write_text("wrong\n", encoding="utf-8")
        right_target = cwd2 / "target.txt"
        right_target.write_text("right\n", encoding="utf-8")

        primary = sb.popen([ctx.binary_path, str(cwd1)], cwd=cwd1)
        wait_for_log_contains(sb, "window shown at", timeout=6.0)
        if primary.poll() is not None:
            return E2EResult(Status.FAIL, f"Primary Gall instance exited prematurely (rc={primary.returncode})")

        second = sb.run([ctx.binary_path, "./sub/target.txt"], cwd=sb.work / "cwd2", timeout=15.0)
        if second.returncode != 0:
            return E2EResult(
                Status.FAIL,
                f"Second-instance forwarding exited non-zero (rc={second.returncode})",
                {"stderr": second.stderr},
            )
        pids = count_gall_pids(sb)
        if len(pids) != 1:
            return E2EResult(Status.FAIL, f"Expected single Gall process after forwarding, found {pids}")

        wait_for_log_contains(sb, str(right_target), timeout=4.0)
        logs = read_gall_logs(sb)
        session_blob = "\n".join(sf.read_text(encoding="utf-8", errors="replace") for sf in find_session_files(sb))
        combined = logs + "\n" + session_blob
        if str(wrong_target) in combined and str(right_target) not in combined:
            return E2EResult(Status.FAIL, "Second instance resolved relative path against primary cwd instead of its own cwd")
        if str(right_target) not in combined and str(cwd2) not in combined:
            return E2EResult(
                Status.FAIL,
                "Expected second-instance cwd target in Gall.log or session.json",
                {"log": logs[-1000:]},
            )
        return E2EResult(Status.PASS, "Second instance forwarded relative path against caller cwd in single process")


def _t2_f1_03_missing_paths_no_crash(ctx: RunContext) -> E2EResult:
    """Non-existent path and missing --select target must fall back gracefully without panic or hang."""
    if pending := require_cli_flags(ctx, "--select"):
        return pending
    if pending := require_selftest_mode(ctx, "e2e-cli-state"):
        return pending
    with SandboxEnv("T2-F1-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        existing_dir = sb.work / "dirA"
        existing_dir.mkdir()
        missing_file = existing_dir / "does_not_exist.txt"

        p1, r1, _ = run_gall_selftest(ctx, sb, "e2e-cli-state", extra_args=["--select", str(missing_file)])
        if pending := assert_unknown_arg_not_ignored(sb, p1, "--select"):
            return pending
        if fail := check_selftest_outcome(p1, r1, "e2e-cli-state", sb=sb):
            return fail
        if str(existing_dir) not in (r1 + "\n" + read_gall_logs(sb)):
            return E2EResult(
                Status.FAIL,
                "Missing --select target did not fall back to existing parent directory dirA",
                {"report": r1[-1000:]},
            )

        p2, r2, _ = run_gall_selftest(ctx, sb, "e2e-cli-state", extra_args=[str(sb.work / "no_such_dir")])
        if fail := check_selftest_outcome(p2, r2, "e2e-cli-state", sb=sb):
            return fail
        return E2EResult(Status.PASS, "Missing --select target and non-existent directory handled gracefully")


def _t2_f1_04_multi_select_new_window(ctx: RunContext) -> E2EResult:
    """--new-window combined with multiple --select flags must forward all items to one process."""
    if pending := require_cli_flags(ctx, "--new-window", "--select"):
        return pending
    assert ctx.binary_path is not None
    with SandboxEnv("T2-F1-04", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        dir_a = sb.work / "dirA"
        dir_a.mkdir()
        f1 = dir_a / "f1.txt"
        f2 = dir_a / "f2.txt"
        f1.write_text("1\n", encoding="utf-8")
        f2.write_text("2\n", encoding="utf-8")

        primary = sb.popen([ctx.binary_path, str(sb.work)], cwd=sb.work)
        wait_for_log_contains(sb, "window shown at", timeout=6.0)
        if primary.poll() is not None:
            return E2EResult(Status.FAIL, f"Primary exited early (rc={primary.returncode})")

        res = sb.run(
            [ctx.binary_path, "--new-window", "--select", str(f1), "--select", str(f2)],
            timeout=15.0,
        )
        if pending := assert_unknown_arg_not_ignored(sb, res, "--new-window"):
            return pending
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"--new-window --select failed (rc={res.returncode})", {"stderr": res.stderr})
        pids = count_gall_pids(sb)
        if len(pids) != 1:
            return E2EResult(Status.FAIL, f"Expected single process for multi-window, found {pids}")

        wait_for_log_contains(sb, "f2.txt", timeout=4.0)
        logs = read_gall_logs(sb)
        session_blob = "\n".join(sf.read_text(encoding="utf-8", errors="replace") for sf in find_session_files(sb))
        combined = logs + "\n" + session_blob
        if "f1.txt" not in combined or "f2.txt" not in combined:
            return E2EResult(
                Status.FAIL,
                "Multi --select items f1.txt/f2.txt were not preserved on --new-window",
                {"log": logs[-1000:]},
            )
        return E2EResult(Status.PASS, "Multi --select + --new-window succeeded in single process")


def _t2_f1_05_compare_bad_args(ctx: RunContext) -> E2EResult:
    """--compare with 1 arg exits non-zero cleanly; --compare with missing second file degrades safely."""
    if pending := require_cli_flags(ctx, "--compare"):
        return pending
    if pending := require_selftest_mode(ctx, "e2e-cli-state"):
        return pending
    assert ctx.binary_path is not None
    with SandboxEnv("T2-F1-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        only_one = sb.work / "only_one.txt"
        only_one.write_text("a\n", encoding="utf-8")

        r1 = sb.run([ctx.binary_path, "--compare", str(only_one)], timeout=10.0)
        if pending := assert_unknown_arg_not_ignored(sb, r1, "--compare"):
            return pending
        if "panicked at" in (r1.stderr or "") or r1.returncode < 0:
            return E2EResult(Status.FAIL, "Gall panicked/signaled on single-arg --compare", {"rc": r1.returncode, "stderr": r1.stderr})
        if r1.returncode == 0:
            return E2EResult(Status.FAIL, "Single-arg --compare unexpectedly exited 0 instead of reporting CLI arity error")
        if not (r1.stderr or "").strip():
            return E2EResult(Status.FAIL, "Single-arg --compare exited non-zero without stderr diagnostic")

        p2, r2, _ = run_gall_selftest(
            ctx,
            sb,
            "e2e-cli-state",
            extra_args=["--compare", str(only_one), str(sb.work / "missing.txt")],
        )
        if fail := check_selftest_outcome(p2, r2, "e2e-cli-state", sb=sb):
            return fail
        combined = r2 + "\n" + read_gall_logs(sb)
        if "compare" not in combined.lower() or "only_one.txt" not in combined:
            return E2EResult(
                Status.FAIL,
                "e2e-cli-state report did not reflect --compare target only_one.txt",
                {"report": r2[-1000:]},
            )
        return E2EResult(Status.PASS, "Invalid --compare argument combinations handled cleanly")


def _t2_f1_06_daemon_to_windows_lifecycle(ctx: RunContext) -> E2EResult:
    """--daemon -> normal launch -> --new-window transitions cleanly with valid session.json."""
    if pending := require_cli_flags(ctx, "--daemon", "--new-window"):
        return pending
    assert ctx.binary_path is not None
    with SandboxEnv("T2-F1-06", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        dir_a = sb.work / "dirA"
        dir_b = sb.work / "dirB"
        dir_a.mkdir()
        dir_b.mkdir()

        daemon = sb.popen([ctx.binary_path, "--daemon"], cwd=sb.work)
        time.sleep(1.2)
        if daemon.poll() is not None:
            return E2EResult(
                Status.FAIL,
                f"gall --daemon exited early (rc={daemon.returncode})",
                {"stderr": daemon.stderr.read().decode("utf-8", errors="replace") if daemon.stderr else ""},
            )
        if not wait_for_dbus_name(sb, "org.freedesktop.FileManager1", timeout=6.0):
            return E2EResult(Status.PENDING_IMPL, "org.freedesktop.FileManager1 not registered by --daemon (M3 pending)")
        if "window shown at" in read_gall_logs(sb):
            return E2EResult(Status.FAIL, "gall --daemon unexpectedly mapped a window while idle")

        r1 = sb.run([ctx.binary_path, str(dir_a)], timeout=15.0)
        r2 = sb.run([ctx.binary_path, "--new-window", str(dir_b)], timeout=15.0)
        if r1.returncode != 0 or r2.returncode != 0:
            return E2EResult(Status.FAIL, f"Window open on --daemon failed (rc1={r1.returncode}, rc2={r2.returncode})")

        if not wait_for_log_contains(sb, "window shown at", timeout=6.0):
            return E2EResult(Status.FAIL, "Daemon did not map window after follow-up launch", {"log": read_gall_logs(sb)[-1000:]})

        pids = count_gall_pids(sb)
        if len(pids) != 1:
            return E2EResult(Status.FAIL, f"Expected single daemon PID, found {pids}")

        for sf in find_session_files(sb):
            try:
                json.loads(sf.read_text(encoding="utf-8"))
            except json.JSONDecodeError as exc:
                return E2EResult(Status.FAIL, f"Corrupt session.json at {sf}: {exc}")

        return E2EResult(Status.PASS, "--daemon -> normal -> --new-window lifecycle maintained single PID and valid session")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase("T2-F1-01", 2, ("F1",), "CLI file:// URI edge chars (#, ?, %25, UTF-8)", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f1_01_uri_edge_chars),
        E2ECase("T2-F1-02", 2, ("F1",), "CLI second-instance relative path resolves against caller cwd", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f1_02_relative_second_instance_cwd),
        E2ECase("T2-F1-03", 2, ("F1",), "CLI non-existent path and missing --select target", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f1_03_missing_paths_no_crash),
        E2ECase("T2-F1-04", 2, ("F1",), "CLI multi --select combined with --new-window", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f1_04_multi_select_new_window),
        E2ECase("T2-F1-05", 2, ("F1",), "CLI --compare with 1 arg or missing second file", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f1_05_compare_bad_args),
        E2ECase("T2-F1-06", 2, ("F1",), "CLI --daemon -> window -> --new-window lifecycle", requires_binary=True, requires_gui=True, requires_dbus=True, fn=_t2_f1_06_daemon_to_windows_lifecycle),
    ]
