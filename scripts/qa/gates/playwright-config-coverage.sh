#!/usr/bin/env bash
# Gate: playwright.config.ts must not silently exclude tests (grep / testIgnore).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$ROOT"
exec node --experimental-strip-types scripts/qa/gates/playwright-config-coverage.mjs
