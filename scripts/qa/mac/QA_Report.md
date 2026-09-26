# Agent Lounge OS — Mac Live QA Report (S2)

| Field | Value |
|-------|-------|
| Date | YYYY-MM-DD |
| Build / commit | |
| Tester | |
| App window title | Agent Lounge OS |

## Displays

| ID | Hardware | Looks-like (CSS px) | Native | Notes |
|----|----------|---------------------|--------|-------|
| D1 | 14" MacBook Retina | ~1512×982 | 3024×1964 @2x | Built-in |
| D2 | 32" Samsung | 1920×1080 | | External |
| D3 | 27" F27G3xTF portrait | 1080×1920 | | Portrait |

Screenshots folder: `./` (filled by `capture-displays.sh`)

## Per-display L1–L6 checklist

Mark Pass / Fail. Failures are [B] layout blockers per plan §6.

### D1 — 14" Retina

| Route | L1 ≥85% width | L2 ≥85% height or scroll | L3 empty ≤15% | L4 no centered narrow | L5 no h-overflow | Notes / shot |
|-------|---------------|---------------------------|---------------|-----------------------|------------------|--------------|
| /dashboard | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /stream | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /vault | ☐ | ☐ | ☐ | ☐ | ☐ | Semantic Map + Experiences half-fill? |
| /health | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /fleet | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /telemetry | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /quotas | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /settings | ☐ | ☐ | ☐ | ☐ | ☐ | UI scale 90/100/130 |
| /onboarding | ☐ | ☐ | ☐ | ☐ | ☐ | |

### D2 — 32" Samsung 1920×1080

| Route | L1 | L2 | L3 | L4 | L5 | Notes |
|-------|----|----|----|----|----|-------|
| /dashboard | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /stream | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /vault | ☐ | ☐ | ☐ | ☐ | ☐ | max-width traps? |
| /health | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /fleet | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /telemetry | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /quotas | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /settings | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /onboarding | ☐ | ☐ | ☐ | ☐ | ☐ | |

### D3 — 27" portrait 1080×1920 (also L6: panels ≥95% width, stacked)

| Route | L1 | L2 | L3 | L4 | L5 | L6 stack ≥95% | Notes |
|-------|----|----|----|----|----|---------------|-------|
| /dashboard | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /stream | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /vault | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /health | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /fleet | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /telemetry | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /quotas | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /settings | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ | |
| /onboarding | ☐ | ☐ | ☐ | ☐ | ☐ | ☐ | |

## Functional smoke (manual)

| ID | Check | Pass? | Notes |
|----|-------|-------|-------|
| SH-06 | Index Workspace folder picker → index | ☐ | |
| GR-01 | Graph UI Enable/Open | ☐ | |
| GR-03 | Main close kills graph child process | ☐ | |
| DS-03 | OPEN_IN_EDITOR (system default / Settings Editor) | ☐ | |
| ST-04 | Graph port recreate | ☐ | |
| EX-* | Experience CRUD (after PR-1/3) | ☐ | |
| AP-08 | Approval alert sound while pending (also background/hidden); stops after decision | ☐ | |
| AP-09 | Settings sound prefs (on/off, built-in, upload, volume, interval, Dinle) | ☐ | |
| AP-10 | macOS notification while approval pending; click focuses app + banner | ☐ | |

## Deep-link / route automation note

PR-0 does **not** change product navigation. Recommended (for a later PR):

1. QA-only: `LOUNGE_QA=1` enables `?qa-nav=/vault` handled in `app-shell` / root redirect.
2. Or custom URL scheme `agent-lounge://nav/vault` registered in Tauri.

Until then, `capture-displays.sh` uses Accessibility clicks on sidebar labels.

## Sign-off

- [ ] All [B] layout rows Pass on D1–D3
- [ ] Screenshots attached to PR
- [ ] No MOCK_* visible in empty workspace
