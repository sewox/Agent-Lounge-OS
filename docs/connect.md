# Connect-1 — Agent MCP binding (F1)

Local **Connect / Repair / Disconnect / Print** for desktop and CLI agents. Writes only the canonical `agent-lounge-os` MCP server key (command = `lounge-mcp`) and optional rule/skill files. Never an MCP tool or deep link.

## Entrypoints

| Surface | How |
|---------|-----|
| Health UI (`/health?tab=agents`) | **Bağla** / **Onar** → plan dialog → confirm |
| CLI | `lounge-mcp connect\|disconnect\|print <agent> [--dry-run] [--global\|--project] [--force-lossy-jsonc]` |

Default scope is **`--global`**. `--project` is opt-in.

## Clients (parity with `lounge_health`)

Antigravity, Cursor, Claude Desktop, Claude Code, Grok Bot, VS Code, Windsurf.

Shared path table: `src-tauri/src/services/agent_mcp_paths.rs` (read + write).

## Rules / skills (CC-3)

| Client | Artifact |
|--------|----------|
| Cursor | `.cursor/rules/agent-lounge.mdc` |
| Claude Code | `.claude/skills/agent-lounge/SKILL.md` (never edits `CLAUDE.md`) |
| Antigravity | project `.agents/rules/…` or `~/.gemini/…` fallback |
| Claude Desktop / Grok Bot | MCP server instructions only — UI still shows **connected** equally |

Rule text source: `docs/agent-rules/agent-lounge.md` (English).

## Safety

- Atomic write: temp + fsync + rename; Windows `ReplaceFileW` (no rename retries)
- Timestamped backup: `<file>.lounge-bak-<UTC>`
- Touches only `agent-lounge-os`; never `.gitignore`
- JSONC with comments: refuse unless UI **Yine de Yaz** / CLI `--force-lossy-jsonc`; else **Sadece Ekrana Bas** / `print`
- Allowed while **Halt / Durduruldu** (local user action)
- Logs mask home paths; no config/env secrets
- Respects `XDG_CONFIG_HOME` / `%APPDATA%` / macOS Application Support
- Create missing file only for `--global` when the parent directory already exists

## Tauri commands

`plan_agent_connect_cmd`, `connect_agent`, `disconnect_agent`, `print_agent_connect` — Halted OK; not registered as MCP tools.

## Smoke (CI)

`scripts/ci/connect-cli-smoke.sh` — temporary `HOME` / `XDG_CONFIG_HOME`, `lounge-mcp print cursor`, dry-run connect, connect, disconnect on Linux/macOS/Windows (mcp-probe + installer jobs).
