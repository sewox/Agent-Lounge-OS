# Approval OS notification click (AP-10)

Backend (PR-1) emits `approval_pending` and shows a native OS notification via
`tauri-plugin-notification` on macOS, Windows, and Linux.

## Click → focus behaviour (PR-2b)

1. Rust `emit_approval_pending` shows an OS toast with `extra.task_id`.
2. Frontend `ApprovalNotificationBridge` registers
   `@tauri-apps/plugin-notification` **`onAction`**. When the OS delivers a click /
   default action, the bridge invokes Tauri command **`focus_app_for_approval`**
   with that `task_id`.
3. Rust `focus_app_for_approval` unminimizes/shows/focuses the main window and
   emits `approval_banner_focus`.
4. The same bridge listens for `approval_banner_focus` and scrolls/focuses the
   approval banner (`[data-qa="approval-banner"]` / `[data-approval-chrome]`).
5. A hidden `[data-qa="approval-notification"]` marker documents the wiring for
   automated smoke checks.

Capability: `notification:default` on the `main` window
(`src-tauri/capabilities/default.json`).

## Platform caveats (honest)

| Platform | Click / action delivery | What we do |
|----------|-------------------------|------------|
| **macOS** Notification Center | Often activates the app when the user clicks the banner. Payload extras / `onAction` are **not reliably delivered** to Tauri desktop for every notification style. | Prefer `onAction` when fired; **fallback**: on next `window` `focus` / `visibilitychange` while an approval is pending, focus the banner. |
| **Windows** toast | Installed (packaged) builds can activate the app; click extras vary by installer / Start Menu registration. Dev `cargo tauri dev` toasts may not deliver actions. | Same: `onAction` when available + window-focus fallback. |
| **Linux** libnotify / D-Bus | Depends on the desktop (GNOME/KDE/etc.). Some expose a default action; others only raise the window or show a non-interactive bubble. | Same: `onAction` when available + window-focus fallback. |

**S2 live checklist:** click a real pending-approval toast on each OS; confirm the
Lounge window comes forward and the approval banner is focused. If the toast
cannot deliver a click callback on that desktop, confirm the fallback: switch
back to the Lounge window (or Alt-Tab) with a still-pending approval and verify
the banner focuses.

**No background volume/interval escalation** (K1): the alert audio engine keeps
the user-configured volume and interval while the window is hidden; the OS
notification is the secondary signal.

Manual S2 checklists: `scripts/qa/{mac,windows,linux}/QA_Report.md`.
