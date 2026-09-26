# Agent Lounge OS — Windows Live QA Report (S2)

| Field | Value |
|-------|-------|
| Date | YYYY-MM-DD |
| Build / commit | |
| Tester | |
| OS | Windows 10/11 |
| Display(s) | |

## Cross-platform checks (§10.2)

| ID | Check | Pass? | Notes |
|----|-------|-------|-------|
| SH-04 | Palette opens with **Ctrl+K**; UI shows `Ctrl+K` (not ⌘K) | ☐ | |
| DS-03 | Open in editor uses `start` / opener; Settings Editor override | ☐ | |
| AP-08 | Approval alert sound while pending (also when window minimized) | ☐ | |
| AP-09 | Settings sound prefs + Dinle; wav/mp3/ogg upload | ☐ | |
| AP-10 | **Windows** toast/notification via Tauri plugin; click focuses app + banner | ☐ | |
| AP-06/07 | Destructive patterns: `del /s`, `rd /s`, `Remove-Item -Recurse`, `format` require confirm | ☐ | |
| Paths | Index / open paths with `C:\…` and mixed separators work | ☐ | |

## Routes smoke (manual)

| Route | Loads | Layout OK | Notes |
|-------|-------|-----------|-------|
| /dashboard | ☐ | ☐ | |
| /vault | ☐ | ☐ | |
| /health | ☐ | ☐ | |
| /settings | ☐ | ☐ | |
| /onboarding | ☐ | ☐ | |

## Sign-off

- [ ] No macOS-only assumptions in UI copy or shortcuts
- [ ] Screenshots attached if UI PR
