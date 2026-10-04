#!/usr/bin/env bash
# Fail if Playwright suite reintroduces skip / fixme / fail / only.
# Uses shared _search.sh (rg → grep fallback; neither → FAIL; self-test required).
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

# Covers chained modifiers too, e.g.:
#   test.describe.parallel.only / test.describe.serial.only / test.describe.parallel.skip
# Also: test.skip, describe.only, test.describe.fixme, testInfo.skip
PATTERN='(test\.describe|describe|test)(\.[A-Za-z_]+)*\.(skip|fixme|fail|only)|testInfo\.skip'

# Search e2e/ with the chosen tool (qa_search_hits excludes e2e by design).
qa_search_e2e_hits() {
  local pattern="$1"
  case "$qa_search_tool" in
    rg)
      rg -n --glob 'e2e/**/*.{ts,tsx,js,jsx}' -e "$pattern" . 2>/dev/null || true
      ;;
    grep)
      # Portable: walk known e2e trees; ignore binary noise.
      if [[ -d e2e ]]; then
        grep -REn --include='*.ts' --include='*.tsx' --include='*.js' --include='*.jsx' \
          "$pattern" e2e 2>/dev/null || true
      fi
      ;;
    *)
      echo "FAIL: qa_search_tool unset; call qa_search_init first" >&2
      return 2
      ;;
  esac
  return 0
}

# Self-test: planted forbidden forms must be detected (incl. chained .parallel/.serial).
qa_playwright_skip_ban_selftest() {
  local probe dir
  dir="$(mktemp -d "${TMPDIR:-/tmp}/qa-pw-skip-ban.XXXXXX")"
  probe="$dir/probe.spec.ts"
  cat >"$probe" <<'EOF'
test.skip('a', async () => {});
test.fixme('b', async () => {});
test.fail('c', async () => {});
test.only('d', async () => {});
describe.skip('e', () => {});
describe.fixme('f', () => {});
describe.fail('g', () => {});
describe.only('h', () => {});
test.describe.skip('i', () => {});
test.describe.fixme('j', () => {});
test.describe.fail('k', () => {});
test.describe.only('l', () => {});
test.describe.parallel.only('m', () => {});
test.describe.serial.only('n', () => {});
test.describe.parallel.skip('o', () => {});
test.describe.serial.fixme('p', () => {});
testInfo.skip(true, 'q');
EOF
  local hits missing=0
  case "$qa_search_tool" in
    rg)
      hits="$(rg -n -e "$PATTERN" "$probe" 2>/dev/null || true)"
      ;;
    grep)
      hits="$(grep -En "$PATTERN" "$probe" 2>/dev/null || true)"
      ;;
    *)
      rm -rf "$dir"
      echo "FAIL: qa_search_tool unset" >&2
      return 1
      ;;
  esac
  for needle in \
    'test.skip' 'test.fixme' 'test.fail' 'test.only' \
    'describe.skip' 'describe.fixme' 'describe.fail' 'describe.only' \
    'test.describe.skip' 'test.describe.fixme' 'test.describe.fail' 'test.describe.only' \
    'test.describe.parallel.only' 'test.describe.serial.only' \
    'test.describe.parallel.skip' 'test.describe.serial.fixme' \
    'testInfo.skip'
  do
    if ! printf '%s\n' "$hits" | grep -Fq "$needle"; then
      echo "FAIL: playwright-skip-ban self-test missed '$needle' via $qa_search_tool" >&2
      missing=1
    fi
  done
  rm -rf "$dir"
  if [[ "$missing" -ne 0 ]]; then
    echo "FAIL: self-test hits were:" >&2
    echo "$hits" >&2
    return 1
  fi
  return 0
}

if ! qa_playwright_skip_ban_selftest; then
  exit 1
fi

HITS="$(qa_search_e2e_hits "$PATTERN")"
COUNT=0
if [[ -n "$HITS" ]]; then
  COUNT="$(printf '%s\n' "$HITS" | grep -c . || true)"
fi

echo "== playwright-skip-ban (tool=$qa_search_tool) =="
if [[ -n "$HITS" ]]; then
  echo "FAIL: found $COUNT forbidden skip/fixme/fail/only line(s) under e2e/:" >&2
  echo "$HITS" >&2
  exit 1
fi

echo "playwright-skip-ban: ok (0 skip/fixme/fail/only in e2e/)"
