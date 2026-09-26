#!/usr/bin/env bash
# S4 small-font gate: ban Tailwind arbitrary fonts below 12px.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"

PATTERN='text-\[(9|10|10\.5|11)px\]'
HITS=$(rg -n --glob '!e2e/**' --glob '!docs/**' --glob '!scripts/**' \
  -g '*.ts' -g '*.tsx' -g '*.css' \
  "$PATTERN" src || true)

echo "== small-font gate =="
if [[ -z "$HITS" ]]; then
  echo "OK: no text-[9|10|10.5|11px] classes in src/"
  exit 0
fi

echo "FAIL: sub-12px font utilities found:"
echo "$HITS"
exit 1
