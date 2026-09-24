# Agent Lounge OS

**Local multi-agent orchestration layer** — low-latency communication between local AI agents (Claude, Cursor, Grok, Llama/Ollama, and more).

Built with **Rust + Tauri + Next.js**. Designed for privacy-aware, on-device agent workflows.

## What it does

- **Lounge Kernel** (Rust / Tokio) — task dispatch and agent lifecycle
- **NATS** — local pub/sub messaging bus between agents
- **Ollama integration** — service start/stop and model routing
- **Unified memory / experience store** — reusable outcomes across sessions
- **Desktop shell** — single Tauri binary, engineering-focused dashboard

## Related project

[EchoMind](https://github.com/sewox/EchoMind) — privacy-first AI meeting assistant (same local-first stack philosophy).

## Development

```bash
npm install
npm run tauri dev
