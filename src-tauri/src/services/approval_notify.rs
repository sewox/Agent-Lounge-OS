//! OS + frontend notifications when approvals are pending or resolved.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tauri_plugin_notification::NotificationExt;

use crate::models::ApprovalRequest;

pub const APPROVAL_PENDING_EVENT: &str = "approval_pending";
pub const APPROVAL_RESOLVED_EVENT: &str = "approval_resolved";

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalPendingPayload {
    pub task_id: String,
    pub summary: String,
    pub from_agent: String,
    pub to_agent: String,
    pub kind: String,
    pub reason: String,
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
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ApprovalResolvedPayload {
    pub task_id: String,
    pub reason: String,
}

/// Emit `approval_pending` to the frontend and show an OS notification when possible.
pub fn emit_approval_pending<R: Runtime>(app: &AppHandle<R>, payload: ApprovalPendingPayload) {
    let _ = app.emit(APPROVAL_PENDING_EVENT, &payload);
    let title = "Approval required";
    let body = if payload.summary.trim().is_empty() {
        format!("{} → {}", payload.from_agent, payload.to_agent)
    } else {
        payload.summary.clone()
    };
    if let Err(err) = app.notification().builder().title(title).body(&body).show() {
        log::debug!("OS notification skipped: {err}");
    }
}

/// Emit `approval_resolved` when a pending approval is cleared.
pub fn emit_approval_resolved<R: Runtime>(app: &AppHandle<R>, task_id: &str, reason: &str) {
    let payload = ApprovalResolvedPayload {
        task_id: task_id.to_string(),
        reason: reason.to_string(),
    };
    let _ = app.emit(APPROVAL_RESOLVED_EVENT, &payload);
}
