# mcp-probe

Independent MCP client-behavior probe for Agent Lounge OS §16 wait-mode research.

- **Does not touch product code** (`src-tauri`, `bridge`, dashboard).
- Transports: **stdio** and **Streamable HTTP / SSE**.
- Tools: `ping`, `slow_echo(delay_ms, progress?)`.
- Per-client **JSONL** logs + `run-matrix` / markdown report.

Full usage (per-client MCP configs): [`docs/qa/mcp-probe.md`](../../docs/qa/mcp-probe.md).

```bash
cd tools/mcp-probe
npm test
node bin/mcp-probe.mjs --transport stdio --client-label cursor
node bin/mcp-probe.mjs --transport http --port 19891
MCP_PROBE_DELAYS=0.05,0.1 npm run run-matrix   # short harness
npm run run-matrix                               # 5..120s matrix
npm run run-antigravity-cliff                    # MANUAL: 150/170/180/190s (~20+ min, not CI)
```

### Antigravity cliff (manual)

`scripts/run-antigravity-cliff.mjs` exercises delays **150, 170, 180, 190** s (plain + progress) to confirm the measured **180 s** hard timeout and that progress does not extend it. **Do not add to CI** — wall time is tens of minutes.
