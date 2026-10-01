# Approval OS notification click (AP-10)

Backend emits `approval_pending` and shows a native OS notification via
`tauri-plugin-notification` on macOS, Windows, and Linux.

## What actually works (honest)

`@tauri-apps/plugin-notification` **`onAction` is mobile-only**. Upstream guest-js
documents it as: only emitted on mobile, for notifications that reference an action
type registered with `registerActionTypes`. The desktop plugin surface is
`notify` / permission helpers — it does **not** deliver click/action callbacks to
the webview on Windows, macOS, or Linux. We do **not** claim otherwise.

### Desktop path (Win / macOS / Linux)

1. Rust `emit_approval_pending` shows an OS toast (best-effort) and **requests
   dock/taskbar attention** (`UserAttentionType::Critical`).
2. The pending `task_id` is stored in a process slot.
3. When the app is activated while that slot is set:
   - **macOS**: `RunEvent::Reopen` (dock click / notification activation that
     reopens the app) → `on_app_activated_for_pending_approval`.
   - **All desktop**: `WindowEvent::Focused(true)` on the main window → same
     handler (covers toast-driven activation when the OS focuses the app, Alt-Tab,
     taskbar click).
4. That handler calls `focus_app_for_approval`: unminimize / show / set_focus,
   clear attention, emit `approval_banner_focus`.
5. Frontend `ApprovalNotificationBridge` listens for `approval_banner_focus` and
   scrolls/focuses the approval banner. A window/`visibilitychange` focus
   fallback also invokes `focus_app_for_approval` while a pending approval exists
   (covers harness + environments where Rust focus events are not observable from
   JS alone).

### Mobile (out of primary scope for PR-2b targets)

If `onAction` fires (action-typed notification), the bridge invokes
`focus_app_for_approval`. Registration failures are **logged**, never swallowed.

### Where click cannot be delivered

If a desktop environment shows a non-interactive bubble and does not activate the
app on click, the user must bring Lounge forward manually (dock / Alt-Tab /
taskbar). On that next focus, the banner is focused and attention is cleared.
**S2 live** still verifies toast → activate → banner on each OS.

Capability: `notification:default` on the `main` window
(`src-tauri/capabilities/default.json`).

## Platform matrix

| Platform | Click / action delivery | What we do |
|----------|-------------------------|------------|
| **macOS** | Notification Center often activates the app; plugin `onAction` does **not** fire on desktop. | Dock attention + `RunEvent::Reopen` / window focus → `focus_app_for_approval`. |
| **Windows** | Toast may activate a packaged app; `onAction` does **not** fire on desktop. Dev `cargo tauri dev` toasts are unreliable for activation. | Taskbar attention + window focus → `focus_app_for_approval`. |
| **Linux** | libnotify / D-Bus varies (GNOME/KDE). Default-action callbacks are **not** wired through the Tauri notification plugin. | Attention + window focus → `focus_app_for_approval`. |

**S2 live checklist:** click a real pending-approval toast on each OS; confirm the
Lounge window comes forward and the approval banner is focused. If the toast
cannot activate the app, Alt-Tab / click the taskbar or dock with a still-pending
approval and verify the banner focuses.

**No background volume/interval escalation** (K1): the alert audio engine keeps
the user-configured volume and interval while the window is hidden; the OS
notification is the secondary signal.

Manual S2 checklists: `scripts/qa/{mac,windows,linux}/QA_Report.md`.
