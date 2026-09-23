//! Security Policy — DecisionGate SECURITY Risky/Critical → görev askıya alma + UI onayı.

use serde::{Deserialize, Serialize};

use crate::kernel::decision_engine::{DecisionResult, SecurityLevel};
use crate::models::{ApprovalKind, ApprovalRequest, KERNEL_AGENT};

/// NATS: güvenlik uyarısı (UI Security Overlay).
pub const ALERT_SECURITY: &str = "lounge.alert.security";
/// NATS: kullanıcı onayı sonrası görevi sürdür.
pub const TASK_RESUME: &str = "lounge.task.resume";

/// Overlay metni (Stitch / TR).
pub const SECURITY_OVERLAY_PROMPT: &str =
    "Ajan kritik bir dosyayı değiştirmek istiyor. Onaylıyor musunuz?";

/// Risky veya Critical → işlem askıya alınır.
pub fn requires_suspend(level: SecurityLevel) -> bool {
    matches!(level, SecurityLevel::Risky | SecurityLevel::Critical)
}

/// DecisionGate güvenlik skoru askı gerektiriyorsa onay isteği üretir.
pub fn evaluate_security(
    task_id: &str,
    summary: &str,
    from_agent: &str,
    decision: &DecisionResult,
) -> Option<ApprovalRequest> {
    let level = decision.security.value;
    if !requires_suspend(level) {
        return None;
    }
    let kind = match level {
        SecurityLevel::Critical => ApprovalKind::SecurityCritical,
        SecurityLevel::Risky => ApprovalKind::SecurityRisky,
        SecurityLevel::Safe => return None,
    };
    let p = decision
        .security
        .probabilities
        .get(level.as_str())
        .copied()
        .unwrap_or(decision.security.confidence);
    Some(ApprovalRequest {
        task_id: task_id.into(),
        summary: summary.into(),
        from_agent: from_agent.into(),
        to_agent: KERNEL_AGENT.into(),
        kind,
        reason: format!(
            "DecisionGate SECURITY_LEVEL={} (p={:.2}) · {SECURITY_OVERLAY_PROMPT}",
            level.as_str(),
            p
        ),
    })
}

pub fn is_security_approval(kind: &ApprovalKind) -> bool {
    matches!(
        kind,
        ApprovalKind::SecurityCritical | ApprovalKind::SecurityRisky
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SecurityAlertPayload {
    pub task_id: String,
    pub summary: String,
    pub from_agent: String,
    pub to_agent: String,
    pub kind: ApprovalKind,
    pub reason: String,
    pub security_level: String,
    pub message: String,
    /// UI / protokol: görev askıda.
    pub status: String,
}

impl SecurityAlertPayload {
    pub fn from_request(request: &ApprovalRequest, level: SecurityLevel) -> Self {
        Self {
            task_id: request.task_id.clone(),
            summary: request.summary.clone(),
            from_agent: request.from_agent.clone(),
            to_agent: request.to_agent.clone(),
            kind: request.kind.clone(),
            reason: request.reason.clone(),
            security_level: level.as_str().into(),
            message: SECURITY_OVERLAY_PROMPT.into(),
            status: "suspending".into(),
        }
    }

    pub fn as_approval(&self) -> ApprovalRequest {
        ApprovalRequest {
            task_id: self.task_id.clone(),
            summary: self.summary.clone(),
            from_agent: self.from_agent.clone(),
            to_agent: self.to_agent.clone(),
            kind: self.kind.clone(),
            reason: self.reason.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskResumePayload {
    pub task_id: String,
    pub vote: String,
    pub status: String,
}

impl TaskResumePayload {
    pub fn approved(task_id: impl Into<String>) -> Self {
        Self {
            task_id: task_id.into(),
            vote: "approve".into(),
            status: "resumed".into(),
        }
    }
}

/// `lounge.alert.security` zarf gövdesi.
pub fn alert_envelope_payload(request: &ApprovalRequest, level: SecurityLevel) -> serde_json::Value {
    serde_json::to_value(SecurityAlertPayload::from_request(request, level))
        .unwrap_or_else(|_| serde_json::json!({ "task_id": request.task_id }))
}

/// `lounge.task.resume` zarf gövdesi.
pub fn resume_envelope_payload(task_id: &str) -> serde_json::Value {
    serde_json::to_value(TaskResumePayload::approved(task_id))
        .unwrap_or_else(|_| serde_json::json!({ "task_id": task_id, "vote": "approve" }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::decision_engine::{RecallHint, RoutingType, Scored};
    use std::collections::HashMap;

    fn decision(level: SecurityLevel) -> DecisionResult {
        let key = level.as_str().to_string();
        DecisionResult {
            message_id: "msg-1".into(),
            subject: "lounge.task.requested".into(),
            routing: Scored {
                value: RoutingType::Task,
                confidence: 0.9,
                probabilities: HashMap::from([("Task".into(), 0.9)]),
            },
            security: Scored {
                value: level,
                confidence: 0.91,
                probabilities: HashMap::from([(key, 0.91)]),
            },
            knowledge_hit: 0.1,
            elapsed_ms: 3,
            elapsed_us: 3_000,
            device: "cpu".into(),
            recall: RecallHint::default(),
        }
    }

    #[test]
    fn safe_does_not_suspend() {
        assert!(!requires_suspend(SecurityLevel::Safe));
        assert!(evaluate_security("t1", "ok", "cursor", &decision(SecurityLevel::Safe)).is_none());
    }

    #[test]
    fn risky_and_critical_suspend() {
        assert!(requires_suspend(SecurityLevel::Risky));
        assert!(requires_suspend(SecurityLevel::Critical));

        let risky = evaluate_security(
            "t-risky",
            "touch secrets.env",
            "cursor",
            &decision(SecurityLevel::Risky),
        )
        .expect("risky");
        assert_eq!(risky.kind, ApprovalKind::SecurityRisky);
        assert!(risky.reason.contains("Risky"));

        let critical = evaluate_security(
            "t-crit",
            "wipe secrets",
            "cursor",
            &decision(SecurityLevel::Critical),
        )
        .expect("critical");
        assert_eq!(critical.kind, ApprovalKind::SecurityCritical);
        assert!(is_security_approval(&critical.kind));
    }

    #[test]
    fn alert_and_resume_subjects_and_payloads() {
        assert_eq!(ALERT_SECURITY, "lounge.alert.security");
        assert_eq!(TASK_RESUME, "lounge.task.resume");

        let req = evaluate_security(
            "task-9",
            "rewrite auth",
            "claude",
            &decision(SecurityLevel::Critical),
        )
        .unwrap();
        let alert = SecurityAlertPayload::from_request(&req, SecurityLevel::Critical);
        assert_eq!(alert.status, "suspending");
        assert_eq!(alert.message, SECURITY_OVERLAY_PROMPT);
        assert_eq!(alert.as_approval().task_id, "task-9");

        let resume = TaskResumePayload::approved("task-9");
        assert_eq!(resume.vote, "approve");
        assert_eq!(resume.status, "resumed");

        let resume_json = resume_envelope_payload("task-9");
        assert_eq!(resume_json["task_id"], "task-9");
        assert_eq!(resume_json["vote"], "approve");
    }

    #[test]
    fn subjects_match_shared_catalog() {
        let raw = include_str!("../../../shared/lounge_protocol/subjects.json");
        let json: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(json["alert"]["security"], ALERT_SECURITY);
        assert_eq!(json["task"]["resume"], TASK_RESUME);
    }
}
