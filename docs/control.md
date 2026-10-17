# Control plane — Halt v1 + deep links

PR-Control-1 adds an operator **Halt** switch and a strict `agentlounge://` deep-link gate.

## Halt v1

### Source of truth

Kernel SQLite `settings` key `halt_state` (JSON):

```json
{ "halted": true, "reason": "optional", "by": "ui", "at": "RFC3339" }
```

Tauri commands (local UI only):

| Command | Role |
|---------|------|
| `get_halt_state` | Read persistent state |
| `halt(reason?)` | Engage Halt (`by=ui`) |
| `resume()` | Clear Halt (`by=ui`) |

Event: `lounge://halt-changed` (payload = `HaltState`).

Halt and Resume **cannot** be triggered via deep link or MCP — only the local UI after confirmation. Both actions are logged (reason presence only; no secrets).

**Fail-closed:** any storage or parse error reading `halt_state` is treated as halted (gates refuse dispatch). `get_halt_state` returns the error to the UI, which shows a load-error chrome instead of “not halted”.

### While halted

| Action | Behavior |
|--------|----------|
| New dispatch (NATS / MCP `lounge_dispatch_task` / `lounge_call_agent` / …) | Refused (before LMR embedding) |
| MCP write tools (`lounge_record_*`) | Refused with structured `halted` error |
| LMR title worker / backfill | Parks jobs (not dropped); Resume wakes worker and runs deferred startup backfill |
| Automatic background health scans | Not started |
| Read tools (`lounge_status`, `lounge_health`, search) | Keep working |
| In-flight / pending tasks | **Not cancelled** (NATS cancel is v2) |
| External agent sessions (Cursor, Antigravity, Claude Desktop, Grok Bot) | **Not stopped** |

UI: Halt/Resume in the top bar on every page; full-width `HaltedBanner` stacked **above** the existing approval / degraded / indexing banner chain (so SourceUnverified Approve/Deny stays visible); status strip uses the halted colour (`StatusPill tone="halted"`).

### MCP `halted` error

See [mcp.md](./mcp.md). Code `halted`, `retryable: true`, message states this is a temporary pause and that in-flight work / external sessions continue.

## Deep links — `agentlounge://`

Scheme name is **`agentlounge://`** (not `lounge://`, which is reserved for internal Tauri events and MCP resource URIs such as `lounge://help/a2a-guide`).

### Allowlist

| URL | Effect | Approval |
|-----|--------|----------|
| `agentlounge://open?route=/health` (exact allowlisted route) | Navigate only | None |
| `agentlounge://dispatch?agent=<registered id>&text=<≤2000 chars>` | Create task `source_verified=false` | Always `SourceUnverified` → `PENDING_APPROVAL` + **Dış Kaynaklı Tetikleme** warning |

Allowed `open` routes: `/`, `/dashboard`, `/stream`, `/vault`, `/health`, `/fleet`, `/telemetry`, `/quotas`, `/settings`, `/onboarding`.

Rejected (logged **without** parameter values):

- Unknown actions (`halt`, `resume`, `shell`, `exec`, …) or unknown query params (e.g. `task=rm -rf /`)
- Over-length `text`, empty required fields, control characters, Unicode format/bidi (Cf) characters, malformed percent-encoding
- Dispatch while Halt is active; unregistered or disabled agent ids

No deep link may run a shell command, execute a path, write config, or Halt/Resume.

### Registration

| Platform | Mechanism |
|----------|-----------|
| macOS | `CFBundleURLTypes` in `Info.plist` (bundle time; from `plugins.deep-link.desktop.schemes`) |
| Windows | `HKCU\Software\Classes\agentlounge` via NSIS/MSI |
| Linux `.deb` | `.desktop` `MimeType=x-scheme-handler/agentlounge;` |
| Linux AppImage | Settings → **Enable desktop integration** / **Masaüstü Entegrasyonunu Etkinleştir** (runtime `register`, user consent). Uses `xdg-mime` / desktop files — **no apt/Debian assumption**. |
| Dev (`tauri dev`) | **Does not** register the scheme automatically |

Plugins: `tauri-plugin-deep-link` + `tauri-plugin-single-instance` (feature `deep-link`) so Windows/Linux URL argv reaches the first process.

## Security model

1. Strict allowlist parser before any side effect.
2. Dispatch always unverified → existing `ApprovalKind::SourceUnverified` gate in `dispatcher.rs`.
3. Prominent UI chrome: **Dış Kaynaklı Tetikleme** / External trigger.
4. Rejection logs omit query values.
5. Halt is a local operator control only.
