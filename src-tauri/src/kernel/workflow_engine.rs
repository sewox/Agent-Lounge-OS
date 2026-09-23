//! Multi-Agent Workflow — başarılı Review/Coding sonrası otomatik Test görevi.

#![allow(deprecated)]

use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::models::{
    is_kernel, LoungeTask, TaskKind, KERNEL_AGENT, TASK_COMPLETED, TASK_REQUESTED, TEST_REQUESTED,
};
use crate::services::autodiscover::find_grok_bot;

/// Fleet / discovery worker id — Grok Bot.
pub const GROK_BOT_WORKER: &str = "grok_bot";
/// Yerel test worker — Worker Fleet paneliyle aynı id.
pub const LOCAL_TEST_WORKER: &str = "lounge-kernel";
const WORKFLOW_AGENT: &str = "workflow_engine";

#[derive(Debug, Clone)]
pub struct WorkflowEngine {
    nats_url: String,
    /// Testlerde fleet override; `None` → canlı discovery.
    fleet_has_grok: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestDispatch {
    pub task: LoungeTask,
    pub subject: &'static str,
    pub target_agent: String,
    pub chain_label: String,
}

impl WorkflowEngine {
    pub fn new(nats_url: impl Into<String>) -> Self {
        Self {
            nats_url: nats_url.into(),
            fleet_has_grok: None,
        }
    }

    /// Birim testleri için Grok Bot varlık durumunu sabitler.
    pub fn with_grok_in_fleet(mut self, present: bool) -> Self {
        self.fleet_has_grok = Some(present);
        self
    }

    pub async fn listen(&self) -> Result<()> {
        loop {
            match self.listen_once().await {
                Ok(()) => log::warn!("workflow_engine NATS dinleyici kapandı, yeniden bağlanılıyor"),
                Err(err) => log::error!("workflow_engine NATS: {err}"),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn listen_once(&self) -> Result<()> {
        let url = self.nats_url.clone();
        let nc = tokio::task::spawn_blocking(move || nats::connect(&url))
            .await
            .context("workflow_engine NATS connect join")?
            .context("workflow_engine NATS bağlantısı kurulamadı")?;

        let (tx, mut rx) = mpsc::channel::<nats::Message>(64);
        let listener = nc.clone();
        tokio::task::spawn_blocking(move || {
            let sub = listener.subscribe(TASK_COMPLETED)?;
            for msg in sub.messages() {
                if tx.blocking_send(msg).is_err() {
                    break;
                }
            }
            Ok::<_, std::io::Error>(())
        });

        log::info!(
            "workflow_engine dinliyor: {} ({TASK_COMPLETED})",
            self.nats_url
        );

        while let Some(msg) = rx.recv().await {
            if let Err(err) = self.handle_completed(&nc, &msg.data).await {
                log::warn!("workflow_engine tamamlanan görev işlenemedi: {err}");
            }
        }
        Ok(())
    }

    async fn handle_completed(&self, nc: &nats::Connection, data: &[u8]) -> Result<()> {
        let completed: LoungeTask =
            serde_json::from_slice(data).context("lounge.task.completed payload")?;
        let Some(dispatch) = self.plan_test_followup(&completed) else {
            return Ok(());
        };
        publish_task(nc, dispatch.subject, &dispatch.task).await?;
        log::info!(
            "workflow_engine Test tetiklendi: {} → {} ({})",
            completed.id,
            dispatch.task.id,
            dispatch.chain_label
        );
        Ok(())
    }

    /// Başarılı Review / Coding (CodeAnalysis) → Test planı; aksi halde `None`.
    pub fn plan_test_followup(&self, completed: &LoungeTask) -> Option<TestDispatch> {
        if !triggers_followup_test(&completed.kind) {
            return None;
        }
        // `lounge.task.completed` zaten başarı; failed ayrı subject.
        // Workflow'un kendi ürettiği Test'i tekrar zincirleme.
        if completed.parent_task_id.is_some() && matches!(completed.kind, TaskKind::Test) {
            return None;
        }

        let has_grok = self.fleet_has_grok.unwrap_or_else(grok_bot_in_fleet);
        let (target_agent, subject) = resolve_test_target(has_grok);
        let parent_agent = working_agent(completed);
        let chain = format_workflow_chain(
            parent_agent,
            &completed.kind,
            &target_agent,
            &TaskKind::Test,
        );

        let mut task = LoungeTask::new(WORKFLOW_AGENT, &completed.project_id, test_summary(completed));
        task.kind = TaskKind::Test;
        task.target_agent = Some(target_agent.clone());
        task.parent_task_id = Some(completed.id.clone());
        task.workflow_chain = Some(chain.clone());
        task.repo_path = completed.repo_path.clone();
        task.ast_refs = completed.ast_refs.clone();
        task.priority = completed.priority.clone();

        Some(TestDispatch {
            task,
            subject,
            target_agent,
            chain_label: chain,
        })
    }
}

/// Review veya Coding (`code_analysis`) başarıyla bittiyse Test üretilir.
pub fn triggers_followup_test(kind: &TaskKind) -> bool {
    matches!(kind, TaskKind::Review | TaskKind::CodeAnalysis)
}

pub fn resolve_test_target(grok_in_fleet: bool) -> (String, &'static str) {
    if grok_in_fleet {
        (GROK_BOT_WORKER.into(), TASK_REQUESTED)
    } else {
        (LOCAL_TEST_WORKER.into(), TEST_REQUESTED)
    }
}

pub fn grok_bot_in_fleet() -> bool {
    find_grok_bot().is_some()
}

pub fn working_agent(task: &LoungeTask) -> &str {
    task.target_agent
        .as_deref()
        .map(str::trim)
        .filter(|agent| !agent.is_empty() && !is_kernel(agent))
        .unwrap_or(task.source_agent.as_str())
}

pub fn kind_chain_label(kind: &TaskKind) -> &'static str {
    match kind {
        TaskKind::CodeAnalysis => "Code",
        TaskKind::Review => "Review",
        TaskKind::Test => "Test",
        TaskKind::General => "General",
        TaskKind::Orchestration => "Orchestration",
    }
}

pub fn agent_chain_label(agent: &str) -> String {
    let lower = agent.trim().to_ascii_lowercase();
    if lower.contains("claude") {
        "Claude".into()
    } else if lower.contains("grok") {
        "Grok".into()
    } else if lower.contains("cursor") {
        "Cursor".into()
    } else if lower.contains("antigravity") {
        "Antigravity".into()
    } else if lower == "lmr" || lower.contains("laya") {
        "LMR".into()
    } else if is_kernel(agent) || lower == LOCAL_TEST_WORKER {
        "Kernel".into()
    } else if agent.trim().is_empty() {
        KERNEL_AGENT.into()
    } else {
        agent.trim().into()
    }
}

/// Örn. `Claude (Code) -> Grok (Test)`.
pub fn format_workflow_chain(
    from_agent: &str,
    from_kind: &TaskKind,
    to_agent: &str,
    to_kind: &TaskKind,
) -> String {
    format!(
        "{} ({}) -> {} ({})",
        agent_chain_label(from_agent),
        kind_chain_label(from_kind),
        agent_chain_label(to_agent),
        kind_chain_label(to_kind)
    )
}

fn test_summary(completed: &LoungeTask) -> String {
    format!(
        "Auto-Test after {}: {}",
        kind_chain_label(&completed.kind),
        completed.summary
    )
}

async fn publish_task(nc: &nats::Connection, subject: &str, task: &LoungeTask) -> Result<()> {
    let bytes = serde_json::to_vec(task)?;
    let nc = nc.clone();
    let subject = subject.to_string();
    let subject_for_err = subject.clone();
    tokio::task::spawn_blocking(move || nc.publish(&subject, bytes))
        .await
        .context("workflow_engine NATS publish join")?
        .with_context(|| format!("workflow_engine NATS publish başarısız: {subject_for_err}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completed(kind: TaskKind, agent: &str) -> LoungeTask {
        let mut task = LoungeTask::new(agent, "agent-lounge-os", "implement workflow engine");
        task.kind = kind;
        task.target_agent = Some(agent.into());
        task
    }

    #[test]
    fn review_and_coding_emit_test_task() {
        let engine = WorkflowEngine::new("nats://127.0.0.1:4222").with_grok_in_fleet(true);

        for kind in [TaskKind::Review, TaskKind::CodeAnalysis] {
            let parent = completed(kind.clone(), "claude");
            let dispatch = engine.plan_test_followup(&parent).expect("follow-up");
            assert_eq!(dispatch.task.kind, TaskKind::Test);
            assert_eq!(dispatch.task.parent_task_id.as_deref(), Some(parent.id.as_str()));
            assert_eq!(dispatch.target_agent, GROK_BOT_WORKER);
            assert_eq!(dispatch.subject, TASK_REQUESTED);
            assert!(dispatch.task.workflow_chain.as_ref().unwrap().contains("->"));
        }
    }

    #[test]
    fn other_kinds_and_nested_test_do_not_emit() {
        let engine = WorkflowEngine::new("nats://127.0.0.1:4222").with_grok_in_fleet(true);

        for kind in [TaskKind::General, TaskKind::Test, TaskKind::Orchestration] {
            let parent = completed(kind, "claude");
            assert!(engine.plan_test_followup(&parent).is_none());
        }

        let mut nested = completed(TaskKind::Review, "claude");
        nested.parent_task_id = Some("already-chained".into());
        nested.kind = TaskKind::Test;
        assert!(engine.plan_test_followup(&nested).is_none());
    }

    #[test]
    fn without_grok_uses_local_test_subject() {
        let engine = WorkflowEngine::new("nats://127.0.0.1:4222").with_grok_in_fleet(false);
        let parent = completed(TaskKind::CodeAnalysis, "cursor");
        let dispatch = engine.plan_test_followup(&parent).expect("follow-up");
        assert_eq!(dispatch.target_agent, LOCAL_TEST_WORKER);
        assert_eq!(dispatch.subject, TEST_REQUESTED);
        assert_eq!(
            dispatch.chain_label,
            "Cursor (Code) -> Kernel (Test)"
        );
    }

    #[test]
    fn chain_label_formats_claude_code_to_grok_test() {
        let label = format_workflow_chain("claude", &TaskKind::CodeAnalysis, "grok_bot", &TaskKind::Test);
        assert_eq!(label, "Claude (Code) -> Grok (Test)");

        let review = format_workflow_chain("Claude Desktop", &TaskKind::Review, GROK_BOT_WORKER, &TaskKind::Test);
        assert_eq!(review, "Claude (Review) -> Grok (Test)");
    }

    #[test]
    fn triggers_only_review_and_code_analysis() {
        assert!(triggers_followup_test(&TaskKind::Review));
        assert!(triggers_followup_test(&TaskKind::CodeAnalysis));
        assert!(!triggers_followup_test(&TaskKind::Test));
        assert!(!triggers_followup_test(&TaskKind::General));
    }
}
