#!/usr/bin/env bash
# Hydration safety gate: ban SSR/client mismatch patterns in render paths.
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

FAIL=0

echo "== hydration-safety gate (strict=${QA_HYDRATION_SAFETY_STRICT:-1}) =="

# 1) paletteShortcutLabel() must receive an explicit platform (no zero-arg calls).
ZERO_ARG_PALETTE="$(qa_search_hits 'paletteShortcutLabel\(\)' src/components src/app 2>/dev/null || true)"
if [[ -n "$ZERO_ARG_PALETTE" ]]; then
  echo "FAIL: paletteShortcutLabel() called without platform argument (use usePlatform + paletteShortcutLabel(platform)):"
  echo "$ZERO_ARG_PALETTE"
  FAIL=1
fi

# 2) detectPlatform() only in platform.ts / hooks / tests.
DETECT_OUTSIDE="$(qa_search_hits 'detectPlatform\(\)' src 2>/dev/null | grep -Ev '(^|/)src/lib/platform\.ts|(^|/)src/hooks/use-platform\.ts|(^|/)src/lib/platform\.test\.ts' || true)"
if [[ -n "$DETECT_OUTSIDE" ]]; then
  echo "FAIL: detectPlatform() used outside platform module/hook:"
  echo "$DETECT_OUTSIDE"
  FAIL=1
fi

# 3) isTauri() in JSX render — use useIsTauri() instead.
TAURI_IN_JSX="$(qa_search_hits 'isTauri\(\)' src/components src/app 2>/dev/null | grep -E '\{.*isTauri\(\)' || true)"
if [[ -n "$TAURI_IN_JSX" ]]; then
  echo "FAIL: isTauri() referenced inside JSX (use useIsTauri hook):"
  echo "$TAURI_IN_JSX"
  FAIL=1
fi

# 4) suppressHydrationWarning only allowed on <html> in layout.tsx.
SUPPRESS="$(qa_search_hits 'suppressHydrationWarning' src 2>/dev/null | grep -Ev '(^|/)src/app/layout\.tsx' || true)"
if [[ -n "$SUPPRESS" ]]; then
  echo "FAIL: suppressHydrationWarning used outside root layout <html>:"
  echo "$SUPPRESS"
  FAIL=1
fi

# 5) toLocaleString() must pass explicit locale in TSX render paths.
LOCALE_STRING="$(qa_search_hits '\.toLocaleString\(\)' src/components src/app 2>/dev/null || true)"
if [[ -n "$LOCALE_STRING" ]]; then
  echo "FAIL: toLocaleString() without explicit locale in UI code:"
  echo "$LOCALE_STRING"
  FAIL=1
fi

if [[ "$FAIL" -eq 0 ]]; then
  echo "OK: hydration safety patterns clean"
  exit 0
fi

if [[ "${QA_HYDRATION_SAFETY_STRICT:-1}" == "1" ]]; then
  exit 1
fi
echo "(warning mode — set QA_HYDRATION_SAFETY_STRICT=1 to fail)"
exit 0
