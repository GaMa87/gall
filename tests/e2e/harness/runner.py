"""Discovery, preflight self-audit, filtering, and execution engine."""

from __future__ import annotations

import importlib
import subprocess
import time
import traceback
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from .assertions import assert_no_forbidden_refs, read_mock_calls
from .env_probe import EnvProbe
from .mocks import MockRegistry
from .sandbox import SandboxEnv
from .types import E2ECase, E2EResult, RunContext, Status

VALID_FEATURES: tuple[str, ...] = tuple(f"F{i}" for i in range(1, 12))


def _run_self_audit(ctx: RunContext) -> E2EResult:
    e2e_dir = ctx.repo_root / "tests" / "e2e"
    audited = assert_no_forbidden_refs(e2e_dir)

    with SandboxEnv("self-audit", ctx.env_probe) as sb:
        mocks = MockRegistry(sb)
        mocks.install_tool("e2e-audit-probe", stdout="ok\n")
        res = sb.run(["e2e-audit-probe", "--ping"])
        if res.returncode != 0 or res.stdout.strip() != "ok":
            return E2EResult(
                Status.FAIL,
                f"Mock tool execution failed in sandbox (rc={res.returncode})",
                {"stdout": res.stdout, "stderr": res.stderr},
            )
        calls = read_mock_calls(sb, cmd="e2e-audit-probe")
        if len(calls) != 1 or calls[0].get("argv") != ["--ping"]:
            return E2EResult(
                Status.FAIL,
                "Mock tool did not record expected JSONL entry to GALL_E2E_MOCK_LOG",
                {"calls": calls},
            )

    return E2EResult(
        Status.PASS,
        f"Audited {len(audited)} Python file(s); sandbox and mock logging verified",
        {"audited_files": audited},
    )


SELF_AUDIT_CASE = E2ECase(
    id="T0-self-audit",
    tier=0,
    features=(),
    title="Opaque-box boundary & mock recording preflight audit",
    requires_binary=False,
    requires_gui=False,
    requires_dbus=False,
    fn=_run_self_audit,
)


def discover_cases(e2e_dir: Path) -> list[E2ECase]:
    """Auto-discover Tier 1..4 test modules exporting get_cases() -> list[E2ECase]."""
    cases: list[E2ECase] = []

    for pkg in ("tier1_features", "tier2_boundaries"):
        pkg_dir = e2e_dir / pkg
        if not pkg_dir.is_dir():
            continue
        for py_file in sorted(pkg_dir.glob("*.py")):
            if py_file.name == "__init__.py" or py_file.name.startswith("_"):
                continue
            mod_name = f"{pkg}.{py_file.stem}"
            mod = importlib.import_module(mod_name)
            getter = getattr(mod, "get_cases", None)
            if callable(getter):
                cases.extend(getter())

    for mod_name in ("tier3_pairwise", "tier4_workloads"):
        mod_file = e2e_dir / f"{mod_name}.py"
        if not mod_file.is_file():
            continue
        mod = importlib.import_module(mod_name)
        getter = getattr(mod, "get_cases", None)
        if callable(getter):
            cases.extend(getter())

    return cases


def filter_cases(
    cases: list[E2ECase],
    tiers: set[int] | None = None,
    features: set[str] | None = None,
    ids: set[str] | None = None,
    include_self_audit: bool = True,
) -> list[E2ECase]:
    """Filter discovered cases and conditionally include T0-self-audit."""
    selected: list[E2ECase] = []

    if include_self_audit:
        wants_all_tiers = not tiers or tiers == {1, 2, 3, 4}
        explicit_id = bool(ids and SELF_AUDIT_CASE.id in ids)
        if explicit_id or (wants_all_tiers and not features and not ids):
            selected.append(SELF_AUDIT_CASE)

    for case in cases:
        if tiers and case.tier not in tiers:
            continue
        if features and not (set(case.features) & features):
            continue
        if ids and case.id not in ids:
            continue
        selected.append(case)

    return selected


@dataclass
class CaseOutcome:
    case: E2ECase
    result: E2EResult

    def to_dict(self) -> dict[str, Any]:
        return {
            "id": self.case.id,
            "tier": self.case.tier,
            "features": list(self.case.features),
            "title": self.case.title,
            "requires_binary": self.case.requires_binary,
            "requires_gui": self.case.requires_gui,
            "requires_dbus": self.case.requires_dbus,
            **self.result.to_dict(),
        }


def _git_head(repo_root: Path) -> str | None:
    try:
        res = subprocess.run(
            ["git", "-C", str(repo_root), "rev-parse", "--short", "HEAD"],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
        if res.returncode == 0:
            return res.stdout.strip() or None
    except (OSError, subprocess.TimeoutExpired):
        pass
    return None


def execute_case(case: E2ECase, ctx: RunContext) -> E2EResult:
    t0 = time.monotonic()
    try:
        if case.requires_binary and not ctx.env_probe.binary_exists:
            return E2EResult(
                Status.PENDING_IMPL,
                f"Gall binary not built at {ctx.binary_path}",
                duration_ms=(time.monotonic() - t0) * 1000.0,
            )
        if case.requires_gui and not ctx.env_probe.gui_available:
            return E2EResult(
                Status.ENV_UNAVAILABLE,
                "No headless compositor (kwin_wayland/labwc/weston/sway/Xvfb) and no reachable Wayland/X11 socket",
                duration_ms=(time.monotonic() - t0) * 1000.0,
            )
        if case.requires_dbus and not (ctx.env_probe.dbus_daemon or ctx.env_probe.dbus_run_session):
            return E2EResult(
                Status.ENV_UNAVAILABLE,
                "Neither dbus-daemon nor dbus-run-session is available on PATH",
                duration_ms=(time.monotonic() - t0) * 1000.0,
            )
        res = case.fn(ctx)
        if res.duration_ms <= 0.0:
            res.duration_ms = (time.monotonic() - t0) * 1000.0
        return res
    except Exception as exc:
        return E2EResult(
            Status.FAIL,
            f"Unhandled {type(exc).__name__}: {exc}",
            details={"traceback": traceback.format_exc()},
            duration_ms=(time.monotonic() - t0) * 1000.0,
        )


def run_suite(
    cases: list[E2ECase],
    ctx: RunContext,
    run_preflight: bool = True,
    strict: bool = True,
    allow_pending: bool = False,
    allow_env_unavailable: bool = False,
) -> tuple[list[CaseOutcome], dict[str, Any]]:
    t0 = time.monotonic()
    outcomes: list[CaseOutcome] = []

    if run_preflight and all(c.id != SELF_AUDIT_CASE.id for c in cases):
        preflight_res = execute_case(SELF_AUDIT_CASE, ctx)
        if preflight_res.status != Status.PASS:
            outcomes.append(CaseOutcome(SELF_AUDIT_CASE, preflight_res))

    for case in cases:
        res = execute_case(case, ctx)
        outcomes.append(CaseOutcome(case, res))

    total_ms = (time.monotonic() - t0) * 1000.0
    counts = {
        Status.PASS.value: 0,
        Status.FAIL.value: 0,
        Status.PENDING_IMPL.value: 0,
        Status.ENV_UNAVAILABLE.value: 0,
    }
    for out in outcomes:
        counts[out.result.status.value] += 1

    selected = len(cases)
    gate_passed = (
        selected > 0
        and counts[Status.FAIL.value] == 0
        and (counts[Status.PENDING_IMPL.value] == 0 or allow_pending)
        and (counts[Status.ENV_UNAVAILABLE.value] == 0 or allow_env_unavailable)
    )

    report: dict[str, Any] = {
        "schema_version": 1,
        "timestamp_utc": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "git_head": _git_head(ctx.repo_root),
        "strict": strict,
        "allow_pending": allow_pending,
        "allow_env_unavailable": allow_env_unavailable,
        "gate_passed": gate_passed,
        "env_probe": ctx.env_probe.to_dict(),
        "summary": {
            "selected": selected,
            "executed": len(outcomes),
            "pass": counts[Status.PASS.value],
            "fail": counts[Status.FAIL.value],
            "pending_impl": counts[Status.PENDING_IMPL.value],
            "env_unavailable": counts[Status.ENV_UNAVAILABLE.value],
            "duration_ms": round(total_ms, 2),
        },
        "cases": [o.to_dict() for o in outcomes],
    }
    return outcomes, report
