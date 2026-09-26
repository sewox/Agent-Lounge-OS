# Lounge MCP — Dış ajan bağlantısı

Agent Lounge OS, Cursor ve Claude Desktop’a **Model Context Protocol (MCP)** sunucusu olarak açılır. Taşıma: **stdio** (JSON-RPC 2.0, satır sonu ile ayrılmış). Bu, her iki istemcinin de resmi olarak desteklediği yoldur.

## Mimari

| Bileşen | Rol |
|---------|-----|
| `lounge-mcp` binary | Stdio MCP sunucusu (`src-tauri/src/bin/lounge_mcp.rs`) |
| `bridge/mcp_server.rs` | Protokol + tool işleyicileri |
| Experience store (SQLite) | Arama / kayıt — **UI gerekmez** |
| NATS `lounge.task.requested` | Görev dağıtımı — **Kernel + NATS gerekir** |

```text
Cursor / Claude Desktop
        │  stdio (MCP)
        ▼
   lounge-mcp
        ├─► SQLite experiences  (search / record)
        └─► NATS 4222 ──► Lounge Kernel (DecisionGate, security, quota)
```

**Önemli:** `lounge_dispatch_task` / `lounge_ask_agent` politikayı baypas etmez. Görev bus’a yazılır; Kernel dinliyorsa güvenlik (`PENDING_APPROVAL`) ve kota kapıları uygulanır. Yalnızca MCP çalışıyorsa arama/kayıt yine çalışır; dispatch NATS’a ulaşamazsa hata döner.

Bağlanan istemci `connected_tools` tablosuna (`app:cursor`, `app:claude_desktop`, …) yazılır; dashboard’daki bağlı ajanlar listesinde görünür.

## Binary derleme

```bash
cargo build --manifest-path src-tauri/Cargo.toml --bin lounge-mcp --release
```

Çıktı (örnek):

- `src-tauri/target/release/lounge-mcp`

Geliştirme:

```bash
cargo run --manifest-path src-tauri/Cargo.toml --bin lounge-mcp
```

### Ortam değişkenleri

| Değişken | Anlam |
|----------|--------|
| `LOUNGE_DB_PATH` / `LOUNGE_EXPERIENCE_DB` | Experience SQLite yolu (yoksa `experiences/lounge.sqlite`) |
| `LOUNGE_NATS_URL` | Varsayılan `nats://127.0.0.1:4222` |

## Cursor — `.cursor/mcp.json`

Proje köküne veya kullanıcı MCP ayarına yapıştırın (`command` yolunu kendi binary’nize göre düzeltin):

```json
{
  "mcpServers": {
    "agent-lounge-os": {
      "command": "/ABS/PATH/TO/Agent-Lounge-OS/src-tauri/target/release/lounge-mcp",
      "args": [],
      "env": {
        "LOUNGE_DB_PATH": "/ABS/PATH/TO/Agent-Lounge-OS/experiences/lounge.sqlite",
        "LOUNGE_NATS_URL": "nats://127.0.0.1:4222"
      }
    }
  }
}
```

## Claude Desktop — `claude_desktop_config.json`

macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`

```json
{
  "mcpServers": {
    "agent-lounge-os": {
      "command": "/ABS/PATH/TO/Agent-Lounge-OS/src-tauri/target/release/lounge-mcp",
      "args": [],
      "env": {
        "LOUNGE_DB_PATH": "/ABS/PATH/TO/Agent-Lounge-OS/experiences/lounge.sqlite",
        "LOUNGE_NATS_URL": "nats://127.0.0.1:4222"
      }
    }
  }
}
```

Claude’u yeniden başlattıktan sonra araçlar listesinde Lounge tool’larını görmelisiniz.

## Tool’lar

| Tool | Açıklama |
|------|----------|
| `lounge_search_experience` | `query`, isteğe bağlı `project` — tecrübe / ADR ara |
| `lounge_record_experience` | `project` + `context` / `decision` — yeni tecrübe |
| `lounge_record_decision` | Aynı iş — ADR alias |
| `lounge_dispatch_task` | `target_agent`, `task`, `project` → NATS |
| `lounge_ask_agent` | Dispatch alias |
| `lounge_status` | Bağlı ajanlar + NATS/LMR sağlık |

## Manuel duman testi

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"cursor","version":"1"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"lounge_status","arguments":{}}}' \
  | cargo run --manifest-path src-tauri/Cargo.toml --bin lounge-mcp --quiet
```

Otomatik test: `bridge::mcp_server::tests::stdio_transport_roundtrip` (duplex stdio framing + experience store).

## Sonraki adım (bu PR dışı)

NATS worker şablonu (`workers/bot_template.py`) — Grok / özel botların `lounge.tasks.<bot>` dinlemesi.
