"""Tier 1 E2E tests for F11: Gall vs. Nautilus comparison benchmark (scripts/benchmark-nautilus.sh)."""

from __future__ import annotations

import json
import os
from typing import Any

from harness import E2ECase, E2EResult, MockRegistry, RunContext, SandboxEnv, Status, read_mock_calls


def _install_benchmark_pkg_mocks(
    ctx: RunContext,
    sb: SandboxEnv,
) -> tuple[dict[str, dict[str, Any]], dict[str, int]]:
    """Install deterministic expac, pactree, and pacman shims matching Arch CLI flags."""
    base_file = ctx.repo_root / "tests" / "e2e" / "fixtures" / "omarchy-base.packages"
    base_pkgs = [
        line.split("#", 1)[0].strip()
        for line in base_file.read_text(encoding="utf-8").splitlines()
        if line.split("#", 1)[0].strip()
    ]

    db: dict[str, dict[str, Any]] = {}
    for pkg in base_pkgs:
        db[pkg] = {"size": 1_000_000, "deps": [], "provides": []}

    # Nautilus stack + exclusive transitive deps
    db["libnautilus-ext"] = {"size": 5_000_000, "deps": [], "provides": []}
    db["python-gobject-ext"] = {"size": 2_000_000, "deps": [], "provides": []}
    db["gjs-sushi-dep"] = {"size": 4_000_000, "deps": [], "provides": []}

    db["nautilus"] = {"size": 8_000_000, "deps": ["base", "gvfs", "libnautilus-ext"], "provides": []}
    db["nautilus-python"] = {"size": 1_000_000, "deps": ["nautilus", "python-gobject-ext"], "provides": []}
    db["sushi"] = {"size": 3_000_000, "deps": ["nautilus", "gjs-sushi-dep"], "provides": []}
    db["ffmpegthumbnailer"] = {"size": 500_000, "deps": ["ffmpeg"], "provides": []}

    # Gall runtime & optional deps
    db["libwebkit-dep"] = {"size": 2_000_000, "deps": [], "provides": []}
    db["gtk3"] = {"size": 10_000_000, "deps": ["fontconfig"], "provides": []}
    db["webkit2gtk-4.1"] = {"size": 20_000_000, "deps": ["gtk3", "libwebkit-dep"], "provides": []}
    db["gst-excl"] = {"size": 1_500_000, "deps": [], "provides": []}
    db["gst-plugins-good"] = {"size": 6_000_000, "deps": ["gst-excl"], "provides": []}

    db_path = sb.tmp / "mock_arch_db.json"
    db_path.write_text(json.dumps(db), encoding="utf-8")

    mocks = MockRegistry(sb)
    mocks.install_pactree_expac(db)

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
    mocks.install_tool("pacman", script_body=pacman_body)

    expected = {
        "nautilus_direct_bytes": 8_000_000 + 1_000_000 + 3_000_000 + 500_000,
        "nautilus_closure_packages": 10,  # 4 direct + 3 excl + base + gvfs + ffmpeg
        "nautilus_closure_bytes": 12_500_000 + 11_000_000 + 3_000_000,
        "nautilus_exclusive_packages": 7,
        "nautilus_exclusive_bytes": 12_500_000 + 11_000_000,
        "gall_closure_packages": 4,  # webkit2gtk-4.1, gtk3, libwebkit-dep, fontconfig
        "gall_closure_bytes": 20_000_000 + 10_000_000 + 2_000_000 + 1_000_000,
        "gall_deps_exclusive_packages": 3,
        "gall_deps_exclusive_bytes": 32_000_000,
        "gst_exclusive_packages": 2,
        "gst_exclusive_bytes": 7_500_000,
    }
    return db, expected


def _t1_f11_01_script_exists_syntax_and_help(ctx: RunContext) -> E2EResult:
    script = ctx.repo_root / "scripts" / "benchmark-nautilus.sh"
    if not script.is_file():
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh not yet created")
    if not os.access(script, os.X_OK):
        return E2EResult(Status.FAIL, "scripts/benchmark-nautilus.sh is not executable")

    with SandboxEnv("T1-F11-01", ctx.env_probe) as sb:
        syn = sb.run(["bash", "-n", str(script)])
        if syn.returncode != 0:
            return E2EResult(Status.FAIL, "benchmark-nautilus.sh failed bash -n syntax check", {"stderr": syn.stderr})

        hlp = sb.run([str(script), "--help"])
        if hlp.returncode != 0:
            return E2EResult(Status.FAIL, f"benchmark-nautilus.sh --help exited {hlp.returncode}", {"stderr": hlp.stderr})
        for opt in ("--json", "--tsv", "--runs", "--gall", "--footprint-only"):
            if opt not in hlp.stdout:
                return E2EResult(Status.FAIL, f"--help missing option documentation for {opt}", {"stdout": hlp.stdout})

    return E2EResult(Status.PASS, "benchmark-nautilus.sh is executable, passes bash -n, and documents all CLI options")


def _t1_f11_02_footprint_and_iso_delta_arithmetic(ctx: RunContext) -> E2EResult:
    script = ctx.repo_root / "scripts" / "benchmark-nautilus.sh"
    if not script.is_file():
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh not yet created")

    with SandboxEnv("T1-F11-02", ctx.env_probe) as sb:
        _, exp = _install_benchmark_pkg_mocks(ctx, sb)
        base_fixture = ctx.repo_root / "tests" / "e2e" / "fixtures" / "omarchy-base.packages"
        res = sb.run(
            [str(script), "--footprint-only", "--json"],
            env_overrides={"OMARCHY_BASE_PACKAGES": str(base_fixture)},
        )
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"benchmark-nautilus.sh exited {res.returncode}", {"stderr": res.stderr})

        try:
            data = json.loads(res.stdout)
        except json.JSONDecodeError as exc:
            return E2EResult(Status.FAIL, f"Invalid JSON output: {exc}", {"stdout": res.stdout})

        fp = data.get("footprint", {})
        if fp.get("status") != "ok":
            return E2EResult(Status.FAIL, f"Footprint status not ok: {fp}")

        naut = fp["nautilus"]
        gall = fp["gall"]
        delta = fp["omarchy_iso_delta"]

        checks = [
            (naut["direct_bytes"], exp["nautilus_direct_bytes"], "nautilus.direct_bytes"),
            (naut["closure_packages"], exp["nautilus_closure_packages"], "nautilus.closure_packages"),
            (naut["closure_bytes"], exp["nautilus_closure_bytes"], "nautilus.closure_bytes"),
            (gall["closure_packages"], exp["gall_closure_packages"], "gall.closure_packages"),
            (gall["closure_bytes"], exp["gall_closure_bytes"], "gall.closure_bytes"),
            (delta["nautilus_exclusive_packages"], exp["nautilus_exclusive_packages"], "delta.nautilus_exclusive_packages"),
            (delta["nautilus_exclusive_bytes"], exp["nautilus_exclusive_bytes"], "delta.nautilus_exclusive_bytes"),
            (delta["gall_deps_exclusive_packages"], exp["gall_deps_exclusive_packages"], "delta.gall_deps_exclusive_packages"),
            (delta["gall_deps_exclusive_bytes"], exp["gall_deps_exclusive_bytes"], "delta.gall_deps_exclusive_bytes"),
            (delta["gall_optional_gst_plugins_good"]["exclusive_packages"], exp["gst_exclusive_packages"], "gst.exclusive_packages"),
            (delta["gall_optional_gst_plugins_good"]["exclusive_bytes"], exp["gst_exclusive_bytes"], "gst.exclusive_bytes"),
        ]
        for actual, expected_val, label in checks:
            if actual != expected_val:
                return E2EResult(Status.FAIL, f"Footprint arithmetic mismatch for {label}: got {actual}, expected {expected_val}")

        if not read_mock_calls(sb, "expac") or not read_mock_calls(sb, "pactree"):
            return E2EResult(Status.FAIL, "Expected mocked expac and pactree invocations to be recorded")

    return E2EResult(Status.PASS, "Footprint closure and Omarchy ISO delta arithmetic exact against mock DB")


def _t1_f11_03_ram_rss_pss_aggregation(ctx: RunContext) -> E2EResult:
    script = ctx.repo_root / "scripts" / "benchmark-nautilus.sh"
    if not script.is_file():
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh not yet created")

    assert ctx.binary_path is not None
    with SandboxEnv("T1-F11-03", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        res = sb.run([str(script), "--runs", "1", "--json", "--gall", str(ctx.binary_path)], timeout=120.0)
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"benchmark-nautilus.sh exited {res.returncode}", {"stderr": res.stderr})
        data = json.loads(res.stdout)
        ram = data.get("ram", {})
        if ram.get("status") == "skipped":
            return E2EResult(
                Status.ENV_UNAVAILABLE,
                f"RAM GUI benchmark skipped by benchmark-nautilus.sh: {ram.get('reason', 'headless/no display')}",
            )
        if ram.get("status") != "ok":
            return E2EResult(Status.FAIL, f"RAM section not ok: {ram}")
        gall_ram = ram.get("gall", {})
        if not (gall_ram.get("app_rss_mib", 0) > 0 and gall_ram.get("app_pss_mib", 0) > 0):
            return E2EResult(Status.FAIL, f"Expected positive Gall RSS/PSS MiB, got {gall_ram}")
    return E2EResult(Status.PASS, "RAM RSS/PSS aggregation via smaps_rollup produced valid MiB figures")


def _t1_f11_04_cold_start_and_daemon_warm_start(ctx: RunContext) -> E2EResult:
    script = ctx.repo_root / "scripts" / "benchmark-nautilus.sh"
    if not script.is_file():
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh not yet created")

    assert ctx.binary_path is not None
    with SandboxEnv("T1-F11-04", ctx.env_probe, allow_gui=True, multi_instance=True, dbus_required=True) as sb:
        res = sb.run([str(script), "--runs", "2", "--json", "--gall", str(ctx.binary_path)], timeout=120.0)
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"benchmark-nautilus.sh exited {res.returncode}", {"stderr": res.stderr})
        data = json.loads(res.stdout)
        cold = data.get("cold_start", {})
        if cold.get("status") == "skipped":
            return E2EResult(
                Status.ENV_UNAVAILABLE,
                f"Cold-start GUI benchmark skipped by benchmark-nautilus.sh: {cold.get('reason', 'headless/no display')}",
            )
        if cold.get("status") != "ok":
            return E2EResult(Status.FAIL, f"cold_start section not ok: {cold}")
        gall_cold = cold.get("gall", {})
        if len(gall_cold.get("wall_ms_samples", [])) != 2 or not (gall_cold.get("wall_ms_median", 0) > 0):
            return E2EResult(Status.FAIL, f"Invalid cold-start samples/median: {gall_cold}")
        warm = data.get("daemon_warm_start", {})
        if warm.get("status") == "unsupported":
            return E2EResult(Status.PENDING_IMPL, f"Daemon warm-start unsupported in binary: {warm.get('reason')}")
        if warm.get("status") == "skipped":
            return E2EResult(
                Status.ENV_UNAVAILABLE,
                f"Daemon warm-start skipped by benchmark-nautilus.sh: {warm.get('reason', 'headless/no display')}",
            )
        if warm.get("status") not in ("ok", "partial"):
            return E2EResult(Status.FAIL, f"Unexpected daemon_warm_start status: {warm}")
    return E2EResult(Status.PASS, "Measured 2-run cold-start median and --daemon warm-start probe")


def _t1_f11_05_output_formats_and_file_contract(ctx: RunContext) -> E2EResult:
    script = ctx.repo_root / "scripts" / "benchmark-nautilus.sh"
    if not script.is_file():
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh not yet created")

    with SandboxEnv("T1-F11-05", ctx.env_probe) as sb:
        _install_benchmark_pkg_mocks(ctx, sb)
        base_fixture = ctx.repo_root / "tests" / "e2e" / "fixtures" / "omarchy-base.packages"
        env_ov = {"OMARCHY_BASE_PACKAGES": str(base_fixture)}

        # JSON contract
        res_json = sb.run([str(script), "--footprint-only", "--json"], env_overrides=env_ov)
        if res_json.returncode != 0:
            return E2EResult(Status.FAIL, f"--json exited {res_json.returncode}", {"stderr": res_json.stderr})
        parsed = json.loads(res_json.stdout)
        for key in ("timestamp_utc", "host", "target_dir", "gall_binary", "footprint", "ram", "cold_start", "daemon_warm_start"):
            if key not in parsed:
                return E2EResult(Status.FAIL, f"JSON output missing required top-level key {key!r}")

        # TSV contract
        res_tsv = sb.run([str(script), "--footprint-only", "--tsv"], env_overrides=env_ov)
        if res_tsv.returncode != 0:
            return E2EResult(Status.FAIL, f"--tsv exited {res_tsv.returncode}", {"stderr": res_tsv.stderr})
        tsv_lines = [ln for ln in res_tsv.stdout.splitlines() if ln.strip()]
        if not tsv_lines or tsv_lines[0] != "metric\tgall\tnautilus\tstatus\tnotes":
            return E2EResult(Status.FAIL, f"Invalid TSV header: {tsv_lines[:2]}")
        if not any(ln.startswith("closure_bytes\t") for ln in tsv_lines):
            return E2EResult(Status.FAIL, "TSV output missing closure_bytes row", {"tsv": tsv_lines})

        # Markdown / --output contract
        out_md = sb.work / "bench-report.md"
        res_md = sb.run([str(script), "--footprint-only", "--output", str(out_md)], env_overrides=env_ov)
        if res_md.returncode != 0 or not out_md.is_file():
            return E2EResult(Status.FAIL, f"--output failed (rc={res_md.returncode})")
        if res_md.stdout.strip():
            return E2EResult(Status.FAIL, "--output should suppress stdout", {"stdout": res_md.stdout})
        md_text = out_md.read_text(encoding="utf-8")
        if "# Gall vs. Nautilus Comparison Benchmark" not in md_text or "MiB" not in md_text:
            return E2EResult(Status.FAIL, "Markdown output missing expected header or MiB units", {"md": md_text[:400]})

    return E2EResult(Status.PASS, "JSON, TSV, and Markdown --output contracts verified")


def _t1_f11_06_isolation_and_zero_side_effects(ctx: RunContext) -> E2EResult:
    script = ctx.repo_root / "scripts" / "benchmark-nautilus.sh"
    if not script.is_file():
        return E2EResult(Status.PENDING_IMPL, "scripts/benchmark-nautilus.sh not yet created")

    with SandboxEnv("T1-F11-06", ctx.env_probe) as sb:
        _install_benchmark_pkg_mocks(ctx, sb)
        base_fixture = ctx.repo_root / "tests" / "e2e" / "fixtures" / "omarchy-base.packages"

        sentinel_cfg = sb.config / "gall" / "settings.json"
        sentinel_cfg.parent.mkdir(parents=True, exist_ok=True)
        sentinel_cfg.write_text('{"sentinel": true}\n', encoding="utf-8")
        sentinel_omarchy = sb.state / "omarchy" / "current" / "theme.name"
        sentinel_omarchy.parent.mkdir(parents=True, exist_ok=True)
        sentinel_omarchy.write_text("catppuccin\n", encoding="utf-8")

        before_tmp = set(p.name for p in sb.tmp.iterdir())
        res = sb.run(
            [str(script), "--footprint-only", "--json", "--runs", "1"],
            env_overrides={"OMARCHY_BASE_PACKAGES": str(base_fixture)},
        )
        if res.returncode != 0:
            return E2EResult(Status.FAIL, f"benchmark-nautilus.sh exited {res.returncode}", {"stderr": res.stderr})

        if sentinel_cfg.read_text(encoding="utf-8") != '{"sentinel": true}\n':
            return E2EResult(Status.FAIL, "Benchmark mutated user Gall settings.json")
        if sentinel_omarchy.read_text(encoding="utf-8") != "catppuccin\n":
            return E2EResult(Status.FAIL, "Benchmark mutated Omarchy state files")

        after_tmp = set(p.name for p in sb.tmp.iterdir())
        stray_bench_dirs = [n for n in (after_tmp - before_tmp) if n.startswith("gall-bench-")]
        if stray_bench_dirs:
            return E2EResult(Status.FAIL, f"Benchmark leaked temp dirs in TMPDIR: {stray_bench_dirs}")

    return E2EResult(Status.PASS, "Benchmark preserved user/Omarchy state and cleaned up temporary directories")


def get_cases() -> list[E2ECase]:
    return [
        E2ECase(
            id="T1-F11-01",
            tier=1,
            features=("F11",),
            title="benchmark-nautilus.sh exists, is executable, passes bash -n, and documents CLI options",
            requires_binary=False,
            requires_gui=False,
            requires_dbus=False,
            fn=_t1_f11_01_script_exists_syntax_and_help,
        ),
        E2ECase(
            id="T1-F11-02",
            tier=1,
            features=("F11",),
            title="Footprint and Omarchy ISO delta arithmetic matches mock package database",
            requires_binary=False,
            requires_gui=False,
            requires_dbus=False,
            fn=_t1_f11_02_footprint_and_iso_delta_arithmetic,
        ),
        E2ECase(
            id="T1-F11-03",
            tier=1,
            features=("F11",),
            title="RAM RSS and PSS aggregation across process trees produces valid MiB values",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f11_03_ram_rss_pss_aggregation,
        ),
        E2ECase(
            id="T1-F11-04",
            tier=1,
            features=("F11",),
            title="Cold-start latency and --daemon warm-start probe execute cleanly",
            requires_binary=True,
            requires_gui=True,
            requires_dbus=True,
            fn=_t1_f11_04_cold_start_and_daemon_warm_start,
        ),
        E2ECase(
            id="T1-F11-05",
            tier=1,
            features=("F11",),
            title="Machine-readable (--json, --tsv) and human-readable (--output) formats validated",
            requires_binary=False,
            requires_gui=False,
            requires_dbus=False,
            fn=_t1_f11_05_output_formats_and_file_contract,
        ),
        E2ECase(
            id="T1-F11-06",
            tier=1,
            features=("F11",),
            title="Benchmark leaves zero side-effects on user config, Omarchy state, and TMPDIR",
            requires_binary=False,
            requires_gui=False,
            requires_dbus=False,
            fn=_t1_f11_06_isolation_and_zero_side_effects,
        ),
    ]
