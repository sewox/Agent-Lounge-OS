use serde::{Deserialize, Serialize};

use super::protocol::LoungeTask;

pub const KERNEL_AGENT: &str = "lounge-kernel";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum QuotaExhaustedAction {
    /// Kotam biterse görevi durdur.
    Stop,
    /// Onay alarak yerel modele geç.
    #[default]
    AskThenLocal,
    /// Onay alarak görevi iptal et.
    AskThenAbort,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentTrigger {
    pub agent_id: String,
    pub label: String,
    /// code_analysis | review | general | fallback
    pub when: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoutingPolicy {
    /// Ajanlar arası geçiş asla otomatik olamaz; UI'da kilitli tutulur.
    pub require_user_approval: bool,
    pub on_quota_exhausted: QuotaExhaustedAction,
    pub local_fallback_agent: String,
    pub local_fallback_model: String,
    pub triggers: Vec<AgentTrigger>,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self {
            require_user_approval: true,
            on_quota_exhausted: QuotaExhaustedAction::AskThenLocal,
            local_fallback_agent: "lmr".into(),
            local_fallback_model: crate::models::DEFAULT_OLLAMA_MODEL.into(),
            triggers: vec![
                AgentTrigger {
                    agent_id: "cursor".into(),
                    label: "Cursor".into(),
                    when: "code_analysis".into(),
                    enabled: true,
                },
                AgentTrigger {
                    agent_id: "claude".into(),
                    label: "Claude".into(),
                    when: "review".into(),
                    enabled: true,
                },
                AgentTrigger {
                    agent_id: "grok".into(),
                    label: "Grok".into(),
                    when: "general".into(),
                    enabled: true,
                },
                AgentTrigger {
                    agent_id: "lmr".into(),
                    label: "LMR".into(),
                    when: "fallback".into(),
                    enabled: true,
                },
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    AgentSwitch,
    QuotaLocalFallback,
    QuotaAbort,
    SecurityCritical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub task_id: String,
    pub summary: String,
    pub from_agent: String,
    pub to_agent: String,
    pub kind: ApprovalKind,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoutingVote {
    Approve,
    ApproveLocal,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteIntent {
    Allow { agent: String },
    Stop { reason: String },
    NeedApproval { request: ApprovalRequest },
}

pub fn is_kernel(agent: &str) -> bool {
    matches!(
        agent.trim().to_ascii_lowercase().as_str(),
        "" | "lounge-kernel" | "kernel" | "dispatcher"
    )
}

/// Ajanlar arası otomatik geçiş yok: hedef harici ajan ve kaynaktan farklıysa onay şart.
pub fn decide_route(
    policy: &RoutingPolicy,
    task: &LoungeTask,
    from_agent: &str,
    to_agent: &str,
    quota_exhausted: bool,
) -> RouteIntent {
    let to = if to_agent.trim().is_empty() {
        KERNEL_AGENT
    } else {
        to_agent
    };

    if quota_exhausted {
        let reason = format!(
            "{} kotası tükendi (policy={:?})",
            to, policy.on_quota_exhausted
        );
        return match policy.on_quota_exhausted {
            QuotaExhaustedAction::Stop => RouteIntent::Stop { reason },
            QuotaExhaustedAction::AskThenLocal => RouteIntent::NeedApproval {
                request: ApprovalRequest {
                    task_id: task.id.clone(),
                    summary: task.summary.clone(),
                    from_agent: from_agent.into(),
                    to_agent: policy.local_fallback_agent.clone(),
                    kind: ApprovalKind::QuotaLocalFallback,
                    reason,
                },
            },
            QuotaExhaustedAction::AskThenAbort => RouteIntent::NeedApproval {
                request: ApprovalRequest {
                    task_id: task.id.clone(),
                    summary: task.summary.clone(),
                    from_agent: from_agent.into(),
                    to_agent: to.into(),
                    kind: ApprovalKind::QuotaAbort,
                    reason,
                },
            },
        };
    }

    if !is_kernel(to) && !from_agent.eq_ignore_ascii_case(to) {
        return RouteIntent::NeedApproval {
            request: ApprovalRequest {
                task_id: task.id.clone(),
                summary: task.summary.clone(),
                from_agent: from_agent.into(),
                to_agent: to.into(),
                kind: ApprovalKind::AgentSwitch,
                reason: "ajanlar arası geçiş kullanıcı onayı ister".into(),
            },
        };
    }

    let _ = policy.require_user_approval;
    RouteIntent::Allow { agent: to.into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LoungeTask;

    #[test]
    fn never_auto_routes_between_external_agents() {
        let policy = RoutingPolicy::default();
        let task = LoungeTask::new("cursor", "agent-lounge-os", "refactor dispatcher");
        let intent = decide_route(&policy, &task, "cursor", "ollama", false);
        assert!(matches!(
            intent,
            RouteIntent::NeedApproval {
                request: ApprovalRequest {
                    kind: ApprovalKind::AgentSwitch,
                    ..
                }
            }
        ));
    }

    #[test]
    fn kernel_assignment_is_not_an_agent_switch() {
        let policy = RoutingPolicy::default();
        let task = LoungeTask::new("cursor", "agent-lounge-os", "ack");
        let intent = decide_route(&policy, &task, "cursor", KERNEL_AGENT, false);
        assert_eq!(
            intent,
            RouteIntent::Allow {
                agent: KERNEL_AGENT.into()
            }
        );
    }

    #[test]
    fn quota_stop_fails_closed() {
        let policy = RoutingPolicy {
            on_quota_exhausted: QuotaExhaustedAction::Stop,
            ..Default::default()
        };
        let task = LoungeTask::new("cursor", "agent-lounge-os", "gen");
        let intent = decide_route(&policy, &task, "cursor", "cursor", true);
        assert!(matches!(intent, RouteIntent::Stop { .. }));
    }

    #[test]
    fn quota_ask_local_requires_approval() {
        let policy = RoutingPolicy::default();
        let task = LoungeTask::new("cursor", "agent-lounge-os", "gen");
        let intent = decide_route(&policy, &task, "cursor", "cursor", true);
        assert!(matches!(
            intent,
            RouteIntent::NeedApproval {
                request: ApprovalRequest {
                    kind: ApprovalKind::QuotaLocalFallback,
                    ..
                }
            }
        ));
    }
}
