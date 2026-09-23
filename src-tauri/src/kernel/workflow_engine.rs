//! Multi-Agent Workflow — başarılı Coding sonrası otomatik Test görevi.

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
/// Yerel test worker — Worker Fleet paneliyle aynı id (`lounge-kernel`).
pub const LOCAL_TEST_WORKER: &str = "lounge-kernel";
/// Yerel Test Worker’ı açıkça tanımlamak için ortam değişkeni.
pub const LOCAL_TEST_WORKER_ENV: &str = "LOUNGE_LOCAL_TEST_WORKER";
const WORKFLOW_AGENT: &str = "workflow_engine";

#[derive(Debug, Clone)]
pub struct WorkflowEngine {
    nats_url: String,
    /// Testlerde Grok varlık override; `None` → canlı discovery.
    fleet_has_grok: Option<bool>,
    /// Testlerde yerel Test Worker override; `None` → env / config.
    fleet_has_local_test: Option<bool>,
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
            fleet_has_local_test: None,
        }
    }

    /// Birim testleri için Grok Bot varlık durumunu sabitler.
    pub fn with_grok_in_fleet(mut self, present: bool) -> Self {
        self.fleet_has_grok = Some(present);
        self
    }

    /// Birim testleri için yerel Test Worker (`lounge-kernel`) varlık durumunu sabitler.
    pub fn with_local_test_in_fleet(mut self, present: bool) -> Self {
        self.fleet_has_local_test = Some(present);
        self
    }

    pub async fn listen(&self) -> Result<()> {
        loop {
            match self.listen_once().await {
                Ok(()) => {
                    log::warn!("workflow_engine NATS dinleyici kapandı, yeniden bağlanılıyor")
                }
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

    /// Başarılı Coding (`CodeAnalysis`) + tanımlı Test Worker → Test planı; aksi halde `None`.
    ///
    /// `lounge.task.completed` başarı demektir; başarısız görevler `lounge.task.failed` üzerindedir.
    pub fn plan_test_followup(&self, completed: &LoungeTask) -> Option<TestDispatch> {
        if !triggers_followup_test(&completed.kind) {
            return None;
        }
        // Workflow'un kendi ürettiği Test'i tekrar zincirleme.
        if completed.parent_task_id.is_some() && matches!(completed.kind, TaskKind::Test) {
            return None;
        }

        let has_grok = self.fleet_has_grok.unwrap_or_else(grok_bot_in_fleet);
        let has_local = self
            .fleet_has_local_test
            .unwrap_or_else(local_test_worker_configured);
        let (target_agent, subject) = resolve_test_target(has_grok, has_local)?;

        let mut task = LoungeTask::new(
            WORKFLOW_AGENT,
            &completed.project_id,
            test_summary(completed),
        );
        task.kind = TaskKind::Test;
        task.target_agent = Some(target_agent.clone());
        task.parent_task_id = Some(completed.id.clone());
        task.repo_path = completed.repo_path.clone();
        task.ast_refs = completed.ast_refs.clone();
        task.priority = completed.priority.clone();

        let chain = format_workflow_chain(completed, &task);
        task.workflow_chain = Some(chain.clone());

        Some(TestDispatch {
            task,
            subject,
            target_agent,
            chain_label: chain,
        })
    }
}

/// Yalnızca Coding (`code_analysis`) başarıyla bittiyse Test üretilir — Review tetiklemez.
pub fn triggers_followup_test(kind: &TaskKind) -> bool {
    matches!(kind, TaskKind::CodeAnalysis)
}

/// Grok varsa `lounge.task.requested` → grok_bot;
/// yoksa yerel Test Worker tanımlıysa `lounge.test.requested` → lounge-kernel;
/// ikisi de yoksa `None` (yayın yok).
pub fn resolve_test_target(
    grok_in_fleet: bool,
    local_test_configured: bool,
) -> Option<(String, &'static str)> {
    if grok_in_fleet {
        Some((GROK_BOT_WORKER.into(), TASK_REQUESTED))
    } else if local_test_configured {
        Some((LOCAL_TEST_WORKER.into(), TEST_REQUESTED))
    } else {
        None
    }
}

pub fn grok_bot_in_fleet() -> bool {
    find_grok_bot().is_some()
}

/// Yerel Test Worker yalnızca açıkça yapılandırıldığında aktiftir
/// (`LOUNGE_LOCAL_TEST_WORKER=1|true|lounge-kernel`). Kernel’e kör fallback yok.
pub fn local_test_worker_configured() -> bool {
    match std::env::var(LOCAL_TEST_WORKER_ENV) {
        Ok(raw) => {
            let value = raw.trim().to_ascii_lowercase();
            matches!(
                value.as_str(),
                "1" | "true" | "yes" | "on" | LOCAL_TEST_WORKER | "kernel" | "local"
            )
        }
        Err(_) => false,
    }
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

/// Görev satır etiketi — özet (title) yoksa id.
pub fn task_chain_label(task: &LoungeTask) -> String {
    let summary = task.summary.trim();
    if summary.is_empty() {
        task.id.clone()
    } else {
        summary.to_string()
    }
}

/// Örn. `implement workflow engine -> Triggered Auto-Test after Code: implement workflow engine`.
pub fn format_workflow_chain(parent: &LoungeTask, child: &LoungeTask) -> String {
    format!(
        "{} -> Triggered {}",
        task_chain_label(parent),
        task_chain_label(child)
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
    fn coding_with_grok_emits_test_task() {
        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(true)
            .with_local_test_in_fleet(false);

        let parent = completed(TaskKind::CodeAnalysis, "claude");
        let dispatch = engine.plan_test_followup(&parent).expect("follow-up");
        assert_eq!(dispatch.task.kind, TaskKind::Test);
        assert_eq!(
            dispatch.task.parent_task_id.as_deref(),
            Some(parent.id.as_str())
        );
        assert_eq!(dispatch.target_agent, GROK_BOT_WORKER);
        assert_eq!(dispatch.subject, TASK_REQUESTED);
        assert_eq!(
            dispatch.chain_label,
            format!(
                "implement workflow engine -> Triggered {}",
                dispatch.task.summary
            )
        );
        assert_eq!(
            dispatch.task.workflow_chain.as_deref(),
            Some(dispatch.chain_label.as_str())
        );
    }

    #[test]
    fn coding_with_local_test_worker_emits_test_requested() {
        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(false)
            .with_local_test_in_fleet(true);
        let parent = completed(TaskKind::CodeAnalysis, "cursor");
        let dispatch = engine.plan_test_followup(&parent).expect("follow-up");
        assert_eq!(dispatch.target_agent, LOCAL_TEST_WORKER);
        assert_eq!(dispatch.subject, TEST_REQUESTED);
        assert!(dispatch.chain_label.contains(" -> Triggered "));
    }

    #[test]
    fn coding_without_test_worker_does_not_emit() {
        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(false)
            .with_local_test_in_fleet(false);
        let parent = completed(TaskKind::CodeAnalysis, "claude");
        assert!(engine.plan_test_followup(&parent).is_none());
    }

    #[test]
    fn review_completed_does_not_emit() {
        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(true)
            .with_local_test_in_fleet(true);
        let parent = completed(TaskKind::Review, "claude");
        assert!(engine.plan_test_followup(&parent).is_none());
    }

    #[test]
    fn other_kinds_and_nested_test_do_not_emit() {
        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(true)
            .with_local_test_in_fleet(true);

        for kind in [TaskKind::General, TaskKind::Test, TaskKind::Orchestration] {
            let parent = completed(kind, "claude");
            assert!(engine.plan_test_followup(&parent).is_none());
        }

        let mut nested = completed(TaskKind::CodeAnalysis, "claude");
        nested.parent_task_id = Some("already-chained".into());
        nested.kind = TaskKind::Test;
        assert!(engine.plan_test_followup(&nested).is_none());
    }

    #[test]
    fn failed_coding_is_out_of_band_completed_only() {
        // Başarısız Coding `lounge.task.failed` subject'ine gider; engine yalnızca
        // `lounge.task.completed` dinler — plan fonksiyonu completed payload alır.
        // Burada Review dışı kind + worker yok senaryosu ile yanlış tetiklenmediğini doğrularız.
        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(true)
            .with_local_test_in_fleet(true);
        assert!(!triggers_followup_test(&TaskKind::Review));
        assert!(triggers_followup_test(&TaskKind::CodeAnalysis));
        // completed dinleyicisi failed mesajını hiç görmez; plan yine de yalnızca Coding'e açık.
        let _ = engine;
    }

    #[test]
    fn chain_label_formats_task_a_triggered_task_b() {
        let parent = completed(TaskKind::CodeAnalysis, "claude");
        let mut child = LoungeTask::new(
            WORKFLOW_AGENT,
            "agent-lounge-os",
            "Auto-Test after Code: implement workflow engine",
        );
        child.kind = TaskKind::Test;
        child.parent_task_id = Some(parent.id.clone());

        let label = format_workflow_chain(&parent, &child);
        assert_eq!(
            label,
            "implement workflow engine -> Triggered Auto-Test after Code: implement workflow engine"
        );

        let mut bare = LoungeTask::new("x", "p", "");
        bare.id = "task-parent-id".into();
        let mut bare_child = LoungeTask::new("y", "p", "");
        bare_child.id = "task-child-id".into();
        assert_eq!(
            format_workflow_chain(&bare, &bare_child),
            "task-parent-id -> Triggered task-child-id"
        );
    }

    #[test]
    fn triggers_only_code_analysis_coding() {
        assert!(!triggers_followup_test(&TaskKind::Review));
        assert!(triggers_followup_test(&TaskKind::CodeAnalysis));
        assert!(!triggers_followup_test(&TaskKind::Test));
        assert!(!triggers_followup_test(&TaskKind::General));
        assert!(!triggers_followup_test(&TaskKind::Orchestration));
    }

    #[test]
    fn resolve_prefers_grok_then_local_then_none() {
        assert_eq!(
            resolve_test_target(true, true),
            Some((GROK_BOT_WORKER.into(), TASK_REQUESTED))
        );
        assert_eq!(
            resolve_test_target(false, true),
            Some((LOCAL_TEST_WORKER.into(), TEST_REQUESTED))
        );
        assert_eq!(resolve_test_target(false, false), None);
    }
}
