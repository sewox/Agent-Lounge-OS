# A2A MCP — PR-3b Tuning & Stability (tasarım notu)

Status: **implemented in PR-3b** (üzerine `main` @ MCP Bridge v2 / #83). Kaynak: Gemini msg57 + ölçümler 2026-10-04/05; inceleme düzeltmeleri B1–B4.

## Bu PR'da

| Alan | Davranış |
|---|---|
| `ClientProfile` | measured/assumed eşik tablosu; override = hard−20 (unknown 170) |
| Cursor progress | progressToken → 10 sn heartbeat, Mod A ≤280 sn |
| In-flight iptal | stdio EOF / `DELETE /mcp` → `SessionDisconnect`: Mod A senkron → CANCELLED+stop (must_deliver → arka plan); **wait_task long-poll yalnız biter, görev iptal edilmez**; backgrounded korunur |
| TTL (ikisi 30 dk, ayarlanabilir) | sonuç orphan → EXPIRED; incomplete orphan → EXPIRED+stop; **must_deliver atlanır**; **teslim (last_wait ≥ result_ready) atlanır** |
| Sessizlik | WTR (backgrounded) **muaf**; EXECUTING/DISPATCHED/QUEUED/RECOVERY_PENDING → NEEDS_HUMAN |
| `long_running` | `true` → eşik beklemeden hemen `backgrounded` + `task_id` |
| `must_deliver` | kopmada iptal yok; orphan EXPIRED yok; 24 sa alınmazsa FAILED(abandoned); kota 5/10 → **-32029** |
| `poll_after_secs` | 15 → ×1.5 → ≤60; `next_action` + tool açıklamaları |
| `task_token` | yaratılışta düz token bir kez; DB SHA-256; wait = aynı oturum VEYA token |
| Proxy timeout | varsayılan **320 sn** (≥ claude-code 300); hata → JSON-RPC error istemciye |

## PR-4'e taşınan

`lounge_list_my_tasks`, `pending_results` piggyback, Resource `lounge://help/a2a-guide` → **PR-4'te**. `notifications/message` hâlâ ertelenmiş.

## İzleme (bu turda zorunlu değil)

kota TOCTOU; `a2a_flags` hata→iptal; standalone progress yalnız stderr; claude_ai/claude_code host id dashboard uyumsuzluğu; DELETE sonrası POST oturumu geri yazma; `task_token_hash` NATS payload; TTL settings sabitleri yalnız env.

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
