#!/usr/bin/env bash
# S4 mock-ban gate: MOCK_HEALTH / MOCK_NODES must not remain in live code paths.
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

PATTERN='MOCK_HEALTH|MOCK_NODES'
HITS="$(qa_search_hits "$PATTERN" src || true)"

COUNT=0
if [[ -n "$HITS" ]]; then
  COUNT="$(printf '%s\n' "$HITS" | grep -c . || true)"
fi

echo "== mock-ban gate (strict=${QA_MOCK_BAN_STRICT:-0}, tool=$qa_search_tool) =="
if [[ -z "$HITS" ]]; then
  echo "OK: no MOCK_HEALTH / MOCK_NODES references in src/"
  exit 0
fi

echo "FAIL: found $COUNT line(s) with MOCK_HEALTH / MOCK_NODES in live code paths:"
echo "$HITS"
echo
echo "Policy (K9/K12): Tauri/live mode must not fall back to MOCK_*."

if [[ "${QA_MOCK_BAN_STRICT:-0}" == "1" ]]; then
  exit 1
fi
echo "(warning mode — set QA_MOCK_BAN_STRICT=1 to fail)"
exit 0
