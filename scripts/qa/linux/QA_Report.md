# Agent Lounge OS — Linux Live QA Report (S2-Linux)

| Field | Value |
|-------|-------|
| Date | YYYY-MM-DD |
| Build / commit | |
| Artifact run | `agent-lounge-linux` (Actions run id) |
| Tester | |
| Distro / DE | Ubuntu-like · X11 |
| Installer | ☐ .deb · ☐ AppImage |

Step-by-step: [CHECKLIST.md](./CHECKLIST.md) · Runtime: [RUNTIME_DEPS.md](./RUNTIME_DEPS.md) · Intro: [README.md](./README.md)

## Viewports

| ID | Size | How |
|----|------|-----|
| L-D0 | 1280×800 | Default / resize |
| L-D3 | ≈1080×1920 or 800×1280 | Same window, portrait resize (no second monitor) |

### L-D0 — 1280×800

| Route | Loads | Layout OK | Notes |
|-------|-------|-----------|-------|
| /dashboard | ☐ | ☐ | |
| /stream | ☐ | ☐ | |
| /vault | ☐ | ☐ | |
| /health | ☐ | ☐ | |
| /fleet | ☐ | ☐ | |
| /telemetry | ☐ | ☐ | |
| /quotas | ☐ | ☐ | |
| /settings | ☐ | ☐ | |
| /onboarding | ☐ | ☐ | |

### L-D3 — portrait resize

| Route | Loads | Layout OK | Notes |
|-------|-------|-----------|-------|
| /dashboard | ☐ | ☐ | |
| /vault | ☐ | ☐ | |
| /health | ☐ | ☐ | |
| /fleet | ☐ | ☐ | |
| /quotas | ☐ | ☐ | |
| /settings | ☐ | ☐ | |

## Cross-platform checks (§10.2)

| ID | Check | Pass? | Notes |
|----|-------|-------|-------|
| SH-04 | Palette opens with **Ctrl+K**; UI shows `Ctrl+K` (not ⌘K) | ☐ | |
| DS-03 | Open in editor uses `xdg-open` / opener; Settings Editor override | ☐ | |
| AP-08 | Approval alert sound while pending (also when window unfocused) | ☐ | |
| AP-09 | Settings sound prefs + Dinle; wav/mp3/ogg upload | ☐ | |
| AP-10 | **Linux** desktop notification via Tauri plugin; click focuses app + banner | ☐ | |
| AP-06/07 | Destructive POSIX patterns (`rm -rf`, …) require confirm | ☐ | |
| Paths | Absolute `/home/…` paths and symlinks behave | ☐ | |

## Degraded services

| Check | Pass? | Notes |
|-------|-------|-------|
| App opens with no `nats-server` / no LMR / no host Ollama | ☐ | Expected: degraded banners, no crash |
| (Optional) With NATS installed, stream shows events | ☐ | |

## Sign-off

- [ ] Notification portal / permission granted if prompted
- [ ] CHECKLIST.md A–C completed
- [ ] Screenshots attached if UI PR
