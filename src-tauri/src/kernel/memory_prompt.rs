//! DecisionGate KNOWLEDGE_HIT → fast_retrieve → NATS system prompt eklentisi.

use anyhow::Result;

use crate::db::{knowledge_hit_triggers, ExperienceStore, FastRetrieveQuery};
use crate::infra::BusManager;
use crate::kernel::decision_engine::DecisionResult;
use crate::models::{is_kernel, now_rfc3339, SystemPromptAddon, AGENT_PROMPT, KERNEL_AGENT};

pub async fn inject_knowledge_hit(
    store: &ExperienceStore,
    bus: &BusManager,
    result: &DecisionResult,
) -> Result<Option<SystemPromptAddon>> {
    if !knowledge_hit_triggers(result.knowledge_hit, &result.subject, &result.recall.query) {
        return Ok(None);
    }
    let query = FastRetrieveQuery {
        project_id: result.recall.project_id.clone(),
        text: result.recall.query.clone(),
        embedding: None,
        knowledge_hit: result.knowledge_hit,
        ast_refs: result.recall.ast_refs.clone(),
        limit: Some(4),
    };
    let context = store.fast_retrieve(query).await?;
    if context.is_empty() {
        return Ok(None);
    }
    let target = prompt_target(result);
    let addon = SystemPromptAddon::from_context(
        result.message_id.clone(),
        target.clone(),
        result.knowledge_hit,
        context,
    );
    let mut envelope = lounge_protocol::LoungeMessage::new(
        AGENT_PROMPT,
        KERNEL_AGENT,
        serde_json::to_value(&addon)?,
    );
    envelope.target_agent = Some(target);
    envelope.created_at = now_rfc3339();
    bus.publish(&envelope).await?;
    Ok(Some(addon))
}

fn prompt_target(result: &DecisionResult) -> String {
    if let Some(agent) = result
        .recall
        .target_agent
        .as_deref()
        .filter(|value| !value.is_empty() && !is_kernel(value))
    {
        return agent.to_string();
    }
    let source = result.recall.source_agent.trim();
    if !source.is_empty() && !is_kernel(source) && source != "nats" && source != "lounge-bus" {
        return source.to_string();
    }
    result.recall.source_agent.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::decision_engine::{RecallHint, RoutingType, Scored, SecurityLevel};
    use crate::models::TASK_REQUESTED;
    use std::collections::HashMap;

    fn scored<T: Clone>(value: T) -> Scored<T> {
        Scored {
            value: value.clone(),
            confidence: 0.9,
            probabilities: HashMap::new(),
        }
    }

    #[test]
    fn prefers_working_agent_over_kernel() {
        let result = DecisionResult {
            message_id: "t1".into(),
            subject: TASK_REQUESTED.into(),
            routing: scored(RoutingType::Task),
            security: scored(SecurityLevel::Safe),
            knowledge_hit: 0.9,
            elapsed_ms: 3,
            elapsed_us: 3_000,
            device: "cpu".into(),
            recall: RecallHint {
                query: "nats dispatcher".into(),
                project_id: "lounge".into(),
                source_agent: "cursor".into(),
                target_agent: Some(KERNEL_AGENT.into()),
                ast_refs: vec![],
            },
        };
        assert_eq!(prompt_target(&result), "cursor");
    }
}
