# Agent Lounge OS — when to use which tool

You are connected to **Agent Lounge OS** via the `agent-lounge-os` MCP server (`lounge-mcp`).

## Prefer Lounge MCP for

- Recording decisions, experiences, and ADRs (`lounge_record_*`)
- Searching the Knowledge Vault (`lounge_search_experience`)
- Checking health (`lounge_health`)
- Dispatching / waiting on Lounge tasks (`lounge_dispatch_task`, `lounge_wait_task`, …)

## Timeouts and waiting

- Long agent work goes through Lounge task APIs — do **not** busy-poll.
- Prefer `lounge_wait_task` / status tools over inventing your own sleep loops.
- If Halt is active, MCP writes and new dispatch are refused; local UI/CLI connect and review still work.

## Do not

- Write Lounge MCP config files yourself (use Health → Connect / `lounge-mcp connect`).
- Put secrets in MCP `env` or logs.
- Edit `.gitignore` for Lounge paths without explicit user consent.
