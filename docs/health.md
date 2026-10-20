# Health UI (Health-UI-2)

Route: `/health` with tabs `health` | `agents` | `dead` | `ignore`.

## Agents & System (`?tab=agents`)

Shows the Health-1b `lounge_health` report in a **PanelGrid** (list | detail | inspector):

- **List:** agent MCP entries (all clients) plus Title/LMR, CBM, and platform sections
- **Detail:** status, severity, masked config path, command, configured/expected version & sha256, detail; **Şimdi Doğrula** only on `unverified`; **Bağla** / **Onar** (Connect-1) on missing/stale/non_canonical/hash_mismatch
- **Inspector:** raw fields + Connect actions
- **Halt / Durduruldu:** agents tab shows a Halted note; Connect/Repair remain available (local action). See [connect.md](./connect.md).

### Status badges

`StatusPill` tone follows severity (`ok` / `info` / `warning`). Each of the eight statuses has its own i18n label. `unverified` is **Not verified (remote URL)** / **Doğrulanmadı (uzak URL)** — text and icon (`link-off`) differ from `source_unverified` approval chrome.

### Şimdi Doğrula (probe)

- User click only — never automatic, timer, or startup
- Tauri `probe_agent_mcp_url(client, config_path_id)` re-reads the URL from disk (never from the UI)
- Loopback only (`127.0.0.1`, `::1`, `localhost`); anything else → `not_probed_remote` without connecting
- Timeouts: 2s connect, 3s total; single request; no redirects
- Result is transient (`reachable` / `unreachable` / `not_probed_remote` + ms); report status stays `unverified`; schema unchanged
- Logs: scheme + host + port only

### Path masking

Backend emits `~/…`. UI applies `maskHomePathForUi` as a second defence (proven with an unmasked fixture in e2e).

### States

Loading skeleton · empty (Connect-1 explanation) · error + Retry · non-Tauri browser mode.

## Dead symbols (`?tab=dead`)

StatsStrip (total / unused / broken / partial) — each number is a filter. Category chips, Kritik/Potansiyel confidence filter, plain-language reasons. Prominent stale warning when the scan is partial or `indexed_at` is old. No Apply / auto-delete / send-to-agent in this PR.

## Startup log (N1)

`agents_warn` counts only `severity=warning`. `agents_info` counts `severity=info`.

## HO-6

Existing **HealthPanel** (project index table) stays on `?tab=health` — not moved into PanelGrid in this PR.
