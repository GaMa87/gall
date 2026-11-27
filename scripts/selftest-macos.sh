#!/usr/bin/env bash
# macOS graphical self-test runner (WKWebView native snapshots + isolated portable profiles).
# Runs the focused graphical suites and/or the full selftest against a bundled debug build
# (target/debug/gall or Gall.app) without touching ~/Library/Application Support/gall.
#
# Usage:
#   ./scripts/selftest-macos.sh [binary-or-Gall.app]
#   ./scripts/selftest-macos.sh --only rendering,keyboard [binary-or-Gall.app]
#   ./scripts/selftest-macos.sh --full [binary-or-Gall.app]
set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "scripts/selftest-macos.sh is for macOS (Darwin)." >&2
  exit 2
fi

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ONLY=""
INCLUDE_FULL=0
while [ $# -gt 0 ]; do
  case "$1" in
    --only) ONLY="${2:-}"; shift 2 ;;
    --full) INCLUDE_FULL=1; shift ;;
    -h|--help)
      sed -n '2,10p' "$0"
      exit 0
      ;;
    *) break ;;
  esac
done

BIN="${1:-$REPO_ROOT/target/debug/gall}"
if [ -d "$BIN" ] && [ -x "$BIN/Contents/MacOS/gall" ]; then
  BIN="$BIN/Contents/MacOS/gall"
fi
if [ ! -x "$BIN" ]; then
  echo "Binary not found: $BIN" >&2
  echo "Build first: npm --prefix ui run build && cargo build -p gall --features tauri/custom-protocol" >&2
  exit 2
fi

DEFAULT_SUITES=(
  rendering
  keyboard
  popup-placement
  window-overlays
  settings-sections
  quick-open
  search-hidden
  notifications
  bug-report
  i18n
  adaptive-layout
  compare-tabs
  git-workspace
)

if [ -n "$ONLY" ]; then
  IFS=',' read -r -a SUITES <<< "$ONLY"
else
  SUITES=("${DEFAULT_SUITES[@]}")
fi

PASS=0
FAIL=0
FAILED_NAMES=()
OUT_LOG="$(mktemp -t gall-macos-selftest-summary-XXXX)"
trap 'rm -f "$OUT_LOG"' EXIT

run_one() {
  local name="$1"
  shift
  echo "=== [macOS graphical selftest] $name ==="
  local tmp_out
  tmp_out="$(mktemp -t gall-macos-one-XXXX)"
  if "$REPO_ROOT/scripts/selftest.sh" "$@" "$BIN" >"$tmp_out" 2>&1; then
    local shots
    shots="$(grep '^Screenshots:' "$tmp_out" | tail -n 1 || true)"
    local summary
    summary="$(grep -E '^[0-9]+ failure' "$tmp_out" | tail -n 1 || true)"
    echo "PASS  $name  (${summary:-ok})  $shots"
    PASS=$((PASS + 1))
  else
    echo "FAIL  $name"
    grep -E '^(FAIL|NO REPORT|Screenshots:|.*failure\(s\))' "$tmp_out" | tail -n 20 || true
    FAIL=$((FAIL + 1))
    FAILED_NAMES+=("$name")
  fi
  rm -f "$tmp_out"
}

for suite in "${SUITES[@]}"; do
  [ -z "$suite" ] && continue
  if [ "$suite" = "full" ]; then
    run_one "full"
  else
    run_one "$suite" "--$suite"
  fi
done

if [ "$INCLUDE_FULL" -eq 1 ]; then
  run_one "full"
fi

echo
echo "macOS graphical self-tests finished: $PASS passed, $FAIL failed."
if [ "$FAIL" -gt 0 ]; then
  echo "Failed suites: ${FAILED_NAMES[*]}"
  exit 1
fi
