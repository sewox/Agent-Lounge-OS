# Screenshots — 2026-09-26 UI audit

Captured with `next dev` browser mock IPC (`localhost:3000`).

- `mock__*` — default MOCK_* data from `src/lib/lounge.ts`
- `empty__*` — attempted empty provider state (note: Health/Map still fall back to MOCK_HEALTH / MOCK_NODES)
- Viewports: `1280x800`, `1536x960`

See parent report: [`../2026-09-26-ui-audit.md`](../2026-09-26-ui-audit.md).

Reproduce: `node docs/qa/2026-09-26/_capture.mjs` (requires Playwright + running `npm run dev` on localhost:3000).
