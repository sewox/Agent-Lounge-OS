#![allow(deprecated)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};

use crate::db::{lexical_embedding, ExperienceStore};
use crate::infra::{probe_quotas, quota_exhausted_for};
use crate::models::{
    decide_route, default_ollama_model, AnalysisDecision, ApprovalRequest, ExperienceContext,
    ExperienceOutcome, ExperienceRecord, LoungeExperience, LoungeTask, RouteIntent, RoutingVote,
    TaskAssignment, EXPERIENCE_REPORTED, KERNEL_AGENT, TASK_ASSIGNED, TASK_COMPLETED, TASK_FAILED,
    TASK_REQUESTED,
};
use crate::services::{chat_json, embed_model, embed_text, MemoryBridge};

const NATS_MONITOR: &str = "http://127.0.0.1:8222";
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

const ANALYZE_SYSTEM: &str = r#"Sen Agent Lounge OS görev dağıtıcısısın.
Gelen İş Emri'ni ve varsa önceki tecrübe context'ini analiz et.
YALNIZCA şu JSON şemasında yanıt ver:
{
  "intent": "code_analysis | dispatch | acknowledge",
  "is_code_analysis": true,
  "reason": "kısa gerekçe",
  "adr_summary": "alınan mimari/iş kararı özeti",
  "outcome": "success | failure | partial",
  "target_agent": null,
  "repo_path": null
}
Kod, AST, indeks, call graph, repo taraması veya memory_bridge gerektiren işler code_analysis'tir.
Önceki tecrübelerle çelişme; uygun olanı adr_summary içinde an.
Ham kod kopyalama. Yalnızca JSON üret."#;

#[derive(Clone)]
pub struct Dispatcher {
    nats_url: String,
    ollama_endpoint: String,
    model: Arc<RwLock<String>>,
    memory: MemoryBridge,
    store: ExperienceStore,
    workspace_root: PathBuf,
    stub_decision: Option<AnalysisDecision>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<RoutingVote>>>>,
    app: Arc<StdMutex<Option<AppHandle>>>,
}

impl Dispatcher {
    pub fn new(
        nats_url: impl Into<String>,
        ollama_endpoint: impl Into<String>,
        model: Arc<RwLock<String>>,
        memory: MemoryBridge,
        store: ExperienceStore,
        workspace_root: PathBuf,
    ) -> Self {
        Self {
            nats_url: nats_url.into(),
            ollama_endpoint: ollama_endpoint.into(),
            model,
            memory,
            store,
            workspace_root,
            stub_decision: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            app: Arc::new(StdMutex::new(None)),
        }
    }

    pub fn with_stub_decision(mut self, decision: AnalysisDecision) -> Self {
        self.stub_decision = Some(decision);
        self
    }

    pub fn attach_app(&self, app: AppHandle) {
        *self.app.lock().expect("dispatcher app lock") = Some(app);
    }

    pub async fn routing_policy(&self) -> Result<crate::models::RoutingPolicy> {
        self.store.get_routing_policy().await
    }

    pub async fn set_routing_policy(
        &self,
        policy: crate::models::RoutingPolicy,
    ) -> Result<crate::models::RoutingPolicy> {
        let mut next = policy;
        next.require_user_approval = true;
        self.store.set_routing_policy(&next).await?;
        Ok(next)
    }

    pub async fn resolve_vote(&self, task_id: String, vote: RoutingVote) -> Result<()> {
        let sender = self.pending.lock().await.remove(&task_id);
        match sender {
            Some(tx) => {
                tx.send(vote)
                    .map_err(|_| anyhow::anyhow!("onay alıcısı kapanmış"))?;
                Ok(())
            }
            None => anyhow::bail!("bekleyen routing onayı yok: {task_id}"),
        }
    }

    pub async fn selected_model(&self) -> String {
        self.model.read().await.clone()
    }

    pub async fn set_model(&self, model: impl Into<String>) {
        *self.model.write().await = model.into();
    }

    pub async fn listen(&self) -> Result<()> {
        loop {
            match self.listen_once().await {
                Ok(()) => {
                    log::warn!("NATS dinleyici kapandı, yeniden bağlanılıyor");
                }
                Err(err) => {
                    log::error!("dispatcher NATS: {err}");
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn listen_once(&self) -> Result<()> {
        let url = self.nats_url.clone();
        let nc = tokio::task::spawn_blocking(move || nats::connect(&url))
            .await
            .context("NATS connect join")?
            .context("NATS bağlantısı kurulamadı")?;

        let (tx, mut rx) = mpsc::channel::<nats::Message>(64);
        let listener = nc.clone();
        tokio::task::spawn_blocking(move || {
            let sub = listener.subscribe(TASK_REQUESTED)?;
            for msg in sub.messages() {
                if tx.blocking_send(msg).is_err() {
                    break;
                }
            }
            Ok::<_, std::io::Error>(())
        });

        log::info!("dispatcher dinliyor: {} ({TASK_REQUESTED})", self.nats_url);

        while let Some(msg) = rx.recv().await {
            if let Err(err) = self.handle_nats_message(&nc, msg).await {
                log::error!("iş emri işlenemedi: {err}");
            }
        }
        Ok(())
    }

    async fn handle_nats_message(&self, nc: &nats::Connection, msg: nats::Message) -> Result<()> {
        let task: LoungeTask =
            serde_json::from_slice(&msg.data).context("İş Emri shared task şemasına uymuyor")?;
        let context = self.recall_context(&task).await.unwrap_or_else(|err| {
            log::warn!("tecrübe araması atlandı: {err}");
            ExperienceContext::default()
        });

        match self.execute_task(task.clone(), context, Some(nc)).await {
            Ok(experience) => {
                publish_json(nc, TASK_COMPLETED, &task).await?;
                publish_json(nc, EXPERIENCE_REPORTED, &experience).await?;
            }
            Err(err) => {
                log::error!("görev başarısız {}: {err}", task.id);
                publish_json(nc, TASK_FAILED, &task).await?;
            }
        }
        Ok(())
    }

    pub async fn handle_task(&self, task: LoungeTask) -> Result<LoungeExperience> {
        let context = self.recall_context(&task).await.unwrap_or_default();
        self.execute_task(task, context, None).await
    }

    /// Yeni iş emri için benzer konuları semantik arar (Tokio).
    pub async fn recall_context(&self, task: &LoungeTask) -> Result<ExperienceContext> {
        let query = task.summary.clone();
        let lexical = lexical_embedding(&query);
        let mut hits = Vec::new();
        match embed_text(&self.ollama_endpoint, &embed_model(), &query).await {
            Ok(vector) if !vector.is_empty() => {
                hits = self
                    .store
                    .similar_to(task.project_id.clone(), vector, Some(3))
                    .await
                    .context("experiences semantik sorgu")?;
            }
            Ok(_) | Err(_) => {}
        }
        if hits.is_empty() {
            hits = self
                .store
                .similar_to(task.project_id.clone(), lexical, Some(3))
                .await
                .context("experiences lexical yedek sorgu")?;
        }
        Ok(ExperienceContext::from_hits(hits))
    }

    async fn embed_topic(&self, text: &str) -> Vec<f32> {
        match embed_text(&self.ollama_endpoint, &embed_model(), text).await {
            Ok(vector) if !vector.is_empty() => vector,
            Ok(_) => lexical_embedding(text),
            Err(err) => {
                log::debug!("Ollama embedding yok, lexical vektör: {err}");
                lexical_embedding(text)
            }
        }
    }

    async fn execute_task(
        &self,
        mut task: LoungeTask,
        context: ExperienceContext,
        nc: Option<&nats::Connection>,
    ) -> Result<LoungeExperience> {
        if task.msg_type != "task" {
            anyhow::bail!("beklenen type=task, gelen={}", task.msg_type);
        }

        let model = task
            .resolved_model(&self.selected_model().await)
            .to_string();
        if task.target_agent.is_none() {
            task.target_agent = Some(KERNEL_AGENT.into());
        }

        let decision = self.analyze_work_order(&task, &model, &context).await?;
        if let Some(agent) = decision.target_agent.clone() {
            task.target_agent = Some(agent);
        }
        self.apply_routing_policy(&mut task, &decision).await?;

        if let Some(nc) = nc {
            let assignment = TaskAssignment {
                task: task.clone(),
                context: context.clone(),
            };
            publish_json(nc, TASK_ASSIGNED, &assignment).await?;
        }

        let mut tags = vec![
            format!("model:{model}"),
            format!("intent:{}", decision.intent),
        ];
        for hit in &context.experiences {
            tags.push(format!("recalled:{}", hit.id));
        }
        let mut outcome = decision
            .outcome
            .clone()
            .unwrap_or(ExperienceOutcome::Success);
        let mut adr = if decision.adr_summary.trim().is_empty() {
            decision.reason.clone()
        } else {
            decision.adr_summary.clone()
        };
        if !context.is_empty() {
            adr = format!("{adr}\n{}", context.prompt_block());
        }

        if decision.needs_code_analysis(&task) {
            tags.push("memory_bridge".into());
            let repo = resolve_repo_path(&task, &decision, &self.workspace_root);
            match self.memory.index_workspace(&repo).await {
                Ok(graph) => {
                    let snapshot = graph.snapshot();
                    tags.push(format!("nodes:{}", snapshot.nodes));
                    tags.push(format!("edges:{}", snapshot.edges));
                    tags.push(format!("dead:{}", snapshot.dead));
                    adr = format!(
                        "{adr}\ncodebase-memory-mcp index: project={} nodes={} edges={} dead={}",
                        snapshot.project, snapshot.nodes, snapshot.edges, snapshot.dead
                    );
                    if let Err(err) = self.store.save_project_index(graph).await {
                        log::warn!("project_index yazılamadı: {err}");
                    }
                }
                Err(err) => {
                    outcome = ExperienceOutcome::Partial;
                    adr = format!("{adr}\nmemory_bridge hata: {err}");
                    log::error!("memory_bridge tetiklenemedi: {err}");
                }
            }
        }

        let mut record =
            ExperienceRecord::from_task(&task, adr.clone(), adr.clone(), outcome.clone(), tags);
        record.embedding = self.embed_topic(&record.search_text()).await;
        self.store
            .insert_record(record.clone())
            .await
            .context("tecrübe SQLite'a yazılamadı")?;
        Ok(record.to_lounge())
    }

    async fn analyze_work_order(
        &self,
        task: &LoungeTask,
        model: &str,
        context: &ExperienceContext,
    ) -> Result<AnalysisDecision> {
        if let Some(decision) = &self.stub_decision {
            return Ok(decision.clone());
        }

        let mut user = serde_json::to_string(task)?;
        user.push_str(&context.prompt_block());
        match chat_json(&self.ollama_endpoint, model, ANALYZE_SYSTEM, &user).await {
            Ok(value) => serde_json::from_value(value).context("analiz kararı çözülemedi"),
            Err(err) => {
                log::warn!("Ollama analiz hatası, kural tabanlı yedek: {err}");
                Ok(fallback_decision(task))
            }
        }
    }

    /// Kullanıcı tercihi + kota; harici ajan geçişi onaysız olamaz.
    /// Testlerde `stub_decision` varken onay beklenmez.
    async fn apply_routing_policy(
        &self,
        task: &mut LoungeTask,
        decision: &AnalysisDecision,
    ) -> Result<()> {
        if self.stub_decision.is_some() {
            return Ok(());
        }

        let policy = self.store.get_routing_policy().await.unwrap_or_default();
        let quotas = probe_quotas(&self.ollama_endpoint, NATS_MONITOR, &self.memory).await;
        let to = decision
            .target_agent
            .as_deref()
            .or(task.target_agent.as_deref())
            .unwrap_or(KERNEL_AGENT);
        let exhausted = quota_exhausted_for(&quotas, to);

        match decide_route(&policy, task, &task.source_agent, to, exhausted) {
            RouteIntent::Allow { agent } => {
                task.target_agent = Some(agent);
                Ok(())
            }
            RouteIntent::Stop { reason } => anyhow::bail!(reason),
            RouteIntent::NeedApproval { request } => {
                self.await_approval(
                    task,
                    request,
                    &policy.local_fallback_agent,
                    &policy.local_fallback_model,
                )
                .await
            }
        }
    }

    async fn await_approval(
        &self,
        task: &mut LoungeTask,
        request: ApprovalRequest,
        local_agent: &str,
        local_model: &str,
    ) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        let task_id = request.task_id.clone();
        self.pending.lock().await.insert(task_id.clone(), tx);
        if let Some(app) = self.app.lock().expect("dispatcher app lock").clone() {
            let _ = app.emit("lounge://routing-approval", &request);
        }

        let vote = match tokio::time::timeout(APPROVAL_TIMEOUT, rx).await {
            Ok(Ok(vote)) => vote,
            Ok(Err(_)) => anyhow::bail!("routing onay kanalı kapandı"),
            Err(_) => {
                self.pending.lock().await.remove(&task_id);
                anyhow::bail!("routing onayı zaman aşımı")
            }
        };

        match vote {
            RoutingVote::Approve => {
                task.target_agent = Some(request.to_agent);
                Ok(())
            }
            RoutingVote::ApproveLocal => {
                task.target_agent = Some(local_agent.into());
                task.model = Some(local_model.into());
                Ok(())
            }
            RoutingVote::Deny => anyhow::bail!("kullanıcı ajan geçişini reddetti"),
        }
    }
}

fn fallback_decision(task: &LoungeTask) -> AnalysisDecision {
    let is_code = matches!(task.kind, crate::models::TaskKind::CodeAnalysis)
        || task.summary.to_ascii_lowercase().contains("analiz")
        || task.summary.to_ascii_lowercase().contains("index")
        || task.summary.to_ascii_lowercase().contains("ast");
    AnalysisDecision {
        intent: if is_code {
            "code_analysis".into()
        } else {
            "acknowledge".into()
        },
        is_code_analysis: is_code,
        reason: "Ollama yok/yanıt vermedi; İş Emri alanlarından çıkarıldı".into(),
        adr_summary: task.summary.clone(),
        outcome: Some(ExperienceOutcome::Partial),
        target_agent: Some(KERNEL_AGENT.into()),
        repo_path: task.repo_path.clone(),
    }
}

fn resolve_repo_path(
    task: &LoungeTask,
    decision: &AnalysisDecision,
    workspace_root: &Path,
) -> PathBuf {
    decision
        .repo_path
        .as_deref()
        .or(task.repo_path.as_deref())
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| {
            let candidate = PathBuf::from(&task.project_id);
            candidate.exists().then_some(candidate)
        })
        .unwrap_or_else(|| workspace_root.to_path_buf())
}

async fn publish_json<T: serde::Serialize>(
    nc: &nats::Connection,
    subject: &str,
    payload: &T,
) -> Result<()> {
    let bytes = serde_json::to_vec(payload)?;
    let nc = nc.clone();
    let subject = subject.to_string();
    let subject_for_err = subject.clone();
    tokio::task::spawn_blocking(move || nc.publish(&subject, bytes))
        .await
        .context("NATS publish join")?
        .with_context(|| format!("NATS publish başarısız: {subject_for_err}"))?;
    Ok(())
}

pub fn default_model_lock() -> Arc<RwLock<String>> {
    Arc::new(RwLock::new(default_ollama_model()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ExperienceStore;
    use crate::models::{ExperienceRecord, TaskKind, TASK_ASSIGNED};
    use crate::services::parse_llm_json;

    fn dispatcher(decision: AnalysisDecision) -> Dispatcher {
        Dispatcher::new(
            "nats://127.0.0.1:4222",
            crate::services::lounge_ollama_endpoint(),
            default_model_lock(),
            MemoryBridge::from_binary("/tmp/missing-codebase-memory-mcp"),
            ExperienceStore::memory().unwrap(),
            PathBuf::from("/tmp"),
        )
        .with_stub_decision(decision)
    }

    #[test]
    fn strips_fenced_llm_json() {
        let value = parse_llm_json(
            "```json\n{\"intent\":\"code_analysis\",\"is_code_analysis\":true}\n```",
        )
        .unwrap();
        assert_eq!(value["intent"], "code_analysis");
    }

    #[tokio::test]
    async fn code_analysis_writes_experience_even_if_bridge_missing() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "code_analysis".into(),
            is_code_analysis: true,
            reason: "repo taranacak".into(),
            adr_summary: "C-binary tetiklenmeli".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });
        let mut task =
            LoungeTask::new("cursor", "agent-lounge-os", "kernel dispatcher'ı analiz et");
        task.kind = TaskKind::CodeAnalysis;
        let experience = dispatcher.handle_task(task.clone()).await.unwrap();
        assert_eq!(experience.msg_type, "experience");
        assert_eq!(
            experience.related_task_id.as_deref(),
            Some(task.id.as_str())
        );
        assert_eq!(experience.outcome, ExperienceOutcome::Partial);
        assert!(experience.tags.iter().any(|tag| tag == "memory_bridge"));
        assert!(dispatcher
            .store
            .get(experience.id.clone())
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn recalls_similar_experience_as_agent_context() {
        let store = ExperienceStore::memory().unwrap();
        let prior = LoungeTask::new(
            "cursor",
            "agent-lounge-os",
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

        let dispatcher = Dispatcher::new(
            "nats://127.0.0.1:4222",
            crate::services::lounge_ollama_endpoint(),
            default_model_lock(),
            MemoryBridge::from_binary("/tmp/missing-codebase-memory-mcp"),
            store,
            PathBuf::from("/tmp"),
        )
        .with_stub_decision(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "önceki tecrübe var".into(),
            adr_summary: "context ile devam".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });

        let task = LoungeTask::new(
            "grok",
            "agent-lounge-os",
            "dispatcher NATS mesajlarını dinle",
        );
        let context = dispatcher.recall_context(&task).await.unwrap();
        assert!(
            !context.is_empty(),
            "benzer NATS/dispatcher tecrübesi context olmalı"
        );
        assert!(context.experiences[0]
            .topic
            .to_ascii_lowercase()
            .contains("nats"));

        let experience = dispatcher.handle_task(task).await.unwrap();
        assert!(experience
            .tags
            .iter()
            .any(|tag| tag.starts_with("recalled:")));
        assert!(experience.adr_summary.contains("Önceki tecrübeler"));
    }

    #[tokio::test]
    async fn general_task_skips_memory_bridge() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "bilgi notu".into(),
            adr_summary: "ajan ataması gerekmiyor".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });
        let task = LoungeTask::new("grok", "agent-lounge-os", "status nedir?");
        let experience = dispatcher.handle_task(task).await.unwrap();
        assert_eq!(experience.outcome, ExperienceOutcome::Success);
        assert!(!experience.tags.iter().any(|tag| tag == "memory_bridge"));
    }

    #[test]
    fn assigned_subject_is_part_of_shared_catalog() {
        assert_eq!(TASK_ASSIGNED, "lounge.task.assigned");
    }
}
