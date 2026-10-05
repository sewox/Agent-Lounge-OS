# A2A MCP — PR-3b Tuning & Stability (tasarım notu)

Status: **implemented in PR-3b** (üzerine `main` @ MCP Bridge v2 / #83). Kaynak: Gemini msg57 + ölçümler 2026-10-04/05; kullanıcı eklemesi: long_running/must_deliver semantiği, list_my_tasks, pending_results piggyback.

## Bu PR'da

| Alan | Davranış |
|---|---|
| `ClientProfile` | measured/assumed eşik tablosu; override = hard−20 (unknown 170) |
| Cursor progress | progressToken → 10 sn heartbeat, Mod A ≤280 sn |
| In-flight iptal | stdio EOF / `DELETE /mcp` → `SessionDisconnect`: varsayılan CANCELLED+stop; **must_deliver** → arka plan (iptal yok). Backgrounded map'te yok → korunur |
| TTL (ikisi 30 dk, ayarlanabilir) | sonuç orphan → EXPIRED; incomplete orphan → EXPIRED+stop; **must_deliver atlanır** |
| `long_running` | `true` → eşiği beklemeden hemen `backgrounded` + `task_id` (+ `task_token`) |
| `must_deliver` | bağlantı kopmasında iptal yok; orphan EXPIRED yok; yalnız `context canceled` / UserStop iptal eder. Kota 5/oturum + 10/ajan → **-32029**; 24s alınmazsa FAILED(abandoned) |
| `poll_after_secs` / `next_action` | profil eşiğine göre (≈threshold/10, 5…15 → ×1.5 → ≤60); `next_action` = `lounge_wait_task(task_id) ile tekrar kontrol et` |
| `task_token` | yaratılışta düz token bir kez; DB SHA-256; wait = aynı oturum VEYA token (sabit-zamanlı). Workspace paylaşımı **yok** |
| `lounge_list_my_tasks` | yalnız çağıran oturumun açık / unclaimed görevleri |
| `pending_results` | her `lounge_*` yanıtında piggyback `[task_id…]` (hazır, henüz alınmamış) |
| `notifications/message` | istemci logging/notifications bildirdiyse best-effort log; **güvenilmez** — piggyback asıl kanal |

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
