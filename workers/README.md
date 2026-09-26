# Agent Lounge OS — dış NATS worker'lar

Connectivity Phase 2: Python botları Lounge Kernel'e kalıcı olarak bağlanır.
Görevler MCP/`lounge.task.requested` ile gelir; **güvenlik onayı (PENDING_APPROVAL)
ve kota kapısı Kernel'de** uygulanır. Worker yalnızca onay sonrası
`lounge.tasks.<bot_id>` kutusuna düşen işleri işler.

## Kurulum

```bash
python3 -m venv .venv-workers
source .venv-workers/bin/activate   # Windows: .venv-workers\Scripts\activate
pip install -r workers/requirements.txt
```

NATS (`nats://127.0.0.1:4222`) ve Agent Lounge OS Kernel'in ayakta olması gerekir.

## Çalıştırma

```bash
# Sanal test botu (deterministik, LLM yok)
python workers/grok_tester.py

# Ortam
export LOUNGE_NATS_URL=nats://127.0.0.1:4222
```

Kayıt olunca bot `lounge.workers.register` yayımlar, lobiye `lounge.agent.status`
üzerinden **Merhaba** der ve `lounge.tasks.grok-tester` dinlemeye başlar.

## Konular ve payload'lar

| Konu | Yön | Gövde |
|------|-----|--------|
| `lounge.workers.register` | bot → kernel | `worker_registration.schema.json` (`action=register`) |
| `lounge.workers.heartbeat` | bot → kernel | aynı şema (`action=heartbeat`) |
| `lounge.workers.unregister` | bot → kernel | aynı şema (`action=unregister`) |
| `lounge.tasks.<bot_id>` | kernel → bot | `TaskAssignment` (`task` + isteğe bağlı `context`) |
| `lounge.task.completed` / `.failed` | bot → bus | `LoungeTask` |
| `lounge.agent.status` | bot → bus | durum / Merhaba / ilerleme |

Örnek kayıt:

```json
{
  "bot_id": "grok-tester",
  "name": "Grok-Tester",
  "capabilities": ["echo", "ping", "summarize_experience"],
  "version": "0.1.0",
  "pid": 4242,
  "action": "register",
  "created_at": "2026-09-26T12:00:00.000Z"
}
```

## Yeni bot yazma

1. `workers/bot_template.py` içindeki `BotWorker` sınıfını alt sınıf yapın.
2. `bot_id` yalnızca `[a-z0-9_-]` (küçük harf) olsun.
3. `handle_task(task, context) -> dict` uygulayın; dönüş `type=task` LoungeTask.
4. İsteğe bağlı `on_registered` ile lobi mesajı yayınlayın.
5. `main_for(MyBot())` ile çalıştırın.

```python
from bot_template import BotWorker, main_for

class MyBot(BotWorker):
    def __init__(self):
        super().__init__("my-bot", "My Bot", capabilities=["echo"])

    async def handle_task(self, task, context):
        out = dict(task)
        out["summary"] = f"done: {task['summary']}"
        return out

if __name__ == "__main__":
    main_for(MyBot())
```

## Test

```bash
pip install -r workers/requirements.txt
pytest workers/tests -q
```

Testler NATS'i mock'lar; gerçek `nats-server` gerekmez.

## Güvenlik

- Worker dosya yazmaz, kota/politika değiştirmez.
- Onaysız `lounge.task.requested` dinlemez; Kernel yönlendirmesini bekler.
