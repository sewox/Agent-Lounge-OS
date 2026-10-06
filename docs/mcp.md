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

## Graph UI port (masaüstü ajanlar)

Lounge’un codebase-memory-mcp **3D Graph UI** dinleme portu Antigravity cbm varsayılanı **9749 değildir**. Lounge bandı **18749–18759** (auto: ilk boş; user: Settings’te sabit). Gerçek portu Tauri `get_graph_ui_status` / Settings’ten okuyun — Grok Bot, Cursor, Antigravity, Claude Desktop **9749 varsaymamalı**. Yabancı bir cbm instance’ı asla adopt edilmez; HTTP `/rpc` yalnız Lounge’un sahip olduğu portta kullanılır.

## Tool’lar

| Tool | Şema | Not |
|------|------|-----|
| `lounge_search_experience` | `mcp_search_experience.schema.json` | `active_file` / `workspace_root` → project |
| `lounge_record_experience` | MCP args + `experience.schema.json` | Atomik SQLite↔vektör |
| `lounge_record_decision` | aynı | ADR alias |
| `lounge_call_agent` | `mcp_call_agent.schema.json` | Mod A→B; `timeout_limit` içinde sonuç veya `backgrounded` |
| `lounge_wait_task` | `mcp_wait_task.schema.json` | Aynı oturum long-poll; eşik aşımında `still_running` |
| `lounge_list_my_tasks` | `mcp_list_my_tasks.schema.json` | `source_session_id` (+ isteğe bağlı workspace) sahipliğindeki görevler |
| `lounge_yield_result` | `mcp_yield_result.schema.json` | `target_agent` = normalize(`clientInfo.name`) ile `agent_sessions` bağlı oturum (initialize kaydı); terminal ezilemez |
| `lounge_dispatch_task` | `mcp_dispatch_task.schema.json` | Fire-and-forget (yeni A2A altyapısı); sonuç → `lounge_wait_task` |
| `lounge_ask_agent` | aynı | Dispatch alias |
| `lounge_status` | `mcp_status.schema.json` | Bağlı ajanlar + sağlık + `timeout_limit_secs` |

Serbest biçim / `additionalProperties` → net MCP `isError` yanıtı.

## Timeout manager (PR-3 / PR-3b)

Oturum başına tek eşik: `timeout_limit` (`bridge/timeout_manager.rs` → [`ClientProfile`]).

Ölçüm matrisi (2026-10-04/05, mcp-probe; `clientInfo.name`):

| Profil | hard_limit | threshold | progress_extends | sends_cancel | kaynak |
|---|---:|---:|:---:|:---:|---|
| `antigravity-client` | 180 | 150 | hayır | evet | measured |
| `cursor-vscode` | 120 | 100 | evet → ≤280 sn (10 sn heartbeat) | hayır (Stop) | measured |
| `claude-ai` (Desktop) | 240 | 210 | hayır (token yok) | hayır (Stop) | measured |
| `claude-code` | — (bilinmiyor; 3600 yazılmaz) | 300 | hayır | hayır (SIGINT=EOF) | assumed |
| `unknown` / Grok Bot / bulut | — | **45** | hayır | — | assumed |

Not: Gemini Grok için 60 sn demişti; tutarlılık için bilinmeyen=45. Cursor progress ile 300 sn ölçüldü; Lounge 280 sn (20 sn marj).

Override: Settings `mcp.timeout_secs` veya env `LOUNGE_MCP_TIMEOUT_SECS`. Üst sınır **profil başına** `hard_limit − 20` (bilinmeyen: 170). Kullanıcı override’ı profil tavanını aşamaz (uyarı log).

Yanıtlarda teşhis: `client_profile`, `timeout_limit_secs`, `profile_source`.

### Bağlantı kopması (cancel göndermeyen istemciler)

stdio EOF / HTTP `DELETE /mcp` / oturum kapanışı → in-flight çağrılara `SessionDisconnect`:
- **Mod A senkron** (`lounge_call_agent` wait): varsayılan → `CANCELLED` + `lounge.control.stop`; **`must_deliver=true`** → arka plan
- **`lounge_wait_task` long-poll**: yalnız bekleme biter (`still_running`); görev **iptal edilmez** (backgrounded korunur)

**Backgrounded** görevler in-flight map'te yoktur → kopmada dokunulmaz. İstemci `lounge_wait_task` ile dönebilir.

`WAIT_TIMEOUT_REACHED` sessizlik taramasından **muaf** (eşik > 120 sn profilde yanlış NEEDS_HUMAN yok).

Terminal sonuç okununca `last_wait_at` yenilenir (teslim) → orphan EXPIRED / must_deliver abandoned uygulanmaz.

Stdio HTTP proxy timeout varsayılan **320 sn** (`LOUNGE_MCP_PROXY_TIMEOUT_SECS`); hata/timeout → istemciye JSON-RPC error.

### Orphan TTL (yedek temizlik)

| Tür | Varsayılan | Davranış |
|---|---|---|
| Sonuç orphan | **30 dk** (`LOUNGE_RESULT_ORPHAN_TTL_SECS`) | Backgrounded → sonuç hazır, hiç `lounge_wait_task` yok → `EXPIRED` (çalışanı öldürmez). **must_deliver atlanır** |
| Incomplete orphan | **30 dk** (`LOUNGE_INCOMPLETE_ORPHAN_TTL_SECS`) | Çağıran oturum yok/disconnected + sorgu yok → `EXPIRED` + `control.stop`. **must_deliver atlanır** |
| must_deliver | **24 sa** (`LOUNGE_MUST_DELIVER_TTL_SECS`) | Alınmayan → `FAILED` reason `abandoned`. Kota: oturum 5 / ajan 10 açık; aşımda JSON-RPC **-32029** (sessiz düşürme yok) |

`long_running=true`: eşik beklenmeden hemen `backgrounded` + `task_id`.

`poll_after_secs`: long_running/backgrounded için 15 sn başlar, her boş wait’te ×1.5, en fazla 60. Yanıtta `next_action`: `lounge_wait_task(task_id) ile tekrar kontrol et`.

Yeniden bağlanma: `task_token` (düz, bir kez) + DB hash; `lounge_wait_task` aynı oturum **veya** geçerli token. Workspace paylaşımı yok.

PR-4 (uygulandı): `lounge_list_my_tasks`, `_meta.pending_results` piggyback (Mcp-Session-Id), Resource `lounge://help/a2a-guide`, NATS `lounge.task.acked` pull-inbox. `notifications/message` hâlâ ertelenmiş (PR-5+).

### `notifications/cancelled`

| reason | Davranış |
|---|---|
| `context canceled` (kullanıcı Stop) | Görev `CANCELLED`; NATS `lounge.control.stop` → dispatcher tüketir ve iptal eder |
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
