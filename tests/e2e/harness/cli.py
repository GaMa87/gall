"""CLI entrypoint for the Gall opaque-box E2E test suite."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from .env_probe import probe_env
from .runner import VALID_FEATURES, discover_cases, filter_cases, run_suite
from .types import RunContext, Status


def _parse_csv_tokens(values: list[str] | None) -> list[str]:
    out: list[str] = []
    if not values:
        return out
    for item in values:
        for part in item.split(","):
            tok = part.strip()
            if tok:
                out.append(tok)
    return out


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="e2e-suite.sh",
        description="Gall opaque-box E2E test runner (Tiers 1-4)",
    )
    p.add_argument(
        "--repo-root",
        default=None,
        help="Path to repository root (defaults to auto-detected repo root)",
    )
    p.add_argument(
        "--tier",
        action="append",
        default=None,
        help="Tier selector: 1|2|3|4|all (repeatable or comma-separated; default: all)",
    )
    p.add_argument(
        "--feature",
        action="append",
        default=None,
        help="Feature selector: F1..F11 (repeatable or comma-separated)",
    )
    p.add_argument(
        "--id",
        action="append",
        default=None,
        help="Exact test ID selector (repeatable or comma-separated)",
    )
    p.add_argument(
        "--binary",
        default=None,
        help="Path to compiled gall binary (default: $REPO_ROOT/target/debug/gall)",
    )
    p.add_argument(
        "--strict",
        action="store_true",
        default=False,
        help="Strict acceptance gate (default): exit 0 iff FAIL=0, PENDING_IMPL=0, ENV_UNAVAILABLE=0",
    )
    p.add_argument(
        "--allow-pending",
        action="store_true",
        default=False,
        help="Do not fail exit code on PENDING_IMPL (FAIL still fails)",
    )
    p.add_argument(
        "--allow-env-unavailable",
        action="store_true",
        default=False,
        help="Do not fail exit code on ENV_UNAVAILABLE (FAIL still fails)",
    )
    p.add_argument(
        "--no-self-audit",
        action="store_true",
        default=False,
        help="Skip built-in T0-self-audit preflight check",
    )
    p.add_argument(
        "--json-out",
        default=None,
        help="Write structured JSON report to path",
    )
    p.add_argument(
        "--list",
        action="store_true",
        default=False,
        help="List matching test cases without running them",
    )
    p.add_argument(
        "--verbose",
        action="store_true",
        default=False,
        help="Print detailed per-test diagnostics",
    )
    return p


def main(argv: list[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)

    if args.strict and (args.allow_pending or args.allow_env_unavailable):
        parser.error("--strict cannot be combined with --allow-pending or --allow-env-unavailable")

    repo_root = (
        Path(args.repo_root).resolve()
        if args.repo_root
        else Path(__file__).resolve().parents[3]
    )
    e2e_dir = repo_root / "tests" / "e2e"
    if not e2e_dir.is_dir():
        print(f"e2e-suite: E2E directory not found: {e2e_dir}", file=sys.stderr)
        return 2

    tier_tokens = _parse_csv_tokens(args.tier)
    tiers: set[int] = set()
    if not tier_tokens or "all" in [t.lower() for t in tier_tokens]:
        tiers = {1, 2, 3, 4}
    else:
        for tok in tier_tokens:
            if tok not in ("1", "2", "3", "4"):
                print(f"e2e-suite: invalid --tier value {tok!r} (expected 1|2|3|4|all)", file=sys.stderr)
                return 2
            tiers.add(int(tok))

    feat_tokens = [f.upper() for f in _parse_csv_tokens(args.feature)]
    for f in feat_tokens:
        if f not in VALID_FEATURES:
            print(
                f"e2e-suite: invalid --feature value {f!r} (expected F1..F11)",
                file=sys.stderr,
            )
            return 2
    features = set(feat_tokens)
    ids = set(_parse_csv_tokens(args.id))

    probe = probe_env(repo_root, args.binary)
    if probe.binary_explicit and not probe.binary_exists:
        print(
            f"e2e-suite: explicit --binary / GALL_E2E_BINARY missing or not executable: {probe.binary_path}",
            file=sys.stderr,
        )
        return 2

    all_cases = discover_cases(e2e_dir)
    selected = filter_cases(
        all_cases,
        tiers=tiers,
        features=features,
        ids=ids,
        include_self_audit=not args.no_self_audit,
    )

    if not selected:
        print("e2e-suite: 0 test cases matched the given filters", file=sys.stderr)
        return 2

    if args.list:
        for case in selected:
            feats = ",".join(case.features) if case.features else "-"
            flags = []
            if case.requires_binary:
                flags.append("binary")
            if case.requires_gui:
                flags.append("gui")
            if case.requires_dbus:
                flags.append("dbus")
            flag_str = ",".join(flags) if flags else "offline"
            print(f"{case.id}\tT{case.tier}\t[{feats}]\t({flag_str})\t{case.title}")
        print(f"Total: {len(selected)} case(s)")
        return 0

    strict = not (args.allow_pending or args.allow_env_unavailable)
    ctx = RunContext(
        repo_root=repo_root,
        binary_path=probe.binary_path,
        env_probe=probe,
        verbose=args.verbose,
    )

    outcomes, report = run_suite(
        selected,
        ctx,
        run_preflight=not args.no_self_audit,
        strict=strict,
        allow_pending=args.allow_pending,
        allow_env_unavailable=args.allow_env_unavailable,
    )

    if args.json_out:
        out_path = Path(args.json_out)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    for outcome in outcomes:
        c = outcome.case
        r = outcome.result
        feats = ",".join(c.features) if c.features else "-"
        print(f"[{r.status.value:15s}] {c.id} (T{c.tier}, {feats}, {r.duration_ms:.1f}ms) - {c.title}")
        if args.verbose or r.status in (Status.FAIL, Status.PENDING_IMPL, Status.ENV_UNAVAILABLE):
            if r.message:
                print(f"    {r.message}")
            if args.verbose and r.details:
                print(f"    details: {json.dumps(r.details, ensure_ascii=False)}")

    s = report["summary"]
    print(
        f"\nSummary: selected={s['selected']} PASS={s['pass']} FAIL={s['fail']} "
        f"PENDING_IMPL={s['pending_impl']} ENV_UNAVAILABLE={s['env_unavailable']} "
        f"({s['duration_ms']:.1f}ms) | gate={'PASS' if report['gate_passed'] else 'FAIL'}"
    )
    return 0 if report["gate_passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
