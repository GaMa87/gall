"""Tier 1 E2E tests for F3: Wayland/Linux file clipboard wire formats."""

from __future__ import annotations

from urllib.parse import quote

from harness import E2ECase, E2EResult, RunContext, SandboxEnv, Status

from . import (
    ClipboardOwnerUnavailable,
    parse_selftest_failures,
    read_gall_log,
    require_selftest_mode,
    run_gall_selftest,
    selftest_mode_pending,
    start_gtk_clipboard_owner,
)


def _file_uri(path: object) -> str:
    return "file://" + quote(str(path), safe="/")


def _stop_proc(proc: object) -> None:
    if hasattr(proc, "poll") and proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=2.0)
        except Exception:
            proc.kill()


def _t1_f3_01_multi_mime_offer_on_copy(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-clip-copy"):
        return pending
    with SandboxEnv("T1-F3-01", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        src = sb.work / "src"
        src.mkdir()
        a_file = src / "a.txt"
        a_file.write_text("hello\n", encoding="utf-8")

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-clip-copy", cli_args=[src])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-clip-copy", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-clip-copy not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-clip-copy failed (rc={rc})", {"stderr": err, "log": log[-1000:]})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        combined = report + "\n" + log
        if "x-special/gnome-copied-files" not in combined or "text/uri-list" not in combined:
            return E2EResult(Status.FAIL, "Report missing required file MIME targets", {"report": report})
        if "text/plain" not in combined:
            return E2EResult(Status.FAIL, "Report missing text/plain fallback MIME target", {"report": report})
        return E2EResult(Status.PASS, "Copy offers gnome-copied-files, text/uri-list, and text/plain simultaneously")


def _t1_f3_02_copy_wire_format_crlf(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-clip-copy"):
        return pending
    with SandboxEnv("T1-F3-02", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        src = sb.work / "src"
        src.mkdir()
        fa = src / "a.txt"
        fb = src / "b.txt"
        fa.write_text("a\n", encoding="utf-8")
        fb.write_text("b\n", encoding="utf-8")

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-clip-copy", cli_args=[src])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-clip-copy", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-clip-copy not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-clip-copy failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        combined = report + "\n" + log
        if "copy" not in combined.lower() or "file://" not in combined:
            return E2EResult(Status.FAIL, "gnome-copied-files wire format missing copy/file://", {"report": report})
        if "crlf" not in combined.lower() and "\\r\\n" not in combined and "\r\n" not in combined:
            return E2EResult(Status.FAIL, "text/uri-list CRLF wire format not verified in report", {"report": report})
        if str(fa) not in combined and "a.txt" not in combined:
            return E2EResult(Status.FAIL, "text/plain path missing copied item", {"report": report})
        return E2EResult(Status.PASS, "Exact wire formats verified for gnome-copied-files, CRLF text/uri-list, and text/plain")


def _t1_f3_03_cut_payload_and_move(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-clip-cut-paste"):
        return pending
    with SandboxEnv("T1-F3-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        src = sb.work / "src"
        dst = sb.work / "dst"
        src.mkdir()
        dst.mkdir()
        cutme = src / "cutme.txt"
        cutme.write_text("move me\n", encoding="utf-8")

        rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-clip-cut-paste", cli_args=[src, "-p2", dst])
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-clip-cut-paste", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-clip-cut-paste not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"e2e-clip-cut-paste failed (rc={rc})", {"stderr": err})
        failures = parse_selftest_failures(report)
        if failures:
            return E2EResult(Status.FAIL, f"Selftest failures: {failures}", {"report": report})

        if not (dst / "cutme.txt").is_file() or cutme.exists():
            return E2EResult(Status.FAIL, "Cut + paste did not move cutme.txt from src to dst")
        return E2EResult(Status.PASS, "Cut payload used cut\\n prefix and Paste moved cutme.txt to dst")


def _t1_f3_04_external_gnome_copied_files_paste(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-clip-paste"):
        return pending
    with SandboxEnv("T1-F3-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        src = sb.work / "src"
        dst = sb.work / "dst"
        src.mkdir()
        dst.mkdir()
        ext = src / "ext.txt"
        ext_cut = src / "ext_cut.txt"
        ext.write_text("external copy\n", encoding="utf-8")
        ext_cut.write_text("external cut\n", encoding="utf-8")

        try:
            owner1 = start_gtk_clipboard_owner(
                sb,
                {"x-special/gnome-copied-files": f"copy\n{_file_uri(ext)}\n".encode("utf-8")},
            )
        except ClipboardOwnerUnavailable as exc:
            return E2EResult(Status.ENV_UNAVAILABLE, str(exc))
        try:
            rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-clip-paste", cli_args=[dst])
        finally:
            _stop_proc(owner1)
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-clip-paste", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-clip-paste not yet registered")
        if rc != 0:
            return E2EResult(Status.FAIL, f"External copy paste failed (rc={rc})", {"stderr": err})
        if failures := parse_selftest_failures(report):
            return E2EResult(Status.FAIL, f"External copy paste selftest failures: {failures}", {"report": report})
        if not (dst / "ext.txt").is_file() or not ext.is_file():
            return E2EResult(Status.FAIL, "External gnome-copied-files copy did not preserve src and copy to dst")

        try:
            owner2 = start_gtk_clipboard_owner(
                sb,
                {"x-special/gnome-copied-files": f"cut\n{_file_uri(ext_cut)}\n".encode("utf-8")},
            )
        except ClipboardOwnerUnavailable as exc:
            return E2EResult(Status.ENV_UNAVAILABLE, str(exc))
        try:
            rc2, out2, err2, report2 = run_gall_selftest(ctx, sb, "e2e-clip-paste", cli_args=[dst])
        finally:
            _stop_proc(owner2)
        if rc2 != 0:
            return E2EResult(Status.FAIL, f"External cut paste failed (rc={rc2})", {"stderr": err2})
        if failures2 := parse_selftest_failures(report2):
            return E2EResult(Status.FAIL, f"External cut paste selftest failures: {failures2}", {"report": report2})
        if not (dst / "ext_cut.txt").is_file() or ext_cut.exists():
            return E2EResult(Status.FAIL, "External gnome-copied-files cut did not move ext_cut.txt to dst")
        return E2EResult(Status.PASS, "External x-special/gnome-copied-files copy and cut pasted accurately")


def _t1_f3_05_fallback_uri_list_and_plain(ctx: RunContext) -> E2EResult:
    if pending := require_selftest_mode(ctx, "e2e-clip-paste"):
        return pending
    with SandboxEnv("T1-F3-05", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        src = sb.work / "src"
        dst = sb.work / "dst"
        src.mkdir()
        dst.mkdir()
        u_file = src / "u.txt"
        lf_file = src / "lf.txt"
        p_file = src / "p.txt"
        u_file.write_text("uri\n", encoding="utf-8")
        lf_file.write_text("lf uri\n", encoding="utf-8")
        p_file.write_text("plain\n", encoding="utf-8")

        # Verify both CRLF and LF-only text/uri-list payloads (e.g. omarchy-transcode)
        try:
            owner1 = start_gtk_clipboard_owner(
                sb,
                {"text/uri-list": f"{_file_uri(u_file)}\r\n{_file_uri(lf_file)}\n".encode("utf-8")},
            )
        except ClipboardOwnerUnavailable as exc:
            return E2EResult(Status.ENV_UNAVAILABLE, str(exc))
        try:
            rc, out, err, report = run_gall_selftest(ctx, sb, "e2e-clip-paste", cli_args=[dst])
        finally:
            _stop_proc(owner1)
        log = read_gall_log(sb)
        if selftest_mode_pending(ctx, "--e2e-clip-paste", report, log) and rc == 0 and not report:
            return E2EResult(Status.PENDING_IMPL, "Selftest mode e2e-clip-paste not yet registered")
        if rc != 0 or not (dst / "u.txt").is_file() or not (dst / "lf.txt").is_file():
            return E2EResult(Status.FAIL, "Fallback text/uri-list paste failed", {"stderr": err, "report": report})

        try:
            owner2 = start_gtk_clipboard_owner(
                sb,
                {"text/plain;charset=utf-8": f"{p_file}\n".encode("utf-8")},
            )
        except ClipboardOwnerUnavailable as exc:
            return E2EResult(Status.ENV_UNAVAILABLE, str(exc))
        try:
            rc2, out2, err2, report2 = run_gall_selftest(ctx, sb, "e2e-clip-paste", cli_args=[dst])
        finally:
            _stop_proc(owner2)
        if rc2 != 0 or not (dst / "p.txt").is_file():
            return E2EResult(Status.FAIL, "Fallback text/plain path paste failed", {"stderr": err2, "report": report2})
        return E2EResult(Status.PASS, "Both text/uri-list (CRLF+LF) and text/plain fallback clipboards pasted cleanly")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase(
            id="T1-F3-01",
            tier=1,
            features=("F3",),
            title="Copy offers gnome-copied-files, text/uri-list, and text/plain simultaneously",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f3_01_multi_mime_offer_on_copy,
        ),
        E2ECase(
            id="T1-F3-02",
            tier=1,
            features=("F3",),
            title="Copy wire format uses copy\\n, CRLF text/uri-list, and plain paths",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f3_02_copy_wire_format_crlf,
        ),
        E2ECase(
            id="T1-F3-03",
            tier=1,
            features=("F3",),
            title="Cut payload and Paste moves file instead of copying",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f3_03_cut_payload_and_move,
        ),
        E2ECase(
            id="T1-F3-04",
            tier=1,
            features=("F3",),
            title="External x-special/gnome-copied-files copy and cut paste into Gall",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f3_04_external_gnome_copied_files_paste,
        ),
        E2ECase(
            id="T1-F3-05",
            tier=1,
            features=("F3",),
            title="Fallback text/uri-list and text/plain path clipboard paste",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f3_05_fallback_uri_list_and_plain,
        ),
    ]
