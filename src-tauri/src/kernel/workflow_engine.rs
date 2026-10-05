//! Multi-Agent Workflow — başarılı Coding sonrası otomatik Test görevi.

#![allow(deprecated)]

use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc;

use crate::db::ExperienceStore;
use crate::models::{
    is_kernel, LoungeTask, TaskKind, TaskStatus, KERNEL_AGENT, TASK_COMPLETED, TASK_REQUESTED,
    TEST_REQUESTED,
};
use crate::services::autodiscover::find_grok_bot;

/// Fleet / discovery worker id — Grok Bot.
pub const GROK_BOT_WORKER: &str = "grok_bot";
/// Yerel test worker — Worker Fleet paneliyle aynı id (`lounge-kernel`).
pub const LOCAL_TEST_WORKER: &str = "lounge-kernel";
/// Yerel Test Worker’ı açıkça tanımlamak için ortam değişkeni.
pub const LOCAL_TEST_WORKER_ENV: &str = "LOUNGE_LOCAL_TEST_WORKER";
const WORKFLOW_AGENT: &str = "workflow_engine";

#[derive(Clone)]
pub struct WorkflowEngine {
    nats_url: String,
    /// Testlerde Grok varlık override; `None` → canlı discovery.
    fleet_has_grok: Option<bool>,
    /// Testlerde yerel Test Worker override; `None` → env / config.
    fleet_has_local_test: Option<bool>,
    /// Kernel içi giriş — NATS spoof’una kapalı (Dispatcher internal ingress).
    trusted_ingress: Option<mpsc::Sender<LoungeTask>>,
    /// A2A görev kaynağı — completed doğrulama DB üzerinden (NATS payload’a güvenilmez).
    store: Option<ExperienceStore>,
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
            trusted_ingress: None,
            store: None,
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

    /// Dispatcher internal mpsc — follow-up NATS’a çıkmadan içeriden işlenir.
    pub fn with_trusted_ingress(mut self, tx: mpsc::Sender<LoungeTask>) -> Self {
        self.trusted_ingress = Some(tx);
        self
    }

    /// a2a_tasks doğrulama deposu — üretimde zorunlu.
    pub fn with_store(mut self, store: ExperienceStore) -> Self {
        self.store = Some(store);
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
        let nc = tokio::task::spawn_blocking(move || crate::services::nats_connect(&url))
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

    /// NATS `lounge.task.completed` — yalnız `id` okunur; görev DB’den doğrulanır.
    /// Lifecycle ile yarış için Completed görünene kadar bounded retry.
    async fn handle_completed(&self, nc: &nats::Connection, data: &[u8]) -> Result<()> {
        let Some(dispatch) = self.resolve_followup_from_bus_async(data).await? else {
            return Ok(());
        };
        let followup_id = dispatch.task.id.clone();
        let chain = dispatch.chain_label.clone();
        let parent_id = dispatch.task.parent_task_id.clone().unwrap_or_default();
        if let Some(tx) = &self.trusted_ingress {
            tx.send(dispatch.task)
                .await
                .context("workflow trusted ingress send")?;
        } else {
            log::warn!(
                "workflow_engine trusted_ingress yok; follow-up NATS’a düşer (unverified damga)"
            );
            publish_task(nc, dispatch.subject, &dispatch.task).await?;
        }
        log::info!("workflow_engine Test tetiklendi: {parent_id} → {followup_id} ({chain})");
        Ok(())
    }

    /// Lifecycle yazımı ile yarış: status henüz Completed değilse kısa retry.
    pub async fn resolve_followup_from_bus_async(
        &self,
        data: &[u8],
    ) -> Result<Option<TestDispatch>> {
        const ATTEMPTS: u32 = 5;
        const DELAY: Duration = Duration::from_millis(200);
        let mut last_not_ready: Option<String> = None;
        for attempt in 0..ATTEMPTS {
            match self.resolve_followup_from_bus(data) {
                Ok(v) => return Ok(v),
                Err(err) => {
                    let msg = err.to_string();
                    if msg.contains("Completed değil") {
                        last_not_ready = Some(msg);
                        if attempt + 1 < ATTEMPTS {
                            tokio::time::sleep(DELAY).await;
                            continue;
                        }
                        break;
                    }
                    return Err(err);
                }
            }
        }
        anyhow::bail!(
            "{}",
            last_not_ready.unwrap_or_else(|| "follow-up: Completed beklenirken zaman aşımı".into())
        )
    }

    /// Bus payload → DB doğrulama → follow-up planı.
    ///
    /// Payload’daki `kind` / `summary` / `repo_path` / `ast_refs` / `priority` /
    /// `source_verified` **yok sayılır**; yalnız `id` kullanılır.
    pub fn resolve_followup_from_bus(&self, data: &[u8]) -> Result<Option<TestDispatch>> {
        let value: serde_json::Value =
            serde_json::from_slice(data).context("lounge.task.completed payload json")?;
        let id = value
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("completed payload'da id yok"))?;

        let Some(store) = &self.store else {
            anyhow::bail!("workflow_engine store yok — completed doğrulanamaz (fail-closed)");
        };
        let parent = store
            .load_a2a_task(id)?
            .ok_or_else(|| anyhow::anyhow!("a2a_tasks'ta completed id yok: {id}"))?;

        if parent.status != TaskStatus::Completed {
            anyhow::bail!(
                "follow-up reddedildi: id={id} status={} (Completed değil)",
                parent.status.as_str()
            );
        }
        if !triggers_followup_test(&parent.kind) {
            return Ok(None);
        }

        Ok(self.plan_test_followup(&parent))
    }

    /// Başarılı Coding (`CodeAnalysis`) + tanımlı Test Worker → Test planı; aksi halde `None`.
    ///
    /// Çağıran DB kaydını vermeli — NATS payload alanlarına güvenilmez.
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
        task.root_id = Some(completed.effective_root_id().to_string());
        task.hop_count = completed.hop_count.saturating_add(1);
        task.session_id = completed.session_id.clone();
        // Aynı parent için tekrar completed → tek Test (idempotency replay).
        task.idempotency_key = Some(format!("workflow-test:{}", completed.id));
        // Parent doğrulanmamışsa follow-up da unverified → SourceUnverified kapısı.
        // (Onaylanmış unverified Coding parent → ikinci SourceUnverified: bilinçli güvenlik kararı.)
        task.source_verified = completed.source_verified;
        // summary/repo/ast/priority yalnız DB parent’tan.
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
    use crate::db::ExperienceStore;
    use crate::models::TaskKind;

    fn completed(kind: TaskKind, agent: &str) -> LoungeTask {
        let mut task = LoungeTask::new(agent, "agent-lounge-os", "implement workflow engine");
        task.kind = kind;
        task.target_agent = Some(agent.into());
        task.status = TaskStatus::Completed;
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
        assert!(
            dispatch.task.source_verified,
            "verified parent → verified child"
        );
        let expected_key = format!("workflow-test:{}", parent.id);
        assert_eq!(
            dispatch.task.idempotency_key.as_deref(),
            Some(expected_key.as_str())
        );
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
    fn unverified_parent_produces_unverified_followup() {
        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(true)
            .with_local_test_in_fleet(false);
        let mut parent = completed(TaskKind::CodeAnalysis, "claude");
        parent.source_verified = false;
        let dispatch = engine.plan_test_followup(&parent).expect("follow-up");
        assert!(
            !dispatch.task.source_verified,
            "unverified parent → follow-up SourceUnverified kapısına düşmeli"
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
    fn spoofed_completed_payload_ignored_fields_use_db() {
        let store = ExperienceStore::memory().unwrap();
        let mut parent = LoungeTask::new("claude", "agent-lounge-os", "real-db-summary");
        parent.kind = TaskKind::CodeAnalysis;
        parent.repo_path = Some("/real/repo".into());
        parent.ast_refs = vec!["RealSym".into()];
        parent.source_verified = true;
        store.admit_a2a_task(&mut parent, 10).unwrap();
        store
            .set_a2a_task_status(&parent.id, TaskStatus::Completed)
            .unwrap();

        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(true)
            .with_local_test_in_fleet(false)
            .with_store(store);

        // Sahte payload: kind/summary/repo/ast spoof + source_verified false — yalnız id geçerli.
        let spoof = serde_json::json!({
            "id": parent.id,
            "type": "task",
            "kind": "test",
            "summary": "ATTACKER SUMMARY",
            "repo_path": "/evil",
            "ast_refs": ["Evil"],
            "priority": "critical",
            "source_verified": false,
            "source_agent": "workflow_engine",
            "project_id": "agent-lounge-os",
        });
        let dispatch = engine
            .resolve_followup_from_bus(spoof.to_string().as_bytes())
            .unwrap()
            .expect("DB CodeAnalysis Completed → follow-up");
        assert!(
            dispatch.task.summary.contains("real-db-summary"),
            "summary DB’den gelmeli, spoof değil: {}",
            dispatch.task.summary
        );
        assert_eq!(dispatch.task.repo_path.as_deref(), Some("/real/repo"));
        assert_eq!(dispatch.task.ast_refs, vec!["RealSym".to_string()]);
        assert!(
            dispatch.task.source_verified,
            "parent DB verified → child verified"
        );
        assert!(!dispatch.task.summary.contains("ATTACKER"));
    }

    #[test]
    fn spoofed_completed_unknown_id_rejected() {
        let store = ExperienceStore::memory().unwrap();
        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(true)
            .with_store(store);
        let spoof = serde_json::json!({
            "id": "not-in-db",
            "type": "task",
            "kind": "code_analysis",
            "summary": "fake",
            "source_agent": "x",
            "project_id": "p",
        });
        let err = engine
            .resolve_followup_from_bus(spoof.to_string().as_bytes())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("a2a_tasks") || err.contains("yok"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn spoofed_completed_wrong_status_rejected() {
        let store = ExperienceStore::memory().unwrap();
        let mut parent = LoungeTask::new("claude", "p", "still-executing");
        parent.kind = TaskKind::CodeAnalysis;
        store.admit_a2a_task(&mut parent, 10).unwrap();
        store
            .set_a2a_task_status(&parent.id, TaskStatus::Executing)
            .unwrap();

        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(true)
            .with_store(store);
        let spoof = serde_json::json!({
            "id": parent.id,
            "type": "task",
            "kind": "code_analysis",
            "summary": "claim completed",
            "source_agent": "x",
            "project_id": "p",
        });
        let err = engine
            .resolve_followup_from_bus(spoof.to_string().as_bytes())
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("Completed değil") || err.contains("follow-up reddedildi"),
            "unexpected: {err}"
        );
    }

    #[test]
    fn triggers_only_code_analysis_coding() {
        assert!(triggers_followup_test(&TaskKind::CodeAnalysis));
        assert!(!triggers_followup_test(&TaskKind::Review));
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

    #[test]
    fn chain_label_formats_task_a_triggered_task_b() {
        let parent = completed(TaskKind::CodeAnalysis, "claude");
        let mut child =
            LoungeTask::new("workflow_engine", "agent-lounge-os", "Auto-Test after Code");
        child.kind = TaskKind::Test;
        assert_eq!(
            format_workflow_chain(&parent, &child),
            "implement workflow engine -> Triggered Auto-Test after Code"
        );

        // Yalın id etiketi (boş summary) + parent_task_id kurulumu.
        let mut bare_parent = LoungeTask::new("claude", "p", "");
        bare_parent.id = "task-parent-id".into();
        bare_parent.summary = String::new();
        bare_parent.kind = TaskKind::CodeAnalysis;
        let mut bare_child = LoungeTask::new("workflow_engine", "p", "");
        bare_child.id = "task-child-id".into();
        bare_child.summary = String::new();
        bare_child.kind = TaskKind::Test;
        bare_child.parent_task_id = Some(bare_parent.id.clone());
        assert_eq!(
            format_workflow_chain(&bare_parent, &bare_child),
            "task-parent-id -> Triggered task-child-id"
        );
        assert_eq!(bare_child.parent_task_id.as_deref(), Some("task-parent-id"));
    }

    #[test]
    fn followup_idempotency_key_replays_on_duplicate_completed() {
        let store = ExperienceStore::memory().unwrap();
        let mut parent = LoungeTask::new("claude", "p", "coding done");
        parent.kind = TaskKind::CodeAnalysis;
        parent.source_verified = true;
        store.admit_a2a_task(&mut parent, 10).unwrap();
        store
            .set_a2a_task_status(&parent.id, TaskStatus::Completed)
            .unwrap();

        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(true)
            .with_store(store.clone());
        let d1 = engine.plan_test_followup(&parent).expect("first follow-up");
        let expected_key = format!("workflow-test:{}", parent.id);
        assert_eq!(
            d1.task.idempotency_key.as_deref(),
            Some(expected_key.as_str())
        );
        let mut first = d1.task;
        assert!(matches!(
            store.admit_a2a_task(&mut first, 10).unwrap(),
            crate::db::AdmitOutcome::Accepted(_)
        ));

        let d2 = engine.plan_test_followup(&parent).expect("second plan");
        let mut second = d2.task;
        match store.admit_a2a_task(&mut second, 10).unwrap() {
            crate::db::AdmitOutcome::Replay {
                existing_task_id, ..
            } => assert_eq!(existing_task_id, first.id),
            other => panic!("Replay beklenir: {other:?}"),
        }
    }

    #[tokio::test]
    async fn delegated_completion_race_retries_until_db_completed() {
        let store = ExperienceStore::memory().unwrap();
        let mut parent = LoungeTask::new("claude", "p", "delegated coding");
        parent.kind = TaskKind::CodeAnalysis;
        parent.source_verified = true;
        store.admit_a2a_task(&mut parent, 10).unwrap();
        // Worker henüz lifecycle yazmadı — DISPATCHED.
        store
            .set_a2a_task_status(&parent.id, TaskStatus::Dispatched)
            .unwrap();

        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(true)
            .with_store(store.clone());
        let payload = serde_json::json!({ "id": parent.id, "type": "task" });
        let bytes = payload.to_string().into_bytes();

        let store_late = store.clone();
        let parent_id = parent.id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(350)).await;
            store_late
                .set_a2a_task_status(&parent_id, TaskStatus::Completed)
                .unwrap();
        });

        let dispatch = engine
            .resolve_followup_from_bus_async(&bytes)
            .await
            .unwrap()
            .expect("retry sonrası follow-up");
        assert_eq!(dispatch.task.kind, TaskKind::Test);
        assert_eq!(
            dispatch.task.parent_task_id.as_deref(),
            Some(parent.id.as_str())
        );
    }

    #[test]
    fn failed_coding_is_out_of_band_completed_only() {
        // Başarısız görevler lounge.task.failed üzerindedir; plan_test_followup
        // Completed + CodeAnalysis DB kaydı ister.
        let store = ExperienceStore::memory().unwrap();
        let mut parent = LoungeTask::new("claude", "p", "failed coding");
        parent.kind = TaskKind::CodeAnalysis;
        store.admit_a2a_task(&mut parent, 10).unwrap();
        store
            .set_a2a_task_status(&parent.id, TaskStatus::Failed)
            .unwrap();
        let engine = WorkflowEngine::new("nats://127.0.0.1:4222")
            .with_grok_in_fleet(true)
            .with_store(store);
        let payload = serde_json::json!({ "id": parent.id, "type": "task" });
        let err = engine
            .resolve_followup_from_bus(payload.to_string().as_bytes())
            .unwrap_err()
            .to_string();
        assert!(err.contains("Completed değil") || err.contains("reddedildi"));
    }
}
