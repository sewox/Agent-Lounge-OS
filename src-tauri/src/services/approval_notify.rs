//! OS + frontend notifications when approvals are pending or resolved.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
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

/// Emit `approval_pending` to the frontend and show an OS notification when possible.
///
/// No volume/interval escalation while backgrounded — a single OS toast + event.
/// Notification click → focus is handled by [`focus_app_for_approval`] (FE `onAction`
/// or OS activate); desktop `tauri-plugin-notification` has no Rust click callback.
pub fn emit_approval_pending<R: Runtime>(app: &AppHandle<R>, payload: ApprovalPendingPayload) {
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
        log::debug!("OS notification skipped: {err}");
    }
}

/// Focus the main window and ask the UI to open the approval banner.
pub fn focus_app_for_approval<R: Runtime>(app: &AppHandle<R>, task_id: Option<&str>) {
    if let Some(window) = app
        .get_webview_window("main")
        .or_else(|| app.webview_windows().into_values().next())
    {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = app.emit(
        APPROVAL_BANNER_FOCUS_EVENT,
        &ApprovalResolvedPayload {
            task_id: task_id.unwrap_or("").to_string(),
            reason: "notification_click".to_string(),
        },
    );
}

/// Emit `approval_resolved` when a pending approval is cleared.
pub fn emit_approval_resolved<R: Runtime>(app: &AppHandle<R>, task_id: &str, reason: &str) {
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
