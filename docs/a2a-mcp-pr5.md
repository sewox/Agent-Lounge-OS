# A2A MCP — PR-5 Security & Reachability

Status: **implemented** (base post-#86 / PR-4). Kaynak: Gemini msg64 + CloudAgent brief.

## Bu PR'da

| Alan | Davranış |
|---|---|
| NATS credentials | Kernel oturum başına rastgele user/pass üretir; `nats-server --user/--pass`; creds `LOUNGE_NATS_CREDS_FILE` + env |
| `source_verified` | NATS ingress: auth aktif → `true`; aksi (legacy/bypass) → `false` → `SourceUnverified` / `PENDING_APPROVAL` |
| Auth default | Kodda **true**; bypass: `LOUNGE_AUTH_REQUIRED=false` |
| Workers | `bot_template.py` Kernel creds okur; auth zorunluyken creds yoksa net hata |
| MCP remote | Non-loopback Host/Origin yalnızca geçerli `X-Lounge-Token` **ve** allow-list host ile |
| UI | Fleet → Connect Grok Bot kartı; kopyalanabilir MCP JSON (URL + token) |

## OUT (bilinçli)

- cloudflared / ngrok binary gömme yok (Managed Origin Validation)
- Launcher / `agents.yaml` (PR-6)
- `notifications/message` piggyback birincil kalır

## Kabul

Uncredentialed NATS yayınları `source_verified=false` + onay. Auth açıkken yetkisiz NATS bağlantıları reddedilir. Lounge MCP dış Host/Origin'i yalnız token + allow-list ile kabul eder.

## Bilinen risk

Grok Bot bulut runtime SSL/header kısıtları — ilk gerçek tünel testinde doğrulanmalı (CI canlı tünel kanıtlamaz).
