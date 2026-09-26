# S2-Linux live checklist

Use with [QA_Report.md](./QA_Report.md). Mark Pass/Fail; note screenshot names.

## Setup

- [ ] X11 session (not pure Wayland-only)
- [ ] Runtime deps installed ([RUNTIME_DEPS.md](./RUNTIME_DEPS.md))
- [ ] App launched from `.deb` or AppImage (`agent-lounge-linux` artifact)
- [ ] Record commit / artifact run id in QA_Report

## A — Landscape 1280×800

Resize or confirm window ≈ **1280×800**.

| Route | Opens | L1 width fill | L2 height / scroll | L5 no h-overflow | Notes / shot |
|-------|-------|---------------|--------------------|------------------|--------------|
| /dashboard | ☐ | ☐ | ☐ | ☐ | |
| /stream | ☐ | ☐ | ☐ | ☐ | |
| /vault | ☐ | ☐ | ☐ | ☐ | |
| /health | ☐ | ☐ | ☐ | ☐ | |
| /fleet | ☐ | ☐ | ☐ | ☐ | |
| /telemetry | ☐ | ☐ | ☐ | ☐ | |
| /quotas | ☐ | ☐ | ☐ | ☐ | |
| /settings | ☐ | ☐ | ☐ | ☐ | |
| /onboarding | ☐ | ☐ | ☐ | ☐ | |

## B — Simulated portrait (window resize)

Keep one display; resize the app window to **≈1080×1920** or **800×1280**.

| Route | Stacks / usable | L5 no h-overflow | L6 panels ~full width | Notes / shot |
|-------|-----------------|------------------|-----------------------|--------------|
| /dashboard | ☐ | ☐ | ☐ | |
| /vault | ☐ | ☐ | ☐ | |
| /health | ☐ | ☐ | ☐ | |
| /fleet | ☐ | ☐ | ☐ | |
| /quotas | ☐ | ☐ | ☐ | |
| /settings | ☐ | ☐ | ☐ | |

## C — Cross-platform (§10.2)

| ID | Check | Pass? |
|----|-------|-------|
| SH-04 | **Ctrl+K** opens palette; UI shows `Ctrl+K` | ☐ |
| DS-03 | Open in editor → `xdg-open` / opener | ☐ |
| AP-08/09 | Approval sound prefs (if built) | ☐ |
| AP-10 | Desktop notification + click focus | ☐ |
| Paths | `/home/…` paths in index/open | ☐ |

## D — Degraded-mode smoke (optional)

Without `nats-server` / LMR on the machine:

- [ ] App window still opens
- [ ] Health / service banner shows degraded (not hard crash)
- [ ] With NATS installed later, Event Stream receives traffic

## Sign-off

- [ ] QA_Report.md completed
- [ ] Screenshots attached for Fail rows
