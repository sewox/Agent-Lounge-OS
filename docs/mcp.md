# Lounge MCP — Dış ajan bağlantısı

Agent Lounge OS, Cursor ve Claude Desktop’a **Model Context Protocol (MCP)** ile açılır.

## Mimari (Connectivity Phase)

| Katman | Rol |
|--------|-----|
| **Kernel HTTP** `127.0.0.1:18791` | Tauri uygulaması ayaktayken MCP JSON-RPC (`POST /mcp`) + SSE nabız (`GET /mcp/sse`) |
| **`lounge-mcp` stdio shim** | Claude Desktop / Cursor `command` ile başlatır → Kernel HTTP’ye proxy |
| **Standalone fallback** | Kernel kapalıysa aynı binary gömülü SQLite modunda çalışır (`LOUNGE_MCP_STANDALONE=1` zorlar) |

```text
Cursor / Claude Desktop
        │  stdio (MCP)
        ▼
   lounge-mcp  ──proxy──►  Kernel HTTP :18791  ──►  aynı ExperienceStore (dashboard sync)
        │                         │
        │ (fallback)              ├── NATS task.requested → DecisionGate / security / quota
        └── SQLite experiences ◄──┘
```

**Güvenlik sınırı (bilinçli):** MCP dosya sistemi yazmaz; kota / politika / routing ayarlarını değiştirmez. Bunlar yalnızca Tauri UI’dan değişir.

**Atomik kayıt:** `lounge_record_*` SQLite (+ yerel embedding) yazar; `LOUNGE_QDRANT_URL` / `LOUNGE_CHROMA_URL` varsa uzak indeksi de bekler — uzak yazım başarısızsa SQLite satırı geri alınır.

**Otomatik project_id:** `active_file` veya `workspace_root` verildiğinde `project_index.repo_path` üzerinden en uzun eşleşen proje seçilir.

## Binary derleme

```bash
cargo build --manifest-path src-tauri/Cargo.toml --bin lounge-mcp --release
```

### Ortam değişkenleri

| Değişken | Anlam |
|----------|--------|
| `LOUNGE_MCP_URL` / `LOUNGE_MCP_BIND` | Kernel HTTP (varsayılan `http://127.0.0.1:18791`) |
| `LOUNGE_MCP_STANDALONE=1` | Proxy’yi atla; gömülü sunucu |
| `LOUNGE_DB_PATH` / `LOUNGE_EXPERIENCE_DB` | Standalone SQLite |
| `LOUNGE_NATS_URL` | Varsayılan `nats://127.0.0.1:4222` |

## Cursor — `.cursor/mcp.json`

```json
{
  "mcpServers": {
    "agent-lounge-os": {
      "command": "/ABS/PATH/TO/Agent-Lounge-OS/src-tauri/target/release/lounge-mcp",
      "args": [],
      "env": {
        "LOUNGE_MCP_URL": "http://127.0.0.1:18791",
        "LOUNGE_DB_PATH": "/ABS/PATH/TO/Agent-Lounge-OS/experiences/lounge.sqlite",
        "LOUNGE_NATS_URL": "nats://127.0.0.1:4222"
      }
    }
  }
}
```

Dashboard ile senkron için **Agent Lounge OS uygulamasını açık tutun**; shim otomatik HTTP’ye bağlanır.

## Claude Desktop — `claude_desktop_config.json`

macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`

```json
{
  "mcpServers": {
    "agent-lounge-os": {
      "command": "/ABS/PATH/TO/Agent-Lounge-OS/src-tauri/target/release/lounge-mcp",
      "args": [],
      "env": {
        "LOUNGE_MCP_URL": "http://127.0.0.1:18791",
        "LOUNGE_DB_PATH": "/ABS/PATH/TO/Agent-Lounge-OS/experiences/lounge.sqlite",
        "LOUNGE_NATS_URL": "nats://127.0.0.1:4222"
      }
    }
  }
}
```

Claude yalnızca stdio başlatır; `lounge-mcp` Kernel HTTP’ye köprü kurar.

## Tool’lar

| Tool | Şema | Not |
|------|------|-----|
| `lounge_search_experience` | `mcp_search_experience.schema.json` | `active_file` / `workspace_root` → project |
| `lounge_record_experience` | MCP args + `experience.schema.json` | Atomik SQLite↔vektör |
| `lounge_record_decision` | aynı | ADR alias |
| `lounge_call_agent` | `mcp_call_agent.schema.json` | Mod A→B; `timeout_limit` içinde sonuç veya `backgrounded` |
| `lounge_wait_task` | `mcp_wait_task.schema.json` | Aynı oturum long-poll; eşik aşımında `still_running` |
| `lounge_yield_result` | `mcp_yield_result.schema.json` | Claim sahibi worker oturumu yazar |
| `lounge_dispatch_task` | `mcp_dispatch_task.schema.json` | Fire-and-forget (yeni A2A altyapısı); sonuç → `lounge_wait_task` |
| `lounge_ask_agent` | aynı | Dispatch alias |
| `lounge_status` | `mcp_status.schema.json` | Bağlı ajanlar + sağlık + `timeout_limit_secs` |

Serbest biçim / `additionalProperties` → net MCP `isError` yanıtı.

## Timeout manager (PR-3)

Oturum başına tek eşik: `timeout_limit` (`bridge/timeout_manager.rs`).

| İstemci (`clientInfo.name` → normalize) | Varsayılan |
|---|---|
| Antigravity | **150 sn** (180 sn sert limit − 30 sn marj) |
| Cursor / Claude Desktop / Claude Code / Grok | **45 sn** (geçici; ölçüm bekleniyor) |
| Bilinmeyen | **30 sn** (güvenli düşük) |

Override: Settings `mcp.timeout_secs` veya env `LOUNGE_MCP_TIMEOUT_SECS` (global). Tablo `TimeoutManager::set_client_timeout` ile güncellenir.

**Progress bildirimleri süreyi uzatmaz** — yalnız UI nabzı.

### `notifications/cancelled`

| reason | Davranış |
|---|---|
| `context canceled` (kullanıcı Stop) | Görev `CANCELLED`; NATS `lounge.control.stop` |
| `context deadline exceeded` | Görev arka planda sürer (`WAIT_TIMEOUT_REACHED` / backgrounded) |

### Sonuç teslimi (deadline sonrası, bağlantı açık)

Ölçüm (Antigravity): zaman aşımında istemci `cancelled` gönderir, **bağlantıyı kapatmaz**. Bridge yanıtı zaten `backgrounded` + `task_id` döndürmüştür (veya iptal sonrası sessiz). Aynı `Mcp-Session-Id` ile ajan `lounge_wait_task(task_id)` çağırır; worker `lounge_yield_result` yazmışsa `completed`/`failed` + sonuç döner. Yetkisiz oturum okuyamaz/yield edemez.

## Manuel duman testi

```bash
# Kernel ayaktayken (tercih):
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"cursor","version":"1"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"lounge_status","arguments":{}}}' \
  | cargo run --manifest-path src-tauri/Cargo.toml --bin lounge-mcp --quiet

# Standalone:
LOUNGE_MCP_STANDALONE=1 cargo run --manifest-path src-tauri/Cargo.toml --bin lounge-mcp
```

## Follow-up

NATS worker şablonu: [`workers/`](../workers/README.md) — `bot_template.py`, `grok_tester.py`.
Botlar `lounge.workers.register` ile kaydolur; onaylı görevler `lounge.tasks.<bot_id>` kutusuna düşer.
