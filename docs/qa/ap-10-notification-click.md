# Approval OS notification click (AP-10)

Backend (PR-1) emits `approval_pending` and shows a native OS notification via
`tauri-plugin-notification` on macOS, Windows, and Linux.

## Click → focus behaviour (PR-2b)

1. Rust `focus_app_for_approval` unminimizes/shows/focuses the main window and
   emits `approval_banner_focus`.
2. The frontend listens for `approval_banner_focus` and scrolls/focuses the
   approval banner (`[data-qa="approval-banner"]` / `[data-approval-chrome]`).
3. A hidden `[data-qa="approval-notification"]` marker documents the wiring for
   automated smoke checks.

## Platform caveats

| Platform | Click event delivery | Fallback |
|----------|----------------------|----------|
| macOS Notification Center | Often focuses the app; click payload is not always exposed to Tauri desktop | Window activate + `focus_app_for_approval` |
| Windows toast | Installed builds can activate the app; click extras vary by installer | Same |
| Linux libnotify / D-Bus | Some desktops expose a default action; others only raise the window | Same |

**No background volume/interval escalation** (K1): the alert audio engine keeps
the user-configured volume and interval while the window is hidden; the OS
notification is the secondary signal.

Manual S2 checklists: `scripts/qa/{mac,windows,linux}/QA_Report.md`.
