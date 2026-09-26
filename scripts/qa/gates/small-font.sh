#!/usr/bin/env bash
# S4 small-font gate: ban Tailwind arbitrary fonts below 12px.
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

# Extra self-test: must detect a synthetic sub-12px class token.
PROBE="$(mktemp "${TMPDIR:-/tmp}/qa-font-probe.XXXXXX")"
printf '%s\n' 'className="text-[10px] font-mono"' >"$PROBE"
FONT_PROBE_HIT=0
case "$qa_search_tool" in
  rg) rg -n 'text-\[(9|10|10\.5|11)px\]' "$PROBE" >/dev/null 2>&1 && FONT_PROBE_HIT=1 || true ;;
  grep) grep -E -n 'text-\[(9|10|10\.5|11)px\]' "$PROBE" >/dev/null 2>&1 && FONT_PROBE_HIT=1 || true ;;
esac
rm -f "$PROBE"
if [[ "$FONT_PROBE_HIT" -ne 1 ]]; then
  echo "FAIL: small-font self-test could not detect text-[10px] via $qa_search_tool" >&2
  exit 1
fi

PATTERN='text-\[(9|10|10\.5|11)px\]'
HITS="$(qa_search_hits "$PATTERN" src || true)"

echo "== small-font gate (tool=$qa_search_tool) =="
if [[ -z "$HITS" ]]; then
  echo "OK: no text-[9|10|10.5|11px] classes in src/"
  exit 0
fi

echo "FAIL: sub-12px font utilities found:"
echo "$HITS"
exit 1
