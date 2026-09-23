//! DecisionGate KNOWLEDGE_HIT → get_relevant_context / fast_retrieve → NATS fısıltı.

use anyhow::Result;

use crate::db::{knowledge_hit_triggers, ExperienceStore, FastRetrieveQuery};
use crate::infra::BusManager;
use crate::kernel::decision_engine::DecisionResult;
use crate::models::{
    is_kernel, now_rfc3339, SystemPromptAddon, AGENT_PROMPT, CONTEXT_WHISPER, KERNEL_AGENT,
};

pub async fn inject_knowledge_hit(
    store: &ExperienceStore,
    bus: &BusManager,
    result: &DecisionResult,
) -> Result<Option<SystemPromptAddon>> {
    let Some(addon) = build_knowledge_whisper(store, result).await? else {
        return Ok(None);
    };
    let mut envelope = lounge_protocol::LoungeMessage::new(
        CONTEXT_WHISPER,
        KERNEL_AGENT,
        serde_json::to_value(&addon)?,
    );
    envelope.target_agent = Some(addon.target_agent.clone());
    envelope.created_at = now_rfc3339();
    bus.publish(&envelope).await?;
    Ok(Some(addon))
}

/// MATCH kapısı + çapraz-proje recall; NATS yayınlamadan SystemPromptAddon üretir.
pub async fn build_knowledge_whisper(
    store: &ExperienceStore,
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
    Ok(Some(SystemPromptAddon::from_context(
        result.message_id.clone(),
        target,
        result.knowledge_hit,
        context,
    )))
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
    use crate::db::get_relevant_context;
    use crate::kernel::decision_engine::{RecallHint, RoutingType, Scored, SecurityLevel};
    use crate::models::{ExperienceOutcome, ExperienceRecord, LoungeTask, TASK_REQUESTED};
    use std::collections::HashMap;

    fn scored<T: Clone>(value: T) -> Scored<T> {
        Scored {
            value: value.clone(),
            confidence: 0.9,
            probabilities: HashMap::new(),
        }
    }

    fn decision(hit: f32, query: &str, subject: &str) -> DecisionResult {
        DecisionResult {
            message_id: "t1".into(),
            subject: subject.into(),
            routing: scored(RoutingType::Task),
            security: scored(SecurityLevel::Safe),
            knowledge_hit: hit,
            elapsed_ms: 3,
            elapsed_us: 3_000,
            device: "cpu".into(),
            recall: RecallHint {
                query: query.into(),
                project_id: "lounge".into(),
                source_agent: "cursor".into(),
                target_agent: None,
                ast_refs: vec![],
            },
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

    #[test]
    fn whisper_subject_is_agent_prompt() {
        assert_eq!(CONTEXT_WHISPER, "lounge.agent.prompt");
        assert_eq!(CONTEXT_WHISPER, AGENT_PROMPT);
        let raw = include_str!("../../../shared/lounge_protocol/subjects.json");
        let json: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(json["context"]["whisper"], CONTEXT_WHISPER);
        assert_eq!(json["agent"]["prompt"], CONTEXT_WHISPER);
    }

    #[tokio::test]
    async fn miss_does_not_build_whisper() {
        let store = ExperienceStore::memory().unwrap();
        let low = decision(0.1, "nats dispatcher", TASK_REQUESTED);
        assert!(build_knowledge_whisper(&store, &low)
            .await
            .unwrap()
            .is_none());

        let wrong_subject = decision(0.9, "nats dispatcher", AGENT_PROMPT);
        assert!(build_knowledge_whisper(&store, &wrong_subject)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn match_triggers_recall_and_prompt_addon() {
        let store = ExperienceStore::memory().unwrap();
        let prior = LoungeTask::new(
            "claude",
            "sister-os",
            "NATS dispatcher dinleyici lounge.task.requested",
        );
        store
            .insert_record(ExperienceRecord::from_task(
                &prior,
                "sync nats client blocking thread",
                "spawn_blocking + mpsc",
                ExperienceOutcome::Success,
                vec![],
            ))
            .await
            .unwrap();

        let result = decision(0.84, "dispatcher NATS mesajlarını dinle", TASK_REQUESTED);
        let addon = build_knowledge_whisper(&store, &result)
            .await
            .unwrap()
            .expect("MATCH recall whisper");
        assert_eq!(addon.target_agent, "cursor");
        assert!(addon.prompt.contains("Cross-Project Memory"));
        assert!(
            addon.prompt.contains("spawn_blocking") || addon.prompt.contains("NATS"),
            "prompt addon tecrübe metni içermeli"
        );
        assert!(addon.knowledge_hit >= 0.3);

        let context = get_relevant_context(&store, result.recall.query.clone())
            .await
            .unwrap();
        assert!(
            !context.is_empty(),
            "get_relevant_context çapraz proje bulmalı"
        );
    }
}
