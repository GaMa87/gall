"""Tier 2 boundary & negative E2E cases for Feature F11 (Gall vs. Nautilus benchmark)."""

from __future__ import annotations

import json
import os
import shutil
import time
from pathlib import Path
from typing import Any

from harness.assertions import read_mock_calls
from harness.mocks import MockRegistry
from harness.sandbox import SandboxEnv
from harness.types import E2ECase, E2EResult, RunContext, Status

from . import make_restricted_bin


def _bench_script(ctx: RunContext) -> Path:
    return ctx.repo_root / "scripts" / "benchmark-nautilus.sh"


def _install_mock_pacman(mocks: MockRegistry, pkg_db: dict[str, dict[str, Any] | int]) -> Path:
    db_path = mocks.sb.tmp / "pacman_pkg_db.json"
    db_path.write_text(json.dumps(pkg_db), encoding="utf-8")
    pacman_body = f"""#!/usr/bin/env python3
import json, sys
with open({str(db_path)!r}, "r", encoding="utf-8") as f:
    db = json.load(f)
args = [a for a in sys.argv[1:] if not a.startswith("-")]
if args and args[0] in db:
    print(args[0])
    raise SystemExit(0)
raise SystemExit(1)
"""
    return mocks.install_tool("pacman", script_body=pacman_body)


def _install_mock_expac(mocks: MockRegistry, pkg_db: dict[str, dict[str, Any] | int]) -> Path:
    db_path = mocks.sb.tmp / "expac_pkg_db.json"
    db_path.write_text(json.dumps(pkg_db), encoding="utf-8")
    expac_body = f"""#!/usr/bin/env python3
import json, sys
with open({str(db_path)!r}, "r", encoding="utf-8") as f:
    db = json.load(f)
args = sys.argv[1:]
fmt = "%n\\t%m\\t%P"
pkgs = []
i = 0
while i < len(args):
    a = args[i]
    if a in ("-Q", "-S"):
        i += 1
        continue
    if a.startswith("-") and i + 1 < len(args) and a in ("-s", "-d"):
        i += 2
        continue
    if a.startswith("-"):
        i += 1
        continue
    if "%" in a and not pkgs:
        fmt = a
    else:
        pkgs.append(a)
    i += 1

targets = pkgs if pkgs else sorted(db.keys())
for pkg in targets:
    entry = db.get(pkg)
    if entry is None:
        continue
    if isinstance(entry, int):
        size = entry
        prov = "None"
    else:
        size = int(entry.get("size", entry.get("install_size", entry.get("bytes", 0))))
        prov = " ".join(entry.get("provides", [])) or "None"
    line = fmt.replace("%n", pkg).replace("%m", str(size)).replace("%P", prov)
    print(line)
"""
    return mocks.install_tool("expac", script_body=expac_body)


def _install_synthetic_pkg_tools(mocks: MockRegistry) -> None:
    pkg_db: dict[str, dict[str, Any] | int] = {
        "nautilus": {"size": 1048576, "deps": ["gtk4", "gvfs"]},
        "nautilus-python": {"size": 524288, "deps": ["nautilus"]},
        "sushi": {"size": 524288, "deps": ["nautilus"]},
        "ffmpegthumbnailer": {"size": 524288, "deps": ["ffmpeg"]},
        "webkit2gtk-4.1": {"size": 2097152, "deps": ["gtk3"]},
        "gtk3": {"size": 1048576, "deps": []},
        "gtk4": {"size": 1048576, "deps": []},
        "gvfs": {"size": 524288, "deps": []},
        "ffmpeg": {"size": 1048576, "deps": []},
        "base": {"size": 1048576, "deps": []},
        "bash-completion": {"size": 131072, "deps": []},
        "btop": {"size": 262144, "deps": []},
        "expac": {"size": 65536, "deps": []},
        "fontconfig": {"size": 131072, "deps": []},
        "fzf": {"size": 262144, "deps": []},
        "git": {"size": 524288, "deps": []},
        "gvfs-mtp": {"size": 65536, "deps": ["gvfs"]},
        "gvfs-nfs": {"size": 65536, "deps": ["gvfs"]},
        "gvfs-smb": {"size": 65536, "deps": ["gvfs"]},
        "hyprland": {"size": 1048576, "deps": []},
        "jq": {"size": 131072, "deps": []},
        "localsend": {"size": 524288, "deps": []},
        "nvim": {"size": 524288, "deps": []},
        "pacman-contrib": {"size": 65536, "deps": []},
        "pinta": {"size": 262144, "deps": []},
        "ripgrep": {"size": 262144, "deps": []},
        "ttf-jetbrains-mono-nerd-basic": {"size": 262144, "deps": []},
        "uwsm": {"size": 65536, "deps": []},
        "wl-clipboard": {"size": 65536, "deps": []},
        "xdg-desktop-portal-gtk": {"size": 131072, "deps": []},
        "xdg-desktop-portal-hyprland": {"size": 131072, "deps": []},
        "xdg-terminal-exec": {"size": 32768, "deps": []},
        "zoxide": {"size": 131072, "deps": []},
        "gst-plugins-good": {"size": 524288, "deps": []},
    }
    mocks.install_pactree_expac(pkg_db)
    _install_mock_expac(mocks, pkg_db)
    _install_mock_pacman(mocks, pkg_db)


def _t2_f11_01_missing_tools_clear_diagnostic(ctx: RunContext) -> E2EResult:
    """Missing pactree/expac/nautilus or base.packages must report skipped diagnostic, never fake 0 MiB."""
    if not ctx.env_probe.benchmark_script_exists:
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh does not exist yet")

    script = _bench_script(ctx)
    with SandboxEnv("T2-F11-01", ctx.env_probe) as sb:
        rbin = make_restricted_bin(sb, exclude={"pactree", "expac", "pacman", "nautilus"})
        res = sb.run(
            [str(script), "--footprint-only", "--json"],
            env_overrides={"PATH": str(rbin)},
            timeout=20.0,
        )
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"benchmark-nautilus.sh failed when Arch tools absent (rc={res.returncode})", {"stderr": res.stderr})
        data = json.loads(res.stdout)
        fp = data.get("footprint", {})
        if fp.get("status") != "skipped" or "missing Arch package tools" not in fp.get("reason", ""):
            return E2EResult(Status.FAIL, f"Expected explicit skipped diagnostic when pactree/expac missing, got {fp}")

        mocks = MockRegistry(sb)
        _install_synthetic_pkg_tools(mocks)
        res2 = sb.run(
            [str(script), "--footprint-only", "--json"],
            env_overrides={"OMARCHY_BASE_PACKAGES": str(sb.tmp / "missing.packages")},
            timeout=20.0,
        )
        if res2.returncode != 0:
            return E2EResult(Status.FAIL, f"benchmark-nautilus.sh failed with missing base.packages (rc={res2.returncode})")
        data2 = json.loads(res2.stdout)
        delta = data2.get("footprint", {}).get("omarchy_iso_delta", {})
        if delta.get("status") != "skipped":
            return E2EResult(Status.FAIL, f"Expected skipped omarchy_iso_delta when file absent, got {delta}")
        return E2EResult(Status.PASS, "Missing pactree/expac/base.packages reported explicit skipped diagnostics")


def _t2_f11_02_runs_boundary_and_invalid_cli_args(ctx: RunContext) -> E2EResult:
    """--runs 1 succeeds cleanly while --runs 0, --runs abc, --runs -2, and bad --dir fail fast."""
    if not ctx.env_probe.benchmark_script_exists:
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh does not exist yet")

    script = _bench_script(ctx)
    with SandboxEnv("T2-F11-02", ctx.env_probe) as sb:
        mocks = MockRegistry(sb)
        _install_synthetic_pkg_tools(mocks)
        base_pkg = ctx.repo_root / "tests" / "e2e" / "fixtures" / "omarchy-base.packages"

        ok = sb.run(
            [str(script), "--runs", "1", "--footprint-only", "--json"],
            env_overrides={"OMARCHY_BASE_PACKAGES": str(base_pkg)},
            timeout=20.0,
        )
        if ok.returncode != 0:
            return E2EResult(Status.FAIL, f"--runs 1 failed (rc={ok.returncode}): {ok.stderr}")
        json.loads(ok.stdout)

        for bad_args in (["--runs", "0"], ["--runs", "abc"], ["--runs", "-2"], ["--runs"], ["--dir", str(sb.tmp / "nope")], ["--bogus-flag"]):
            bad = sb.run([str(script), *bad_args], timeout=10.0)
            if bad.returncode == 0:
                return E2EResult(Status.FAIL, f"Invalid CLI args {bad_args} unexpectedly exited 0")
            if not bad.stderr.strip():
                return E2EResult(Status.FAIL, f"Invalid CLI args {bad_args} produced empty stderr diagnostic")
        return E2EResult(Status.PASS, "--runs 1 passed and all invalid --runs/--dir/flag arguments failed fast")


def _t2_f11_03_missing_or_nonexec_gall_binary(ctx: RunContext) -> E2EResult:
    """Non-existent or chmod 0644 --gall binary path reports status='missing' cleanly."""
    if not ctx.env_probe.benchmark_script_exists:
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh does not exist yet")

    script = _bench_script(ctx)
    with SandboxEnv("T2-F11-03", ctx.env_probe) as sb:
        mocks = MockRegistry(sb)
        _install_synthetic_pkg_tools(mocks)
        nonexec = sb.tmp / "gall-nonexec"
        nonexec.write_text("#!/bin/sh\n", encoding="utf-8")
        os.chmod(nonexec, 0o644)

        for target in (sb.tmp / "does-not-exist" / "gall", nonexec):
            res = sb.run([str(script), "--gall", str(target), "--footprint-only", "--json"], timeout=20.0)
            if res.returncode != 0:
                return E2EResult(Status.FAIL, f"--gall {target} crashed footprint mode (rc={res.returncode})")
            data = json.loads(res.stdout)
            gb = data.get("gall_binary", {})
            if gb.get("status") != "missing" or gb.get("path") is not None:
                return E2EResult(Status.FAIL, f"Expected gall_binary.status='missing' for {target}, got {gb}")
        return E2EResult(Status.PASS, "Missing and non-executable --gall paths reported cleanly as status='missing'")


def _t2_f11_04_sigterm_cleanup_mid_benchmark(ctx: RunContext) -> E2EResult:
    """SIGTERM sent mid-benchmark must terminate promptly and clean up TMP_ROOT gall-bench-* dirs."""
    if not ctx.env_probe.benchmark_script_exists:
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh does not exist yet")

    script = _bench_script(ctx)
    with SandboxEnv("T2-F11-04", ctx.env_probe) as sb:
        mocks = MockRegistry(sb)
        pkg_db: dict[str, dict[str, Any] | int] = {"nautilus": {"size": 1000, "deps": []}}
        mocks.install_pactree_expac(pkg_db)
        _install_mock_expac(mocks, pkg_db)
        mocks.install_tool("pactree", stdout="nautilus\n", delay=10.0)
        _install_mock_pacman(mocks, pkg_db)

        proc = sb.popen([str(script), "--footprint-only", "--json"])
        deadline = time.monotonic() + 6.0
        while time.monotonic() < deadline:
            if read_mock_calls(sb, cmd="pactree"):
                break
            if proc.poll() is not None:
                break
            time.sleep(0.05)

        if not read_mock_calls(sb, cmd="pactree"):
            return E2EResult(Status.FAIL, "benchmark-nautilus.sh never reached slow pactree before SIGTERM")

        t0 = time.monotonic()
        proc.terminate()
        try:
            proc.wait(timeout=5.0)
        except Exception:
            return E2EResult(Status.FAIL, "benchmark-nautilus.sh did not terminate within 5s after SIGTERM")
        elapsed = time.monotonic() - t0
        time.sleep(0.15)

        leftover = list(sb.tmp.glob("gall-bench-*"))
        if leftover:
            return E2EResult(Status.FAIL, f"SIGTERM left behind benchmark temp dirs: {leftover}")
        return E2EResult(Status.PASS, f"SIGTERM terminated benchmark in {elapsed:.2f}s with zero leftover temp dirs")


def _t2_f11_05_zero_side_effects_and_footprint_arithmetic(ctx: RunContext) -> E2EResult:
    """Benchmark leaves user config/state untouched and computes exact footprint/ISO delta arithmetic."""
    if not ctx.env_probe.benchmark_script_exists:
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh does not exist yet")

    script = _bench_script(ctx)
    with SandboxEnv("T2-F11-05", ctx.env_probe) as sb:
        mocks = MockRegistry(sb)
        _install_synthetic_pkg_tools(mocks)
        base_pkg = ctx.repo_root / "tests" / "e2e" / "fixtures" / "omarchy-base.packages"

        s1 = sb.home / ".config" / "omarchy" / "sentinel"
        s2 = sb.home / ".local" / "state" / "omarchy" / "sentinel"
        s1.parent.mkdir(parents=True, exist_ok=True)
        s2.parent.mkdir(parents=True, exist_ok=True)
        s1.write_text("untouched-cfg\n", encoding="utf-8")
        s2.write_text("untouched-state\n", encoding="utf-8")
        m1, m2 = s1.stat().st_mtime_ns, s2.stat().st_mtime_ns

        res = sb.run(
            [str(script), "--footprint-only", "--json"],
            env_overrides={"OMARCHY_BASE_PACKAGES": str(base_pkg)},
            timeout=20.0,
        )
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"benchmark-nautilus.sh failed (rc={res.returncode}): {res.stderr}")
        data = json.loads(res.stdout)
        fp = data.get("footprint", {})
        if fp.get("status") != "ok":
            return E2EResult(Status.FAIL, f"Expected footprint.status='ok', got {fp}")
        naut = fp["nautilus"]
        gall = fp["gall"]
        delta = fp["omarchy_iso_delta"]
        expected_naut_direct = 1048576 + 524288 * 3  # 2621440
        expected_naut_closure = 5242880
        expected_gall_closure = 2097152 + 1048576    # 3145728
        if naut.get("direct_bytes") != expected_naut_direct:
            return E2EResult(Status.FAIL, f"nautilus.direct_bytes={naut.get('direct_bytes')} != {expected_naut_direct}")
        if naut.get("closure_bytes") != expected_naut_closure or naut.get("closure_packages") != 7:
            return E2EResult(Status.FAIL, f"nautilus closure mismatch: {naut}")
        if gall.get("closure_bytes") != expected_gall_closure or gall.get("closure_packages") != 2:
            return E2EResult(Status.FAIL, f"gall closure mismatch: {gall}")
        expected_naut_exclusive = expected_naut_direct + 1048576  # 3670016 (4 nautilus stack pkgs + gtk4)
        if delta.get("status") == "skipped" or delta.get("nautilus_exclusive_bytes") != expected_naut_exclusive or delta.get("gall_deps_exclusive_bytes") != expected_gall_closure:
            return E2EResult(Status.FAIL, f"omarchy_iso_delta arithmetic mismatch: {delta}")
        if s1.read_text(encoding="utf-8") != "untouched-cfg\n" or s1.stat().st_mtime_ns != m1:
            return E2EResult(Status.FAIL, "Benchmark mutated ~/.config/omarchy sentinel")
        if s2.read_text(encoding="utf-8") != "untouched-state\n" or s2.stat().st_mtime_ns != m2:
            return E2EResult(Status.FAIL, "Benchmark mutated ~/.local/state/omarchy sentinel")

        return E2EResult(Status.PASS, "Exact footprint arithmetic verified with zero user/Omarchy mutations")


def _t2_f11_06_host_real_package_footprint_dry_run(ctx: RunContext) -> E2EResult:
    """Real host pactree/expac footprint run against /usr/share/omarchy/install/omarchy-base.packages without sudo."""
    if not ctx.env_probe.benchmark_script_exists:
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh does not exist yet")

    base_file = Path("/usr/share/omarchy/install/omarchy-base.packages")
    missing = [t for t in ("pactree", "expac", "pacman") if not shutil.which(t)]
    if missing or not base_file.is_file():
        return E2EResult(
            Status.ENV_UNAVAILABLE,
            f"Host Arch footprint prerequisites missing (tools={missing}, base_file={base_file.is_file()})",
        )

    script = _bench_script(ctx)
    with SandboxEnv("T2-F11-06", ctx.env_probe) as sb:
        res = sb.run([str(script), "--footprint-only", "--json"], timeout=30.0)
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"Real host footprint benchmark failed (rc={res.returncode}): {res.stderr}")
        data = json.loads(res.stdout)
        fp = data.get("footprint", {})
        if fp.get("status") != "ok":
            return E2EResult(Status.FAIL, f"Host footprint status not ok: {fp}")
        if fp["nautilus"]["closure_packages"] <= 0 or fp["gall"]["closure_packages"] <= 0:
            return E2EResult(Status.FAIL, f"Unexpected zero closure package count: {fp}")
        delta = fp.get("omarchy_iso_delta", {})
        if delta.get("nautilus_exclusive_bytes", 0) <= 0:
            return E2EResult(Status.FAIL, f"Expected positive nautilus_exclusive_bytes against real base.packages, got {delta}")
        return E2EResult(
            Status.PASS,
            f"Real host footprint succeeded (nautilus={fp['nautilus']['closure_mib']} MiB, gall_deps={fp['gall']['closure_mib']} MiB, nautilus_iso_delta=+{delta['nautilus_exclusive_mib']} MiB)",
        )


def get_cases() -> list[E2ECase]:
    return [
        E2ECase("T2-F11-01", 2, ("F11",), "Benchmark missing pactree/expac/base.packages graceful skipped diagnostic", requires_binary=False, requires_gui=False, fn=_t2_f11_01_missing_tools_clear_diagnostic),
        E2ECase("T2-F11-02", 2, ("F11",), "Benchmark --runs 1 valid boundary vs invalid --runs/--dir args", requires_binary=False, requires_gui=False, fn=_t2_f11_02_runs_boundary_and_invalid_cli_args),
        E2ECase("T2-F11-03", 2, ("F11",), "Benchmark missing or non-executable --gall binary path", requires_binary=False, requires_gui=False, fn=_t2_f11_03_missing_or_nonexec_gall_binary),
        E2ECase("T2-F11-04", 2, ("F11",), "Benchmark SIGTERM cleanup removes temp dirs without hanging", requires_binary=False, requires_gui=False, fn=_t2_f11_04_sigterm_cleanup_mid_benchmark),
        E2ECase("T2-F11-05", 2, ("F11",), "Benchmark exact footprint arithmetic & zero home/omarchy side effects", requires_binary=False, requires_gui=False, fn=_t2_f11_05_zero_side_effects_and_footprint_arithmetic),
        E2ECase("T2-F11-06", 2, ("F11",), "Benchmark real host pactree/expac against /usr/share/omarchy base list", requires_binary=False, requires_gui=False, fn=_t2_f11_06_host_real_package_footprint_dry_run),
    ]
