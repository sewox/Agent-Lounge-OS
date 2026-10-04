#!/usr/bin/env bash
# Fail if Playwright suite reintroduces skip / fixme / expected-fail / only.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"

pattern='test\.(skip|fixme|fail|only)|describe\.(skip|only)|testInfo\.skip'
if rg -n --glob 'e2e/**/*.{ts,tsx,js,jsx}' -e "$pattern" .; then
  echo "playwright-skip-ban: forbidden skip/fixme/fail/only found under e2e/" >&2
  exit 1
fi

echo "playwright-skip-ban: ok (0 skip/fixme/fail/only in e2e/)"
