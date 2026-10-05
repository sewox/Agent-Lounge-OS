# A2A MCP — PR-3b Tuning & Stability (tasarım notu)

Status: **implemented in PR-3b** (üzerine `main` @ MCP Bridge v2 / #83). Kaynak: Gemini msg57 + ölçümler 2026-10-04/05; kapsam güncellemesi (list/piggyback/notifications → **PR-4**).

## Bu PR'da

| Alan | Davranış |
|---|---|
| `ClientProfile` | measured/assumed eşik tablosu; override = hard−20 (unknown 170) |
| Cursor progress | progressToken → 10 sn heartbeat, Mod A ≤280 sn |
| In-flight iptal | stdio EOF / `DELETE /mcp` → yalnız senkron in-flight CANCELLED+stop; backgrounded korunur |
| TTL (ikisi 30 dk, ayarlanabilir) | sonuç orphan → EXPIRED; incomplete orphan → EXPIRED+stop |
| `long_running` / `must_deliver` | bayraklar; must_deliver kota 5/oturum + 10/ajan → **-32029**; 24s alınmazsa FAILED(abandoned) |
| `poll_after_secs` | 15 → ×1.5 → ≤60; `next_action` + tool açıklamaları |
| `task_token` | yaratılışta düz token bir kez; DB SHA-256; wait = aynı oturum VEYA token (sabit-zamanlı). Workspace paylaşımı **yok** |

## PR-4'e taşınan (burada yok)

`lounge_list_my_tasks`, `pending_results` piggyback, `notifications/message`.

## ClientProfile özeti

| name | hard | threshold | progress_extends | sends_cancel | source |
|---|---:|---:|:---:|:---:|---|
| antigravity-client | 180 | 150 | false | true | measured |
| cursor-vscode | 120 | 100 | true (≤280) | false | measured |
| claude-ai | 240 | 210 | false | false | measured |
| claude-code | — | 300 | false | false | assumed |
| unknown (Grok) | — | 45 | false | false | assumed |

## Kabul edilen riskler (PR-3'ten)

- Yield spoof (`Mcp-Session-Id` + `clientInfo.name` istemci seçimli) — PR-5 token/PID.
- `control.stop` yalnız DB status; worker hard-kill yok.
- Eşik yarışı: `try_mark_a2a_wait_timeout` atomik (tek kazanan).
- must_deliver idempotency: ilk kabul kazanır.
- Cancel, progress heartbeat'ten önce işlenir.
