# Approval OS notification click (AP-10)

Backend emits `approval_pending` and shows a native OS notification via
`tauri-plugin-notification` on macOS, Windows, and Linux.

## CI status (retired from open manual debt)

| Surface | Status | Notes |
|---------|--------|-------|
| Playwright AP-10 (FE contract) | **Pass / CI gate** | Mock `focus_app_for_approval` → `approval_banner_focus` + banner focus |
| Rust unit tests (slot / clear / activation gate) | **Pass / CI gate** | `approval_notify` tests |
| Live OS toast → activate → banner (S2) | **Retired from CI gate** | Optional platform smoke only — see below |

Reason for retirement: `@tauri-apps/plugin-notification` **`onAction` is mobile-only**.
Desktop does **not** deliver toast-click callbacks to the webview. The real desktop
path is dock/taskbar / window focus (`RunEvent::Reopen`, `WindowEvent::Focused`),
which cannot be driven from the Playwright browser harness. Keeping an open
“NOT YET RUN” blocker would be a permanent false debt.

Optional S2 smoke remains in `scripts/qa/{mac,windows,linux}/QA_Report.md` for
humans with a packaged build — it is **not** a merge blocker and is **not**
counted as skipped/expected-fail in the e2e suite.

## What actually works (honest)

`@tauri-apps/plugin-notification` **`onAction` is mobile-only**. Upstream guest-js
documents it as: only emitted on mobile, for notifications that reference an action
type registered with `registerActionTypes`. The desktop plugin surface is
`notify` / permission helpers — it does **not** deliver click/action callbacks to
the webview on Windows, macOS, or Linux. We do **not** claim otherwise.

### Desktop path (Win / macOS / Linux) — Rust

1. Rust `emit_approval_pending` shows an OS toast (best-effort) and **requests
   dock/taskbar attention** (`UserAttentionType::Critical`).
2. The pending `task_id` is stored in a process slot.
3. When the app is activated while that slot is set:
   - **macOS**: `RunEvent::Reopen` → `on_app_activated_for_pending_approval`.
   - **All desktop**: `WindowEvent::Focused(true)` on the main window → same
     handler (toast-driven activation, Alt-Tab, taskbar click).
4. That handler calls `focus_app_for_approval`: unminimize / show / set_focus,
   clear attention, emit `approval_banner_focus`.
5. On **every** resolution path the pending slot is cleared with **id match only**
   so a later focus cannot re-raise with a stale id:
   - Routing: Approve / ApproveLocal / Deny via `resolve_vote` / `await_approval`,
     plus timeout / channel_closed / failed.
   - Destructive: `confirm_destructive`, `reject_destructive`, and expiry purge
     inside `take_confirmed_allowance` (slot id = confirm id).

Rust unit tests cover: matching clear, mismatched id does not clear, activation
gate is inert after resolve (routing + destructive confirm/reject).

### Frontend bridge + Playwright (front-end contract — CI gate)

`ApprovalNotificationBridge` listens for `approval_banner_focus` and focuses the
tabindex banner. Playwright AP-10 uses the **e2e Tauri mock** of
`focus_app_for_approval`, which emits the event and focuses the banner in the
harness. That proves the **front-end contract** (invoke → banner focus / window
focus fallback), **not** the live Rust raise path. Do not treat AP-10 e2e as a
desktop OS toast-click pass.

### Mobile (out of primary scope)

If `onAction` fires (action-typed notification), the bridge invokes
`focus_app_for_approval`. Registration failures are **logged**, never swallowed.

## Optional S2 platform smoke (not CI)

| Platform | Click / activation | Checklist |
|----------|--------------------|-----------|
| **macOS** | Notification Center often activates the app; plugin `onAction` does **not** fire. | Pending approval toast → window forward + banner focus. If toast cannot activate: dock/Alt-Tab with still-pending approval → banner focus. After Approve/Deny, further focus must **not** re-raise. |
| **Windows** | Toast may activate a packaged app; `onAction` does **not** fire. Dev `cargo tauri dev` toasts are unreliable. | Same as macOS. |
| **Linux** | libnotify / D-Bus varies; default-action callbacks are **not** wired through the Tauri notification plugin. | Same as macOS. |

Capability: `notification:default` on the `main` window
(`src-tauri/capabilities/default.json`).

**No background volume/interval escalation** (K1): the alert audio engine keeps
the user-configured volume and interval while the window is hidden; the OS
notification is the secondary signal.

Manual S2 checklists: `scripts/qa/{mac,windows,linux}/QA_Report.md`.
