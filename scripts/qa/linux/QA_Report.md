# Agent Lounge OS — Linux Live QA Report (S2)

| Field | Value |
|-------|-------|
| Date | YYYY-MM-DD |
| Build / commit | |
| Tester | |
| Distro / DE | |
| Display(s) | |

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

## Routes smoke (manual)

| Route | Loads | Layout OK | Notes |
|-------|-------|-----------|-------|
| /dashboard | ☐ | ☐ | |
| /vault | ☐ | ☐ | |
| /health | ☐ | ☐ | |
| /settings | ☐ | ☐ | |
| /onboarding | ☐ | ☐ | |

## Sign-off

- [ ] Notification portal / permission granted if prompted
- [ ] Screenshots attached if UI PR
