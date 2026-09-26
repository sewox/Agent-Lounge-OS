#!/usr/bin/env bash
# S4 mock-ban gate: MOCK_HEALTH / MOCK_NODES / MOCK_QUOTAS / MOCK_EXPERIENCES / MOCK_EVENTS
# must not remain in live (Tauri) code paths.
# Allowlist: src/lib/mock/** (browser-harness fixtures only).
# PR-0: warning mode (exit 0, prints findings). PR-2: set QA_MOCK_BAN_STRICT=1 to fail.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"
# shellcheck source=scripts/qa/gates/_search.sh
source "$(dirname "$0")/_search.sh"

if ! qa_search_init; then
  exit 2
fi
if ! qa_search_selftest; then
  exit 1
fi

PATTERN='MOCK_(HEALTH|NODES|QUOTAS|EXPERIENCES|EVENTS)'
RAW_HITS="$(qa_search_hits "$PATTERN" src || true)"

# Explicit browser-only allowlist: fixtures under src/lib/mock/
HITS=""
if [[ -n "$RAW_HITS" ]]; then
  HITS="$(printf '%s\n' "$RAW_HITS" | grep -vE '(^|/)src/lib/mock/' || true)"
fi

COUNT=0
if [[ -n "$HITS" ]]; then
  COUNT="$(printf '%s\n' "$HITS" | grep -c . || true)"
fi

echo "== mock-ban gate (strict=${QA_MOCK_BAN_STRICT:-0}, tool=$qa_search_tool) =="
if [[ -z "$HITS" ]]; then
  echo "OK: no banned MOCK_(HEALTH|NODES|QUOTAS|EXPERIENCES|EVENTS) outside src/lib/mock/"
  exit 0
fi

echo "FAIL: found $COUNT line(s) with banned MOCK_* in live code paths:"
echo "$HITS"
echo
echo "Policy (K9/K12): Tauri/live mode must not fall back to MOCK_*."
echo "Browser fixtures belong in src/lib/mock/ (allowlisted)."

if [[ "${QA_MOCK_BAN_STRICT:-0}" == "1" ]]; then
  exit 1
fi
echo "(warning mode — set QA_MOCK_BAN_STRICT=1 to fail)"
exit 0
