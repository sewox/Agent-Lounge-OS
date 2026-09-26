#!/usr/bin/env bash
# Shared search helper for S4 gates: prefer `rg`, else `grep -E`.
# Exits 2 if neither tool is available (callers must not treat that as "no hits").
#
# Usage:
#   source "$(dirname "$0")/_search.sh"
#   qa_search_init          # fail fast if no tool
#   qa_search_selftest      # must detect a known pattern or exit 1
#   qa_search_hits PATTERN [PATH...]   # prints matches; exit 0 always if tool works
#                                      # (empty stdout = no hits)

qa_search_tool=""

qa_search_init() {
  if command -v rg >/dev/null 2>&1; then
    qa_search_tool="rg"
  elif command -v grep >/dev/null 2>&1; then
    qa_search_tool="grep"
  else
    echo "FAIL: neither rg nor grep is available on PATH" >&2
    return 2
  fi
  return 0
}

# Sanity: the chosen tool must find a known string (prevents false "OK" when broken).
qa_search_selftest() {
  local probe
  probe="$(mktemp "${TMPDIR:-/tmp}/qa-gate-probe.XXXXXX")"
  printf '%s\n' "QA_GATE_SELFTEST_TOKEN_7f3a9c" >"$probe"
  local found=0
  case "$qa_search_tool" in
    rg)
      if rg -n "QA_GATE_SELFTEST_TOKEN_7f3a9c" "$probe" >/dev/null 2>&1; then
        found=1
      fi
      ;;
    grep)
      if grep -E -n "QA_GATE_SELFTEST_TOKEN_7f3a9c" "$probe" >/dev/null 2>&1; then
        found=1
      fi
      ;;
    *)
      rm -f "$probe"
      echo "FAIL: qa_search_tool unset; call qa_search_init first" >&2
      return 1
      ;;
  esac
  rm -f "$probe"
  if [[ "$found" -ne 1 ]]; then
    echo "FAIL: gate self-test could not detect known pattern via $qa_search_tool" >&2
    return 1
  fi
  return 0
}

# Search PATHS (default: .) for PATTERN. Prints matching lines to stdout.
# Return code: 0 if tool ran (whether or not matches), 2 if tool missing.
qa_search_hits() {
  local pattern="$1"
  shift
  local paths=("$@")
  if [[ ${#paths[@]} -eq 0 ]]; then
    paths=(.)
  fi
  case "$qa_search_tool" in
    rg)
      # rg exits 1 when no matches — normalize to 0 for callers that only inspect stdout.
      rg -n --glob '!e2e/**' --glob '!docs/**' --glob '!scripts/**' \
        -g '*.ts' -g '*.tsx' -g '*.js' -g '*.jsx' -g '*.css' \
        "$pattern" "${paths[@]}" 2>/dev/null || true
      ;;
    grep)
      # Portable recursive grep; ignore binary noise.
      grep -REn --include='*.ts' --include='*.tsx' --include='*.js' \
        --include='*.jsx' --include='*.css' \
        --exclude-dir=e2e --exclude-dir=docs --exclude-dir=scripts \
        --exclude-dir=node_modules --exclude-dir=.next --exclude-dir=out \
        --exclude-dir=playwright-report --exclude-dir=test-results \
        "$pattern" "${paths[@]}" 2>/dev/null || true
      ;;
    *)
      echo "FAIL: qa_search_tool unset; call qa_search_init first" >&2
      return 2
      ;;
  esac
  return 0
}
