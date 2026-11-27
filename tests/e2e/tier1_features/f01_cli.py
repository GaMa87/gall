"""Tier 1 E2E tests for F1: CLI contract & multi-window startup."""

from __future__ import annotations

import time
from urllib.parse import quote

from harness import E2ECase, E2EResult, RunContext, SandboxEnv, Status

from . import (
    cli_flag_missing_in_help,
    count_gall_pids,
    log_reports_unknown_arg,
    parse_selftest_failures,
    read_gall_log,
    read_gall_session,
    require_selftest_mode,
    run_gall_selftest,
    selftest_mode_pending,
    wait_for_gall_log_contains,
)


def _t1_f1_01_positional_dual_pane(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-cli-state"):
        return pending
    with SandboxEnv("T1-F1-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        dir_a = sb.work / "dirA"
        dir_b = sb.work / "dirB"
        dir_a.mkdir()
        dir_b.mkdir()
        (dir_a / "alpha.txt").write_text("alpha\n", encoding="utf-8")
        (dir_b / "beta.txt").write_text("beta\n", encoding="utf-8")

        rc, out, err, report = run_gall_selftest(
            ctx,
            sb,
            "e2e-cli-state",
            cli_args=[dir_a, "-p2", dir_b],
        )
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-cli-state", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-cli-state not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"gall exited {rc}", {"stderr": err, "log": log[-1000:]})

        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        combined = report + "\n" + log
        if str(dir_a) not in combined or str(dir_b) not in combined:
            return E2EResult(Status.FAIL, "Expected both dirA and dirB to be opened in dual panes", {"report": report})
        return E2EResult(Status.PASS, "Positional dir and -p2 opened dirA and dirB cleanly")


def _t1_f1_02_select_and_positional_file(ctx: RunContext) -> E2EResult:
    if cli_flag_missing_in_help(ctx, "--select"):
        return E2EResult(Status.PENDING_IMPL, "--select flag not listed in gall --help yet")
    if pending := require_selftest_mode(ctx, "e2e-cli-state"):
        return pending

    with SandboxEnv("T1-F1-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        dir_a = sb.work / "dirA"
        dir_a.mkdir()
        pick = dir_a / "pick.txt"
        pick.write_text("pick me\n", encoding="utf-8")
        (dir_a / "other.txt").write_text("other\n", encoding="utf-8")

        for label, argv in (("explicit --select", ["--select", pick]), ("positional file", [pick])):
            rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-cli-state", cli_args=argv)
            log = read_gall_log(sb)
            if log_reports_unknown_arg(log, err, "--select"):
                return E2EResult(Status.PENDING_IMPL, "--select unrecognized by binary")
            if selftest_mode_pending(ctx, "--e2e-cli-state", report, log) and rc == 0 and not report:
                return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-cli-state not yet registered")
            if rc != 0:
                return E2EResult(Status.FAIL, f"{label} failed (rc={rc})", {"stderr": err, "log": log[-1000:]})
            failures = parse_selftest_failures(report)
            if failures:
                return E2EResult(Status.FAIL, f"{label} selftest failures: {failures}", {"report": report})
            if "ENOTDIR" in (err + log):
                return E2EResult(Status.FAIL, f"{label} errored with ENOTDIR", {"log": log[-1000:]})
            combined = report + "\n" + log
            if str(dir_a) not in combined or "pick.txt" not in combined:
                return E2EResult(Status.FAIL, f"{label} did not open parent and select pick.txt", {"report": report})

        return E2EResult(Status.PASS, "Both --select <file> and positional file opened parent and selected pick.txt")


def _t1_f1_03_file_uri_decoding(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-cli-state"):
        return pending
    with SandboxEnv("T1-F1-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        spaced_dir = sb.work / "dir one"
        spaced_dir.mkdir()
        target = spaced_dir / "hello world.txt"
        target.write_text("uri decoded\n", encoding="utf-8")

        encoded = quote(str(target), safe="/")
        for uri in (f"file://{encoded}", f"file://localhost{encoded}"):
            rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-cli-state", cli_args=[uri])
            log = read_gall_log(sb)
            if selftest_mode_pending(ctx, "--e2e-cli-state", report, log) and rc == 0 and not report:
                return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-cli-state not yet registered")
            if rc != 0:
                return E2EResult(Status.FAIL, f"URI launch failed for {uri} (rc={rc})", {"stderr": err})
            failures = parse_selftest_failures(report)
            if failures:
                return E2EResult(Status.FAIL, f"URI selftest failures: {failures}", {"report": report})
            combined = report + "\n" + log
            if "dir one" not in combined or "hello world.txt" not in combined:
                return E2EResult(Status.FAIL, f"URI {uri} was not decoded to dir one / hello world.txt", {"report": report})

        return E2EResult(Status.PASS, "file:// and file://localhost/ URIs decoded %20 spaces and selected target")


def _t1_f1_04_new_window_single_process(ctx: RunContext) -> E2EResult:
    assert ctx.binary_path is not None
    if cli_flag_missing_in_help(ctx, "--new-window"):
        return E2EResult(Status.PENDING_IMPL, "--new-window flag not listed in gall --help yet")

    # multi_instance=False so single-instance IPC routes --new-window to the primary process
    with SandboxEnv("T1-F1-04", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        dir_a = sb.work / "dirA"
        dir_b = sb.work / "dirB"
        dir_a.mkdir()
        dir_b.mkdir()

        primary = sb.popen([str(ctx.binary_path), str(dir_a)])
        if not wait_for_gall_log_contains(sb, "window shown", timeout=8.0):
            time.sleep(1.0)
        if primary.poll() is not None:
            return E2EResult(
                Status.FAIL,
                f"Primary gall exited prematurely (rc={primary.returncode})",
                {"log": read_gall_log(sb)[-1000:]},
            )

        t0 = time.monotonic()
        second = sb.run([str(ctx.binary_path), "--new-window", str(dir_b)], timeout=5.0)
        elapsed = time.monotonic() - t0
        log = read_gall_log(sb)
        if log_reports_unknown_arg(log, second.stderr or "", "--new-window"):
            return E2EResult(Status.PENDING_IMPL, "--new-window unrecognized by binary")
        if second.returncode != 0:
            return E2EResult(
                Status.FAIL,
                f"Second --new-window launch exited {second.returncode}",
                {"stderr": second.stderr, "log": log[-1000:]},
            )
        if elapsed > 4.0:
            return E2EResult(Status.FAIL, f"Second --new-window launch took {elapsed:.2f}s (>4s)")

        time.sleep(0.8)
        if primary.poll() is not None:
            return E2EResult(Status.FAIL, "Primary gall crashed after --new-window IPC")

        pids = count_gall_pids(sb)
        if len(pids) != 1:
            return E2EResult(
                Status.FAIL,
                f"Expected exactly 1 Gall main process in sandbox, found PIDs {pids}",
            )

        log = read_gall_log(sb)
        session = read_gall_session(sb)
        if "win-" not in log and (not session or len(str(session)) < 10):
            return E2EResult(
                Status.FAIL,
                "Expected win-* window creation in Gall.log or multi-window session state",
                {"log": log[-1000:]},
            )
        return E2EResult(Status.PASS, f"--new-window forwarded in {elapsed:.2f}s to single primary process (PID {pids[0]})")


def _t1_f1_05_compare_cli(ctx: RunContext) -> E2EResult:
    if cli_flag_missing_in_help(ctx, "--compare"):
        return E2EResult(Status.PENDING_IMPL, "--compare flag not listed in gall --help yet")
    if pending := require_selftest_mode(ctx, "e2e-cli-state"):
        return pending

    with SandboxEnv("T1-F1-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        fa = sb.work / "a.txt"
        fb = sb.work / "b.txt"
        fa.write_text("line 1\nline 2\n", encoding="utf-8")
        fb.write_text("line 1\nline 2 modified\n", encoding="utf-8")

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-cli-state", cli_args=["--compare", fa, fb])
        log = read_gall_log(sb)
        if log_reports_unknown_arg(log, err, "--compare"):
            return E2EResult(Status.PENDING_IMPL, "--compare unrecognized by binary")
        if selftest_mode_pending(ctx, "--e2e-cli-state", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-cli-state not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"--compare exited {rc}", {"stderr": err, "log": log[-1000:]})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"--compare selftest failures: {failures}", {"report": report})
        combined = report + "\n" + log
        if "compare" not in combined.lower() or "a.txt" not in combined or "b.txt" not in combined:
            return E2EResult(Status.FAIL, "Compare view did not load a.txt and b.txt", {"report": report})
        return E2EResult(Status.PASS, "--compare opened Compare view with both files")


def _t1_f1_06_git_and_daemon(ctx: RunContext) -> E2EResult:
    if cli_flag_missing_in_help(ctx, "--git") or cli_flag_missing_in_help(ctx, "--daemon"):
        return E2EResult(Status.PENDING_IMPL, "--git or --daemon flag not listed in gall --help yet")
    if pending := require_selftest_mode(ctx, "e2e-cli-state"):
        return pending

    with SandboxEnv("T1-F1-06", ctx.env_probe, allow_gui=True, multi_instance=False, dbus_required=True) as sb:
        repo = sb.work / "repo"
        repo.mkdir()
        sb.run(["git", "init"], cwd=repo, check=True)
        sb.run(["git", "config", "user.email", "e2e@example.com"], cwd=repo, check=True)
        sb.run(["git", "config", "user.name", "E2E"], cwd=repo, check=True)
        (repo / "README.md").write_text("hello\n", encoding="utf-8")
        sb.run(["git", "add", "README.md"], cwd=repo, check=True)
        sb.run(["git", "commit", "-m", "init"], cwd=repo, check=True)

        rc, out, err, report = run_gall_selftest(
            ctx,
            sb,
            "e2e-cli-state",
            cli_args=["--git", repo],
            env_overrides={"GALL_MULTI_INSTANCE": "1"},
        )
        log = read_gall_log(sb)
        if log_reports_unknown_arg(log, err, "--git"):
            return E2EResult(Status.PENDING_IMPL, "--git unrecognized by binary")
        if selftest_mode_pending(ctx, "--e2e-cli-state", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-cli-state not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"--git exited {rc}", {"stderr": err, "log": log[-1000:]})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"--git selftest failures: {failures}", {"report": report})
        if "git" not in (report + "\n" + log).lower():
            return E2EResult(Status.FAIL, "--git did not open Git workspace", {"report": report})

        # Now test --daemon stays resident without exiting or mapping an initial window
        assert ctx.binary_path is not None
        log_offset = len(read_gall_log(sb))
        daemon_proc = sb.popen([str(ctx.binary_path), "--daemon"])
        time.sleep(1.5)
        if daemon_proc.poll() is not None:
            d_log = read_gall_log(sb)[log_offset:]
            if log_reports_unknown_arg(d_log, "", "--daemon"):
                return E2EResult(Status.PENDING_IMPL, "--daemon unrecognized by binary")
            return E2EResult(Status.FAIL, f"--daemon exited prematurely (rc={daemon_proc.returncode})", {"log": d_log[-1000:]})

        idle_log = read_gall_log(sb)[log_offset:]
        if "window shown at" in idle_log:
            return E2EResult(Status.FAIL, "--daemon mapped a window while idle", {"log": idle_log[-1000:]})

        wake_res = sb.run([str(ctx.binary_path), str(repo)], timeout=5.0)
        if wake_res.returncode != 0:
            return E2EResult(Status.FAIL, f"Second launch against --daemon failed (rc={wake_res.returncode})")
        if not wait_for_gall_log_contains(sb, "window shown", timeout=8.0, start_offset=log_offset):
            return E2EResult(Status.FAIL, "Follow-up launch against --daemon did not map a window")
        return E2EResult(Status.PASS, "--git opened Git workspace and --daemon stayed resident for follow-up launch")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase(
            id="T1-F1-01",
            tier=1,
            features=("F1",),
            title="Positional directory and -p2 dual pane opening",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f1_01_positional_dual_pane,
        ),
        E2ECase(
            id="T1-F1-02",
            tier=1,
            features=("F1",),
            title="--select <file> and positional file open parent with item selected",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f1_02_select_and_positional_file,
        ),
        E2ECase(
            id="T1-F1-03",
            tier=1,
            features=("F1",),
            title="file:// and file://localhost/ URI decoding with spaces",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f1_03_file_uri_decoding,
        ),
        E2ECase(
            id="T1-F1-04",
            tier=1,
            features=("F1",),
            title="--new-window creates second window in single primary process",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f1_04_new_window_single_process,
        ),
        E2ECase(
            id="T1-F1-05",
            tier=1,
            features=("F1",),
            title="--compare <file1> <file2> launches Compare view",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f1_05_compare_cli,
        ),
        E2ECase(
            id="T1-F1-06",
            tier=1,
            features=("F1",),
            title="--git <repo> opens Git workspace and --daemon stays resident",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f1_06_git_and_daemon,
        ),
    ]
