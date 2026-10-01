//! OS + frontend notifications when approvals are pending or resolved.

use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, UserAttentionType};
use tauri_plugin_notification::NotificationExt;

use crate::models::ApprovalRequest;

pub const APPROVAL_PENDING_EVENT: &str = "approval_pending";
pub const APPROVAL_RESOLVED_EVENT: &str = "approval_resolved";
/// Frontend listens for this when a notification click (or focus command) should open the banner.
pub const APPROVAL_BANNER_FOCUS_EVENT: &str = "approval_banner_focus";

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalPendingPayload {
    pub task_id: String,
    pub summary: String,
    pub from_agent: String,
    pub to_agent: String,
    pub kind: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm_id: Option<String>,
}

impl From<&ApprovalRequest> for ApprovalPendingPayload {
    fn from(request: &ApprovalRequest) -> Self {
        Self {
            task_id: request.task_id.clone(),
            summary: request.summary.clone(),
            from_agent: request.from_agent.clone(),
            to_agent: request.to_agent.clone(),
            kind: format!("{:?}", request.kind),
            reason: request.reason.clone(),
            command: None,
            pattern: None,
            source: None,
            confirm_id: None,
        }
    }
}

impl ApprovalPendingPayload {
    pub fn from_destructive(event: &crate::kernel::DestructivePendingEvent) -> Self {
        Self {
            task_id: event.id.clone(),
            summary: format!("Destructive: {}", event.command),
            from_agent: event.source.clone(),
            to_agent: "user".into(),
            kind: "destructive".into(),
            reason: event.pattern.clone(),
            command: Some(event.command.clone()),
            pattern: Some(event.pattern.clone()),
            source: Some(event.source.clone()),
            confirm_id: Some(event.id.clone()),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalResolvedPayload {
    pub task_id: String,
    pub reason: String,
}

fn pending_slot() -> &'static Mutex<Option<String>> {
    static SLOT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// Remember the latest pending approval so activation / focus can raise the banner.
pub fn set_pending_approval_task_id(task_id: Option<String>) {
    *pending_slot().lock().expect("pending approval lock") = task_id;
}

pub fn pending_approval_task_id() -> Option<String> {
    pending_slot()
        .lock()
        .expect("pending approval lock")
        .clone()
}

/// Clear the pending-approval slot only when it still holds `task_id`.
/// Returns true if the slot was cleared.
pub fn clear_pending_approval_if_matches(task_id: &str) -> bool {
    let mut slot = pending_slot().lock().expect("pending approval lock");
    if slot.as_deref() == Some(task_id) {
        *slot = None;
        true
    } else {
        false
    }
}

/// Whether app activation should raise + emit the approval banner.
pub fn should_focus_on_activation() -> bool {
    pending_approval_task_id().is_some()
}

fn request_dock_attention<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app
        .get_webview_window("main")
        .or_else(|| app.webview_windows().into_values().next())
    {
        // Bounce dock / flash taskbar so the user notices even when click
        // callbacks are unavailable on desktop.
        let _ = window.request_user_attention(Some(UserAttentionType::Critical));
    }
}

/// Emit `approval_pending` to the frontend and show an OS notification when possible.
///
/// Desktop note: `@tauri-apps/plugin-notification` `onAction` is **mobile-only**.
/// On Win/mac/Linux we: (1) show a toast, (2) request dock/taskbar attention,
/// (3) raise + focus the banner when the app is activated (`RunEvent::Reopen` on
/// macOS / `WindowEvent::Focused(true)` on all desktop) while a pending approval
/// is recorded. See `docs/qa/ap-10-notification-click.md`.
pub fn emit_approval_pending<R: Runtime>(app: &AppHandle<R>, payload: ApprovalPendingPayload) {
    set_pending_approval_task_id(Some(payload.task_id.clone()));
    let _ = app.emit(APPROVAL_PENDING_EVENT, &payload);
    let title = "Approval required";
    let body = if payload.summary.trim().is_empty() {
        format!("{} → {}", payload.from_agent, payload.to_agent)
    } else {
        payload.summary.clone()
    };
    if let Err(err) = app
        .notification()
        .builder()
        .title(title)
        .body(&body)
        .extra("task_id", &payload.task_id)
        .extra("event", APPROVAL_PENDING_EVENT)
        .show()
    {
        log::warn!("OS notification skipped: {err}");
    }
    request_dock_attention(app);
}

/// Focus the main window and ask the UI to open the approval banner.
pub fn focus_app_for_approval<R: Runtime>(app: &AppHandle<R>, task_id: Option<&str>) {
    let resolved = task_id
        .map(str::to_string)
        .or_else(pending_approval_task_id);
    if let Some(window) = app
        .get_webview_window("main")
        .or_else(|| app.webview_windows().into_values().next())
    {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        // Clear attention once the user is looking at the app.
        let _ = window.request_user_attention(None);
    }
    let _ = app.emit(
        APPROVAL_BANNER_FOCUS_EVENT,
        &ApprovalResolvedPayload {
            task_id: resolved.unwrap_or_default(),
            reason: "notification_click".to_string(),
        },
    );
}

/// Called from the Tauri run loop when the app is activated (dock/taskbar/reopen)
/// or the main window gains focus while an approval is still pending.
pub fn on_app_activated_for_pending_approval<R: Runtime>(app: &AppHandle<R>) {
    if should_focus_on_activation() {
        focus_app_for_approval(app, None);
    }
}

/// Emit `approval_resolved` when a pending approval is cleared.
///
/// Slot clear is **id-matched only**: a different `task_id` must not wipe the
/// current pending approval.
pub fn emit_approval_resolved<R: Runtime>(app: &AppHandle<R>, task_id: &str, reason: &str) {
    clear_pending_approval_if_matches(task_id);
    let payload = ApprovalResolvedPayload {
        task_id: task_id.to_string(),
        reason: reason.to_string(),
    };
    let _ = app.emit(APPROVAL_RESOLVED_EVENT, &payload);
}

/// NATS subject for destructive / routing approval_pending bus events (F3).
pub const APPROVAL_PENDING_NATS: &str = "lounge.approval.pending";

/// Wire PolicyGate destructive blocks → Tauri event + OS notification + NATS.
pub fn install_destructive_approval_emitter<R: Runtime>(app: AppHandle<R>, nats_url: String) {
    crate::kernel::destructive_confirm::install_emitter(move |event| {
        let payload = ApprovalPendingPayload::from_destructive(event);
        emit_approval_pending(&app, payload);
        let body = serde_json::to_vec(event).unwrap_or_default();
        let url = nats_url.clone();
        // Best-effort bus publish; failures are non-fatal (UI already notified).
        let _ = std::thread::Builder::new()
            .name("destructive-nats".into())
            .spawn(move || {
                // Sync nats client is deprecated upstream; keep until async bus refactor.
                #[allow(deprecated)]
                let connect = nats::connect(&url);
                if let Ok(nc) = connect {
                    let _ = nc.publish(APPROVAL_PENDING_NATS, body);
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_slot_round_trips_and_clears() {
        set_pending_approval_task_id(Some("task-1".into()));
        assert_eq!(pending_approval_task_id().as_deref(), Some("task-1"));
        set_pending_approval_task_id(None);
        assert_eq!(pending_approval_task_id(), None);
    }

    #[test]
    fn clear_pending_if_matches_only_matching_id() {
        set_pending_approval_task_id(Some("task-keep".into()));
        assert!(!clear_pending_approval_if_matches("task-other"));
        assert_eq!(pending_approval_task_id().as_deref(), Some("task-keep"));
        assert!(clear_pending_approval_if_matches("task-keep"));
        assert_eq!(pending_approval_task_id(), None);
        // Clearing again / clearing empty is a no-op.
        assert!(!clear_pending_approval_if_matches("task-keep"));
        assert!(!should_focus_on_activation());
    }

    #[test]
    fn resolve_paths_clear_slot_so_activation_is_inert() {
        // Mirrors resolve_vote / await_approval (Approve, Deny, ApproveLocal, routing).
        for (id, _vote) in [
            ("approve-1", "Approve"),
            ("deny-1", "Deny"),
            ("local-1", "ApproveLocal"),
            ("route-1", "routing"),
        ] {
            set_pending_approval_task_id(Some(id.into()));
            assert!(should_focus_on_activation());
            assert!(clear_pending_approval_if_matches(id));
            assert_eq!(pending_approval_task_id(), None);
            assert!(
                !should_focus_on_activation(),
                "after {id} resolution, activation must not raise/emit"
            );
        }
    }

    #[test]
    fn mismatched_clear_leaves_pending_so_activation_still_armed() {
        set_pending_approval_task_id(Some("live-approval".into()));
        assert!(!clear_pending_approval_if_matches("stale-other"));
        assert_eq!(pending_approval_task_id().as_deref(), Some("live-approval"));
        assert!(should_focus_on_activation());
        // Clean up for other tests sharing the process-wide slot.
        assert!(clear_pending_approval_if_matches("live-approval"));
    }
}
