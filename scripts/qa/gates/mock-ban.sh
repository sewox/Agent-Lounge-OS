#!/usr/bin/env bash
# S4 mock-ban gate: MOCK_HEALTH / MOCK_NODES must not remain in live code paths.
# PR-0: warning mode (exit 0, prints findings). PR-2: set QA_MOCK_BAN_STRICT=1 to fail.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"

PATTERN='MOCK_HEALTH|MOCK_NODES'
# Live UI paths that still reference mocks (known until PR-2).
HITS=$(rg -n --glob '!e2e/**' --glob '!docs/**' --glob '!scripts/**' \
  -g '*.ts' -g '*.tsx' -g '*.js' -g '*.jsx' \
  "$PATTERN" src || true)

COUNT=$(printf '%s' "$HITS" | grep -c . || true)
echo "== mock-ban gate (warning mode) =="
if [[ -z "$HITS" ]]; then
  echo "OK: no MOCK_HEALTH / MOCK_NODES references in src/"
  exit 0
fi

echo "WARN: found $COUNT line(s) with MOCK_HEALTH / MOCK_NODES in live code paths:"
echo "$HITS"
echo
echo "Policy (K9/K12): Tauri/live mode must not fall back to MOCK_*. PR-2 converts this gate to fail."

if [[ "${QA_MOCK_BAN_STRICT:-0}" == "1" ]]; then
  exit 1
fi
exit 0
