# MCP Probe — istemci davranış ölçümü

Bağımsız harness: `tools/mcp-probe/`. Ürün koduna (`src-tauri`, `bridge`, UI) dokunmaz.

Amaç: Antigravity, Grok Bot, Cursor, Claude Desktop ve diğer GUI/CLI MCP istemcilerinin **tool timeout**, **progress notification**, **cancel** ve **bağlantı kopma/yeniden bağlanma** davranışını ölçmek. §16 (`delegate_and_wait` / asenkron aynı oturuma dönüş) için istemci matrisi bu loglardan çıkarılır.

Gerçek ölçümü ilgili uygulamada siz (veya QA) yaparsınız; bu belge yalnızca aracı ve harness’i anlatır.

## Kurulum

Node ≥ 20. Ek npm bağımlılığı yok.

```bash
cd tools/mcp-probe
npm test                 # birim + entegrasyon (sahte istemci)
```

## Sunucuyu başlatma

### stdio (çoğu masaüstü istemci)

```bash
node tools/mcp-probe/bin/mcp-probe.mjs \
  --transport stdio \
  --log-dir /ABS/PATH/TO/Agent-Lounge-OS/tools/mcp-probe/logs \
  --client-label cursor
```

Ortam değişkenleri:

| Değişken | Anlam |
|----------|--------|
| `MCP_PROBE_LOG_DIR` | JSONL dizini (mutlak yol önerilir) |
| `MCP_PROBE_CLIENT` | Log dosya etiketi (`cursor`, `claude-desktop`, …) |

### Streamable HTTP / SSE

```bash
node tools/mcp-probe/bin/mcp-probe.mjs --transport http --host 127.0.0.1 --port 19891
```

- `POST /mcp` — JSON-RPC (uzun çağrılarda SSE yanıt)
- `GET /mcp` — SSE (sunucu→istemci; `Mcp-Session-Id` gerekir)
- `DELETE /mcp` — oturum kapat
- `GET /health` — sağlık

## Araçlar

| Tool | Argümanlar | Davranış |
|------|------------|----------|
| `ping` | `note?` | Anında yanıt |
| `slow_echo` | `delay_ms` (zorunlu), `message?`, `progress?` | `delay_ms` bekler; `progress=true` ve istemci `_meta.progressToken` verirse `notifications/progress` heartbeat gönderir |

Ölçüm senaryosu (istemci sohbetinde):

1. `ping` — bağlantı var mı?
2. `slow_echo` delay=5000, progress=false
3. `slow_echo` delay=15000…120000, progress=true
4. Uzun çağrı sırasında istemciyi kapat / ağı kes / iptal et → logda `cancelled`, `connection_close`, `timeout_observed`

## JSONL log alanları

Dosya: `logs/<client-label>-<session-id>.jsonl`

| event | Anlam |
|-------|--------|
| `session_start` / `session_end` | Oturum başlangıç/bitiş |
| `initialize` | `clientInfo`, `capabilities`, `protocolVersion` |
| `tools_call_start` / `tools_call_end` | Çağrı zamanları, `duration_ms`, `delay_ms`, status |
| `progress_token_present` / `progress_token_missing` | Progress token desteği |
| `progress_sent` | Sunucunun gönderdiği progress |
| `cancelled` | İstemci `notifications/cancelled` |
| `connection_close` | Bağlantı kapandı (stdio EOF, HTTP abort, DELETE, …) |
| `connection_reconnect` | Aynı / sunulan session id ile yeniden geliş |
| `timeout_observed` | Uzun çağrı sırasında kopma; `elapsed_s` |

## run-matrix + rapor

Varsayılan gecikmeler (saniye): **5, 15, 25, 30, 45, 60, 120** (her biri progress açık/kapalı).

```bash
cd tools/mcp-probe
npm run run-matrix
# kısa smoke:
MCP_PROBE_DELAYS=0.05,0.1 npm run run-matrix
# istemci timeout simülasyonu:
node scripts/run-matrix.mjs --delays 5 --client-timeout-ms 1000

node scripts/report.mjs --log-dir ./logs --matrix ./logs/matrix/matrix-results.json --out ./logs/report.md
```

Çıktı: `logs/matrix/matrix-results.json` + `matrix-report.md` (markdown tablo).

### Antigravity cliff (yalnız manuel — CI’da yok)

120–180 sn aralığını **sunucu tarafı** (mcp-probe sahte istemci) ile doğrulamak için:

```bash
npm run run-antigravity-cliff
# veya: node scripts/run-antigravity-cliff.mjs --out-dir ./logs/antigravity-cliff
```

Gecikmeler: **150, 170, 180, 190** sn (progress açık/kapalı). Duvar süresi ~20+ dk.

**Beklenti (dürüst):** Bu script gerçek Antigravity IDE’nin ~180 sn `deadline exceeded` kesintisini **üretemez**; probe kendi `clientTimeoutMs` ile abort eder. Doğrulanan: sunucu/bridge’in uzun görev + iptal/backgrounded davranışı. Gerçek Antigravity cliff ölçümü manuel IDE oturumunda yapılır.

## İstemci MCP config örnekleri

Yolları kendi makinenize göre mutlak yapın. Log etiketini `--client-label` / `MCP_PROBE_CLIENT` ile ayırın.

### Cursor — `~/.cursor/mcp.json` veya proje `.cursor/mcp.json`

```json
{
  "mcpServers": {
    "mcp-probe": {
      "command": "node",
      "args": [
        "/ABS/PATH/TO/Agent-Lounge-OS/tools/mcp-probe/bin/mcp-probe.mjs",
        "--transport",
        "stdio",
        "--log-dir",
        "/ABS/PATH/TO/Agent-Lounge-OS/tools/mcp-probe/logs",
        "--client-label",
        "cursor"
      ]
    }
  }
}
```

HTTP (Cursor URL destekliyorsa):

```json
{
  "mcpServers": {
    "mcp-probe-http": {
      "url": "http://127.0.0.1:19891/mcp"
    }
  }
}
```

### Claude Desktop — `claude_desktop_config.json`

- macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`
- Windows: `%APPDATA%\Claude\claude_desktop_config.json`
- Linux: `~/.config/Claude/claude_desktop_config.json`

```json
{
  "mcpServers": {
    "mcp-probe": {
      "command": "node",
      "args": [
        "/ABS/PATH/TO/Agent-Lounge-OS/tools/mcp-probe/bin/mcp-probe.mjs",
        "--transport",
        "stdio",
        "--log-dir",
        "/ABS/PATH/TO/Agent-Lounge-OS/tools/mcp-probe/logs",
        "--client-label",
        "claude-desktop"
      ]
    }
  }
}
```

Uygulamayı yeniden başlatın.

### Antigravity

> **Doğrulanmadı:** aşağıdaki config yolları istemci sürümüne göre değişebilir; ölçüm öncesi makinedeki gerçek dosyayı kontrol edin.

Global (tercih, doğrulanmadı): `~/.gemini/config/mcp_config.json`  
Legacy (doğrulanmadı): `~/.gemini/antigravity/mcp_config.json`  
Proje (doğrulanmadı): `.agents/mcp_config.json`

```json
{
  "mcpServers": {
    "mcp-probe": {
      "command": "node",
      "args": [
        "/ABS/PATH/TO/Agent-Lounge-OS/tools/mcp-probe/bin/mcp-probe.mjs",
        "--transport",
        "stdio",
        "--log-dir",
        "/ABS/PATH/TO/Agent-Lounge-OS/tools/mcp-probe/logs",
        "--client-label",
        "antigravity"
      ]
    }
  }
}
```

### Grok Bot

Resmi MCP config yolu / şeması belgelenmemiş (tasarım §15.1). Keşif sonrası aynı stdio komutunu Grok’un beklediği anahtara (`mcpServers` veya eşdeğeri) yapıştırın; `--client-label grok-bot` kullanın. Bulunan yolu ölçüm notuna yazın.

HTTP dinliyorsa probe’u `--transport http` ile açıp Grok’un URL alanına `http://127.0.0.1:19891/mcp` verin.

### Diğer GUI/CLI

Aynı `command` + `args` kalıbı Claude Code (`.mcp.json`), VS Code (`.vscode/mcp.json` → çoğu sürümde `servers`), Windsurf, Zed (`context_servers`) için uyarlanır. Her istemci için ayrı `--client-label` kullanın.

## Cancel davranışı

İstemci `notifications/cancelled` gönderdiğinde probe iptali **yalnızca loglar** ve in-flight `slow_echo`’yu durdurur; iptal edilen istek için **JSON-RPC `-32800` yanıtı göndermez** (MCP spec: server SHOULD NOT respond to a cancelled request).

## CI

`ci.yml` içinde `MCP probe` işi (ubuntu / windows / macos matrisi) zorunlu aggregate `ci` job’ına `needs` ile bağlıdır (bilinçli: probe regresyonu ana CI’yı kırmızıya çeker).

Adımlar:

1. `cd tools/mcp-probe && npm test` — birim + sahte-istemci entegrasyon testleri
2. `MCP_PROBE_DELAYS=0.05,0.1 npm run run-matrix` — kısa matrix smoke (JSON + markdown rapor)

Testler sahte istemci kullanır; GUI ölçümü CI dışındadır. `paths-ignore` ve branch korumasına dokunulmamıştır; test gevşetme / `continue-on-error` / skip etiketi yoktur. Zamanlayıcı yarışlarında sabit kısa `sleep` yerine olay beklenir (`waitFor`, cömert timeout).

Geçersiz `MCP_PROBE_DELAYS` (boş, `abc`, negatif) non-zero exit verir; sıfır satırla başarı sayılmaz.

## Güvenlik notu

Probe yalnızca yerel ölçüm içindir. HTTP yüzeyi loopback `Origin`/`Host` ister, gövde ≤ 1 MiB, oturum sayısı sınırlıdır; `Mcp-Session-Id` `^[A-Za-z0-9._-]{1,64}$` ile doğrulanır (path traversal engeli). `127.0.0.1` dışında bind etmeyin. Loglar istemci adını ve yeteneklerini içerir — paylaşımdan önce gözden geçirin.
