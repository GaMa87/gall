#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if ! command -v python3 >/dev/null 2>&1; then
  echo "e2e-suite.sh: python3 is required (>= 3.11, stdlib only)" >&2
  exit 2
fi

if ! python3 -c 'import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)'; then
  PY_VER="$(python3 -c 'import sys; print(".".join(map(str, sys.version_info[:3])))' 2>/dev/null || echo unknown)"
  echo "e2e-suite.sh: python3 >= 3.11 required, found $PY_VER" >&2
  exit 2
fi

export PYTHONDONTWRITEBYTECODE=1
export PYTHONPATH="$REPO_ROOT/tests/e2e${PYTHONPATH:+:$PYTHONPATH}"

exec python3 -m harness.cli --repo-root "$REPO_ROOT" "$@"
