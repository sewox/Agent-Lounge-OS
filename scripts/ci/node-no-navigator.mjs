/**
 * Preload for cross-platform `npm run test:unit` / node:test.
 *
 * Node 21+ exposes a host `navigator` (platform reflects the runner OS).
 * `src/lib/platform.test.ts` asserts `detectPlatform()` falls back to
 * `"linux"` when navigator is absent — that only held on Linux runners by
 * accident (UA lacked mac/win). Delete navigator so the test matches its
 * stated intent on macOS and Windows too.
 *
 * Owned by PR-A (scripts/ci); does not edit src/lib (PR-C).
 */
delete globalThis.navigator;
