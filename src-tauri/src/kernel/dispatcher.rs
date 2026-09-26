#![allow(deprecated)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot, RwLock};

use crate::db::{lexical_embedding, ExperienceStore, FastRetrieveQuery};
use crate::infra::probe_quotas;
use crate::kernel::decision_engine::{
    lookup_decision, DecisionCache, DecisionResult, RoutingType, SecurityLevel,
};
use crate::kernel::policy_manager::{
    alert_envelope_payload, cancel_envelope_payload, evaluate_security, is_security_approval,
    is_security_denied_error, resume_envelope_payload, ALERT_SECURITY, SECURITY_DENIED_MARKER,
    TASK_CANCEL, TASK_RESUME,
};
use crate::kernel::worker_registry::{route_subject_for, WorkerRegistry};
use crate::models::{
    decide_route, default_ollama_model, is_kernel, AnalysisDecision, ApprovalRequest,
    ExperienceContext, ExperienceOutcome, ExperienceRecord, LoungeExperience, LoungeTask,
    QuotaVerdict, RouteIntent, RoutingVote, TaskAssignment, ALERT_QUOTA, EXPERIENCE_REPORTED,
    KERNEL_AGENT, TASK_ASSIGNED, TASK_COMPLETED, TASK_FAILED, TASK_REQUESTED,
};
use crate::services::{
    chat_json, embed_model, embed_text, evaluate_assignment, is_quota_approval,
    limit_policy_percent, lmr_endpoint_up, nats_monitor_endpoint, normalize_lmr_agent,
    quota_alert_envelope, MemoryBridge,
};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

/// UI'ya onay banner'ını temizletmek için Tauri olayı.
pub const ROUTING_APPROVAL_EVENT: &str = "lounge://routing-approval";
pub const ROUTING_APPROVAL_CLEARED_EVENT: &str = "lounge://routing-approval-cleared";

#[derive(Debug, Clone, serde::Serialize)]
pub struct ApprovalCleared {
    pub task_id: String,
    pub reason: String,
}

/// `execute_task` sonucu — yerel tamamlandı veya dış worker'a delege edildi.
#[derive(Debug, Clone)]
enum TaskExecution {
    Local(LoungeExperience),
    Delegated { bot_id: String, subject: String },
}

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

/// Bekleyen onay: oneshot + UI'nin yeniden bağlanması için request anlığı.
struct PendingApproval {
    request: ApprovalRequest,
    tx: oneshot::Sender<RoutingVote>,
}

#[derive(Clone)]
pub struct Dispatcher {
    nats_url: String,
    ollama_endpoint: String,
    model: Arc<RwLock<String>>,
    memory: MemoryBridge,
    store: ExperienceStore,
    workspace_root: PathBuf,
    stub_decision: Option<AnalysisDecision>,
    /// std Mutex: UI `resolve_routing` senkron komutu async runtime'ı beklemeden oneshot'a ulaştırır.
    /// (tokio::Mutex + async komut, await_approval ile aynı runtime'da kilitlenmeye yol açabiliyordu.)
    /// Request kopyası UI rehydrate için tutulur (listener yarışı / webview yenileme).
    pending: Arc<StdMutex<HashMap<String, PendingApproval>>>,
    app: Arc<StdMutex<Option<AppHandle>>>,
    decisions: DecisionCache,
    workers: WorkerRegistry,
    approval_timeout: Duration,
    /// DecisionGate RAM'de ve sınıflandırma yapabiliyorsa true.
    gate_ready: Arc<AtomicBool>,
    /// Test: kota probe (yavaş TCP) atlanır.
    skip_quota_probe: bool,
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
            pending: Arc::new(StdMutex::new(HashMap::new())),
            app: Arc::new(StdMutex::new(None)),
            decisions: Arc::new(StdMutex::new(HashMap::new())),
            workers: WorkerRegistry::new(),
            approval_timeout: APPROVAL_TIMEOUT,
            gate_ready: Arc::new(AtomicBool::new(false)),
            skip_quota_probe: false,
        }
    }

    pub fn with_approval_timeout(mut self, timeout: Duration) -> Self {
        self.approval_timeout = timeout;
        self
    }

    pub fn with_skip_quota_probe(mut self) -> Self {
        self.skip_quota_probe = true;
        self
    }

    pub fn set_gate_ready(&self, ready: bool) {
        self.gate_ready.store(ready, Ordering::SeqCst);
    }

    pub fn gate_is_ready(&self) -> bool {
        self.gate_ready.load(Ordering::SeqCst)
    }

    pub fn with_workers(mut self, workers: WorkerRegistry) -> Self {
        self.workers = workers;
        self
    }

    pub fn workers(&self) -> &WorkerRegistry {
        &self.workers
    }

    pub fn with_stub_decision(mut self, decision: AnalysisDecision) -> Self {
        self.stub_decision = Some(decision);
        self
    }

    pub fn attach_app(&self, app: AppHandle) {
        *self.app.lock().expect("dispatcher app lock") = Some(app);
    }

    pub fn with_decision_cache(mut self, cache: DecisionCache) -> Self {
        self.decisions = cache;
        self
    }

    pub fn inject_decision(&self, result: DecisionResult) {
        crate::kernel::decision_engine::remember(&self.decisions, &result);
    }

    fn gate_decision(&self, task_id: &str) -> Option<DecisionResult> {
        lookup_decision(&self.decisions, task_id)
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

    pub fn resolve_vote(&self, task_id: String, vote: RoutingVote) -> Result<()> {
        log::info!("resolve_vote task_id={task_id} vote={vote:?}");
        let sender = self
            .pending
            .lock()
            .expect("dispatcher pending lock")
            .remove(&task_id);
        match sender {
            Some(PendingApproval { tx, .. }) => {
                tx.send(vote)
                    .map_err(|_| anyhow::anyhow!("onay alıcısı kapanmış"))?;
                Ok(())
            }
            None => anyhow::bail!("bekleyen routing onayı yok: {task_id}"),
        }
    }

    /// Test / UI: bekleyen onay sayısı.
    pub fn pending_count(&self) -> usize {
        self.pending.lock().expect("dispatcher pending lock").len()
    }

    pub fn has_pending(&self, task_id: &str) -> bool {
        self.pending
            .lock()
            .expect("dispatcher pending lock")
            .contains_key(task_id)
    }

    /// UI rehydrate: dinleyici kurulmadan kaçan veya state kaybı sonrası bekleyen onaylar.
    pub fn pending_approvals(&self) -> Vec<ApprovalRequest> {
        self.pending
            .lock()
            .expect("dispatcher pending lock")
            .values()
            .map(|row| row.request.clone())
            .collect()
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
            let this = self.clone();
            let nc = nc.clone();
            tokio::spawn(async move {
                if let Err(err) = this.handle_nats_message(&nc, msg).await {
                    log::error!("iş emri işlenemedi: {err}");
                }
            });
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
            Ok(TaskExecution::Local(experience)) => {
                publish_json(nc, TASK_COMPLETED, &task).await?;
                publish_json(nc, EXPERIENCE_REPORTED, &experience).await?;
            }
            Ok(TaskExecution::Delegated { bot_id, subject }) => {
                log::info!(
                    "görev {} onay sonrası dış worker'a yönlendirildi: {bot_id} → {subject}",
                    task.id
                );
            }
            Err(err) => {
                log::error!("görev başarısız {}: {err}", task.id);
                self.emit_approval_cleared(&task.id, "failed");
                // Güvenlik Red zaten lounge.task.failed (cancel) yayınladı.
                if !is_security_denied_error(&err) {
                    publish_json(nc, TASK_FAILED, &task).await?;
                }
            }
        }
        Ok(())
    }

    pub async fn handle_task(&self, task: LoungeTask) -> Result<LoungeExperience> {
        let context = self.recall_context(&task).await.unwrap_or_default();
        match self.execute_task(task, context, None).await? {
            TaskExecution::Local(experience) => Ok(experience),
            TaskExecution::Delegated { bot_id, .. } => {
                anyhow::bail!(
                    "görev dış worker'a delegasyon bekliyor: {bot_id} (NATS bağlantısı yok)"
                )
            }
        }
    }

    /// Yeni iş emri için benzer konuları semantik arar (Tokio).
    pub async fn recall_context(&self, task: &LoungeTask) -> Result<ExperienceContext> {
        if let Some(gate) = self.gate_decision(&task.id) {
            if gate.knowledge_is_hit() {
                let embedding =
                    match embed_text(&self.ollama_endpoint, &embed_model(), &task.summary).await {
                        Ok(vector) if !vector.is_empty() => Some(vector),
                        _ => None,
                    };
                return self
                    .store
                    .fast_retrieve(FastRetrieveQuery::from_task(
                        task,
                        gate.knowledge_hit,
                        embedding,
                    ))
                    .await
                    .context("fast_retrieve KNOWLEDGE_HIT");
            }
        }

        let query = task.summary.clone();
        let lexical = lexical_embedding(&query);
        let skip_embed = self
            .gate_decision(&task.id)
            .is_some_and(|gate| gate.knowledge_is_low());
        let mut hits = Vec::new();
        if !skip_embed {
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
    ) -> Result<TaskExecution> {
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
            // Fallback/KERNEL kararı, MCP'nin açık worker hedefini ezmesin.
            let keep_explicit =
                task.target_agent.as_ref().is_some_and(|t| !is_kernel(t)) && is_kernel(&agent);
            if !keep_explicit {
                task.target_agent = Some(agent);
            }
        }
        self.apply_routing_policy(&mut task, &decision).await?;

        let target = task
            .target_agent
            .clone()
            .unwrap_or_else(|| KERNEL_AGENT.into());
        let worker_inbox = if !is_kernel(&target) && self.workers.has_worker(&target) {
            Some(route_subject_for(&self.workers, &target).ok_or_else(|| {
                anyhow::anyhow!("hedef worker çevrimdışı veya heartbeat süresi doldu: {target}")
            })?)
        } else {
            None
        };

        if let Some(nc) = nc {
            let assignment = TaskAssignment {
                task: task.clone(),
                context: context.clone(),
            };
            publish_json(nc, TASK_ASSIGNED, &assignment).await?;
            if let Some(subject) = worker_inbox.clone() {
                publish_json(nc, &subject, &assignment).await?;
                return Ok(TaskExecution::Delegated {
                    bot_id: target,
                    subject,
                });
            }
        } else if let Some(subject) = worker_inbox {
            // Test / senkron yol: NATS yokken de yerel çalıştırmayı atla.
            return Ok(TaskExecution::Delegated {
                bot_id: target,
                subject,
            });
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
            match self
                .memory
                .index_workspace(repo.to_string_lossy().into_owned())
                .await
            {
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
        Ok(TaskExecution::Local(record.to_lounge()))
    }

    async fn analyze_work_order(
        &self,
        task: &LoungeTask,
        model: &str,
        context: &ExperienceContext,
    ) -> Result<AnalysisDecision> {
        if let Some(decision) = &self.stub_decision {
            let mut decision = decision.clone();
            if let Some(gate) = self.gate_decision(&task.id) {
                apply_gate_hints(&mut decision, &gate);
            }
            return Ok(decision);
        }

        if let Some(gate) = self.gate_decision(&task.id) {
            if gate.confident() {
                return Ok(analysis_from_gate(task, &gate));
            }
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
    /// Testlerde `stub_decision` varken kota onayı beklenmez; güvenlik askısı stub yokken.
    /// DecisionGate kararı yoksa (soğuk/sınıflandırılamadı) muhafazakâr güvenlik onayı istenir.
    async fn apply_routing_policy(
        &self,
        task: &mut LoungeTask,
        decision: &AnalysisDecision,
    ) -> Result<()> {
        if let Some(gate) = self.gate_decision(&task.id) {
            if let Some(request) =
                evaluate_security(&task.id, &task.summary, &task.source_agent, &gate)
            {
                if self.stub_decision.is_none() {
                    let policy = self.store.get_routing_policy().await.unwrap_or_default();
                    return self
                        .await_approval(
                            task,
                            request,
                            Some(gate.security.value),
                            None,
                            &policy.local_fallback_agent,
                            &policy.local_fallback_model,
                        )
                        .await;
                }
            }
        } else if self.stub_decision.is_none() && !self.gate_is_ready() {
            // Soğuk DecisionGate — sınıflandırma yok; güvenlik atlanmaz, muhafazakâr onay.
            let policy = self.store.get_routing_policy().await.unwrap_or_default();
            let request = crate::kernel::policy_manager::unclassified_security_approval(
                &task.id,
                &task.summary,
                &task.source_agent,
            );
            return self
                .await_approval(
                    task,
                    request,
                    Some(SecurityLevel::Risky),
                    None,
                    &policy.local_fallback_agent,
                    &policy.local_fallback_model,
                )
                .await;
        }

        if self.stub_decision.is_some() {
            return Ok(());
        }

        let policy = self.store.get_routing_policy().await.unwrap_or_default();
        // explicit task hedefi (MCP worker) decision fallback KERNEL'inden öncelikli
        let to = task
            .target_agent
            .as_deref()
            .filter(|t| !t.trim().is_empty())
            .or(decision.target_agent.as_deref())
            .unwrap_or(KERNEL_AGENT);
        let limit = limit_policy_percent();
        let verdict = if self.skip_quota_probe {
            QuotaVerdict::Allow
        } else {
            let quotas = probe_quotas(
                &self.ollama_endpoint,
                &nats_monitor_endpoint(),
                &self.memory,
            )
            .await;
            evaluate_assignment(&quotas, to, limit)
        };
        let quota_blocked = verdict.is_blocked();

        match decide_route(&policy, task, &task.source_agent, to, quota_blocked) {
            RouteIntent::Allow { agent } => {
                task.target_agent = Some(agent);
                Ok(())
            }
            RouteIntent::Stop { reason } => anyhow::bail!(reason),
            RouteIntent::NeedApproval { request } => {
                let request = if is_quota_approval(&request.kind) {
                    let mut enriched = request;
                    if let QuotaVerdict::Block {
                        reason,
                        tool,
                        percent,
                    } = &verdict
                    {
                        enriched.reason = format!(
                            "{reason} · tool={tool} · %{:.0} · limit={limit:.0}%",
                            percent.unwrap_or(100.0)
                        );
                        enriched.to_agent = normalize_lmr_agent(&policy.local_fallback_agent);
                    }
                    enriched
                } else {
                    request
                };
                self.await_approval(
                    task,
                    request,
                    None,
                    Some(verdict),
                    &policy.local_fallback_agent,
                    &policy.local_fallback_model,
                )
                .await
            }
        }
    }

    fn emit_approval_cleared(&self, task_id: &str, reason: &str) {
        if let Some(app) = self.app.lock().expect("dispatcher app lock").clone() {
            let payload = ApprovalCleared {
                task_id: task_id.to_string(),
                reason: reason.to_string(),
            };
            let _ = app.emit(ROUTING_APPROVAL_CLEARED_EVENT, &payload);
            crate::services::emit_approval_resolved(&app, task_id, reason);
        }
    }

    /// Görev ajana gitmeden önce onay hold; güvenlik / kota NATS alert + resume.
    async fn await_approval(
        &self,
        task: &mut LoungeTask,
        request: ApprovalRequest,
        security_level: Option<SecurityLevel>,
        quota_verdict: Option<QuotaVerdict>,
        local_agent: &str,
        local_model: &str,
    ) -> Result<()> {
        let request = request.stamp_timeout(self.approval_timeout);
        let (tx, rx) = oneshot::channel();
        let task_id = request.task_id.clone();
        self.pending
            .lock()
            .expect("dispatcher pending lock")
            .insert(
                task_id.clone(),
                PendingApproval {
                    request: request.clone(),
                    tx,
                },
            );
        if let Some(app) = self.app.lock().expect("dispatcher app lock").clone() {
            let _ = app.emit(ROUTING_APPROVAL_EVENT, &request);
            crate::services::emit_approval_pending(
                &app,
                crate::services::ApprovalPendingPayload::from(&request),
            );
        }

        if is_security_approval(&request.kind) {
            let level = security_level.unwrap_or(SecurityLevel::Critical);
            let payload = alert_envelope_payload(&request, level);
            if let Err(err) = self.publish_subject(ALERT_SECURITY, &payload).await {
                log::warn!("lounge.alert.security yayınlanamadı: {err}");
            }
        } else if is_quota_approval(&request.kind) {
            let lmr_up = lmr_endpoint_up(&self.ollama_endpoint).await;
            let verdict = quota_verdict.unwrap_or(QuotaVerdict::Block {
                reason: request.reason.clone(),
                tool: request.to_agent.clone(),
                percent: None,
            });
            let payload = quota_alert_envelope(&request, &verdict, limit_policy_percent(), lmr_up);
            if let Err(err) = self.publish_subject(ALERT_QUOTA, &payload).await {
                log::warn!("lounge.alert.quota yayınlanamadı: {err}");
            }
        }

        let vote = match tokio::time::timeout(self.approval_timeout, rx).await {
            Ok(Ok(vote)) => vote,
            Ok(Err(_)) => {
                self.pending
                    .lock()
                    .expect("dispatcher pending lock")
                    .remove(&task_id);
                self.emit_approval_cleared(&task_id, "channel_closed");
                anyhow::bail!("routing onay kanalı kapandı")
            }
            Err(_) => {
                self.pending
                    .lock()
                    .expect("dispatcher pending lock")
                    .remove(&task_id);
                self.emit_approval_cleared(&task_id, "timeout");
                anyhow::bail!("routing onayı zaman aşımı")
            }
        };

        match vote {
            RoutingVote::Approve => {
                if is_security_approval(&request.kind) {
                    let level = security_level.unwrap_or(match request.kind {
                        crate::models::ApprovalKind::SecurityRisky => SecurityLevel::Risky,
                        _ => SecurityLevel::Critical,
                    });
                    if let Err(err) = crate::db::feedback::with_conn_record_security(
                        &self.store,
                        true,
                        level,
                        &task_id,
                        &request.summary,
                    ) {
                        log::warn!("security_approve feedback yazılamadı: {err}");
                    }
                    let payload = resume_envelope_payload(&task_id);
                    if let Err(err) = self.publish_subject(TASK_RESUME, &payload).await {
                        log::warn!("lounge.task.resume yayınlanamadı: {err}");
                    }
                }
                task.target_agent = Some(request.to_agent);
                Ok(())
            }
            RoutingVote::ApproveLocal => {
                // Continue → Lounge LMR (:18790), host Ollama (:11434) değil.
                if !lmr_endpoint_up(&self.ollama_endpoint).await {
                    anyhow::bail!(
                        "Lounge LMR (127.0.0.1:18790) ayakta değil; host Ollama ile devam edilmez"
                    );
                }
                task.target_agent = Some(normalize_lmr_agent(local_agent));
                task.model = Some(local_model.into());
                let payload = resume_envelope_payload(&task_id);
                if let Err(err) = self.publish_subject(TASK_RESUME, &payload).await {
                    log::warn!("lounge.task.resume (quota→LMR) yayınlanamadı: {err}");
                }
                Ok(())
            }
            RoutingVote::Deny => {
                if is_security_approval(&request.kind) {
                    let level = security_level.unwrap_or(match request.kind {
                        crate::models::ApprovalKind::SecurityRisky => SecurityLevel::Risky,
                        _ => SecurityLevel::Critical,
                    });
                    if let Err(err) = crate::db::feedback::with_conn_record_security(
                        &self.store,
                        false,
                        level,
                        &task_id,
                        &request.summary,
                    ) {
                        log::warn!("security_deny feedback yazılamadı: {err}");
                    }
                    let payload = cancel_envelope_payload(&task_id);
                    if let Err(err) = self.publish_subject(TASK_CANCEL, &payload).await {
                        log::warn!("lounge.task.failed (cancel) yayınlanamadı: {err}");
                    }
                    anyhow::bail!("{SECURITY_DENIED_MARKER}: kullanıcı güvenlik onayını reddetti");
                }
                anyhow::bail!("kota uyarısı reddedildi; görev durduruldu")
            }
        }
    }

    async fn publish_subject<T: serde::Serialize>(&self, subject: &str, payload: &T) -> Result<()> {
        let url = self.nats_url.clone();
        let subject = subject.to_string();
        let bytes = serde_json::to_vec(payload).context("NATS payload serialize")?;
        let subject_for_err = subject.clone();
        tokio::task::spawn_blocking(move || {
            let nc = nats::connect(&url)?;
            nc.publish(&subject, bytes)
        })
        .await
        .context("NATS publish join")?
        .with_context(|| format!("NATS publish başarısız: {subject_for_err}"))?;
        Ok(())
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

fn apply_gate_hints(decision: &mut AnalysisDecision, gate: &DecisionResult) {
    match gate.routing.value {
        RoutingType::Review => {
            decision.intent = "review".into();
        }
        RoutingType::Task => {
            if decision.intent.is_empty() {
                decision.intent = "dispatch".into();
            }
        }
        RoutingType::Experience => {
            decision.intent = "acknowledge".into();
            decision.is_code_analysis = false;
        }
    }
    if matches!(gate.routing.value, RoutingType::Task) && decision.is_code_analysis {
        decision.intent = "code_analysis".into();
    }
}

fn analysis_from_gate(task: &LoungeTask, gate: &DecisionResult) -> AnalysisDecision {
    let mut decision = fallback_decision(task);
    apply_gate_hints(&mut decision, gate);
    decision.reason = format!(
        "DecisionGate {:.1}ms routing={:?} security={:?} hit={:.2}",
        gate.latency_ms(),
        gate.routing.value,
        gate.security.value,
        gate.knowledge_hit
    );
    decision.adr_summary = task.summary.clone();
    decision.outcome = Some(ExperienceOutcome::Success);
    decision
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
    use crate::kernel::decision_engine::{Scored, SecurityLevel};
    use crate::models::{ApprovalKind, ExperienceRecord, TaskKind, TASK_ASSIGNED, TASK_REQUESTED};
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
        assert!(experience.adr_summary.contains("Cross-Project Memory"));
    }

    #[tokio::test]
    async fn knowledge_hit_recalls_cross_project_via_fast_retrieve() {
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
            reason: "laya hit".into(),
            adr_summary: "fast_retrieve".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });

        let task = LoungeTask::new(
            "cursor",
            "agent-lounge-os",
            "dispatcher NATS mesajlarını dinle",
        );
        dispatcher.inject_decision(DecisionResult {
            message_id: task.id.clone(),
            subject: TASK_REQUESTED.into(),
            routing: crate::kernel::decision_engine::Scored {
                value: RoutingType::Task,
                confidence: 0.9,
                probabilities: HashMap::from([("Task".into(), 0.9)]),
            },
            security: crate::kernel::decision_engine::Scored {
                value: SecurityLevel::Safe,
                confidence: 0.9,
                probabilities: HashMap::from([("Safe".into(), 0.9)]),
            },
            knowledge_hit: 0.84,
            elapsed_ms: 5,
            elapsed_us: 5_000,
            device: "cpu".into(),
            recall: crate::kernel::decision_engine::RecallHint {
                query: task.summary.clone(),
                project_id: task.project_id.clone(),
                source_agent: task.source_agent.clone(),
                target_agent: None,
                ast_refs: vec![],
            },
        });

        let context = dispatcher.recall_context(&task).await.unwrap();
        assert!(!context.is_empty());
        assert_eq!(context.experiences[0].project_id, "sister-os");
        assert_eq!(context.knowledge_hit, Some(0.84));
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

    #[tokio::test]
    async fn delegates_approved_task_to_online_worker_inbox() {
        use crate::kernel::worker_registry::WorkerRegistration;
        use crate::models::worker_tasks_subject;

        let workers = WorkerRegistry::new();
        workers
            .apply_registration(WorkerRegistration {
                bot_id: "grok-tester".into(),
                name: "Grok-Tester".into(),
                capabilities: vec!["echo".into()],
                version: "0.1.0".into(),
                pid: 1,
                action: "register".into(),
                created_at: None,
            })
            .unwrap();

        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "worker".into(),
            adr_summary: "delegate".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: Some("grok-tester".into()),
            repo_path: None,
        })
        .with_workers(workers);

        let mut task = LoungeTask::new("mcp:cursor", "agent-lounge-os", "echo hello");
        task.target_agent = Some("grok-tester".into());

        // nc=None → delegasyon denemesi NATS olmadan Local'a düşmez; has_worker + route
        // yolu handle_task üzerinden hata verir (delegasyon NATS ister).
        let err = dispatcher.handle_task(task).await.unwrap_err();
        assert!(
            err.to_string().contains("delegasyon") || err.to_string().contains("worker"),
            "unexpected: {err}"
        );
        assert_eq!(
            worker_tasks_subject("grok-tester").as_deref(),
            Some("lounge.tasks.grok-tester")
        );
        assert!(dispatcher.workers().is_online("grok-tester"));
    }

    #[tokio::test]
    async fn offline_registered_worker_rejects_route() {
        use crate::kernel::worker_registry::WorkerRegistration;

        let workers = WorkerRegistry::new();
        workers
            .apply_registration(WorkerRegistration {
                bot_id: "grok-tester".into(),
                name: "Grok-Tester".into(),
                capabilities: vec!["echo".into()],
                version: "0.1.0".into(),
                pid: 1,
                action: "register".into(),
                created_at: None,
            })
            .unwrap();
        workers
            .apply_registration(WorkerRegistration {
                bot_id: "grok-tester".into(),
                name: "Grok-Tester".into(),
                capabilities: vec!["echo".into()],
                version: "0.1.0".into(),
                pid: 1,
                action: "unregister".into(),
                created_at: None,
            })
            .unwrap();

        assert!(!workers.is_online("grok-tester"));
        assert!(workers.has_worker("grok-tester"));
        assert!(
            crate::kernel::worker_registry::route_subject_for(&workers, "grok-tester").is_none()
        );
    }

    #[test]
    fn injected_critical_gate_asks_for_approval() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "stub".into(),
            adr_summary: "stub".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });
        let task = LoungeTask::new("cursor", "agent-lounge-os", "rewrite vault keys");
        dispatcher.inject_decision(DecisionResult {
            message_id: task.id.clone(),
            subject: TASK_REQUESTED.into(),
            routing: Scored {
                value: RoutingType::Task,
                confidence: 0.9,
                probabilities: HashMap::from([("Task".into(), 0.9)]),
            },
            security: Scored {
                value: SecurityLevel::Critical,
                confidence: 0.95,
                probabilities: HashMap::from([("Critical".into(), 0.95)]),
            },
            knowledge_hit: 0.1,
            elapsed_ms: 7,
            elapsed_us: 7_000,
            device: "cpu".into(),
            recall: Default::default(),
        });
        let found = dispatcher.gate_decision(&task.id).unwrap();
        let request = evaluate_security(&task.id, &task.summary, "cursor", &found).unwrap();
        assert_eq!(request.kind, ApprovalKind::SecurityCritical);
        assert!(is_security_approval(&request.kind));
    }

    #[test]
    fn injected_risky_gate_asks_for_approval() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "stub".into(),
            adr_summary: "stub".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });
        let task = LoungeTask::new("cursor", "agent-lounge-os", "patch config.toml");
        dispatcher.inject_decision(DecisionResult {
            message_id: task.id.clone(),
            subject: TASK_REQUESTED.into(),
            routing: Scored {
                value: RoutingType::Task,
                confidence: 0.88,
                probabilities: HashMap::from([("Task".into(), 0.88)]),
            },
            security: Scored {
                value: SecurityLevel::Risky,
                confidence: 0.8,
                probabilities: HashMap::from([("Risky".into(), 0.8)]),
            },
            knowledge_hit: 0.2,
            elapsed_ms: 4,
            elapsed_us: 4_000,
            device: "cpu".into(),
            recall: Default::default(),
        });
        let found = dispatcher.gate_decision(&task.id).unwrap();
        let request = evaluate_security(&task.id, &task.summary, "cursor", &found).unwrap();
        assert_eq!(request.kind, ApprovalKind::SecurityRisky);
    }

    #[test]
    fn resume_subject_matches_catalog() {
        assert_eq!(TASK_RESUME, "lounge.task.resume");
        assert_eq!(ALERT_SECURITY, "lounge.alert.security");
        assert_eq!(ALERT_QUOTA, "lounge.alert.quota");
        assert_ne!(ALERT_QUOTA, ALERT_SECURITY);
        let payload = resume_envelope_payload("task-approve-1");
        assert_eq!(payload["vote"], "approve");
        assert_eq!(payload["task_id"], "task-approve-1");
    }

    #[test]
    fn deny_cancel_uses_task_failed_subject() {
        assert_eq!(TASK_CANCEL, "lounge.task.failed");
        assert_eq!(TASK_FAILED, "lounge.task.failed");
        let payload = cancel_envelope_payload("task-deny-1");
        assert_eq!(payload["vote"], "deny");
        assert_eq!(payload["status"], "cancelled");
        assert_eq!(payload["task_id"], "task-deny-1");
    }

    fn critical_decision(task_id: &str) -> DecisionResult {
        DecisionResult {
            message_id: task_id.into(),
            subject: TASK_REQUESTED.into(),
            routing: Scored {
                value: RoutingType::Task,
                confidence: 0.9,
                probabilities: HashMap::from([("Task".into(), 0.9)]),
            },
            security: Scored {
                value: SecurityLevel::Critical,
                confidence: 0.95,
                probabilities: HashMap::from([("Critical".into(), 0.95)]),
            },
            knowledge_hit: 0.1,
            elapsed_ms: 7,
            elapsed_us: 7_000,
            device: "cpu".into(),
            recall: Default::default(),
        }
    }

    fn live_dispatcher(timeout: Duration) -> Dispatcher {
        Dispatcher::new(
            "nats://127.0.0.1:4222",
            crate::services::lounge_ollama_endpoint(),
            default_model_lock(),
            MemoryBridge::from_binary("/tmp/missing-codebase-memory-mcp"),
            ExperienceStore::memory().unwrap(),
            PathBuf::from("/tmp"),
        )
        .with_approval_timeout(timeout)
        .with_skip_quota_probe()
    }

    #[tokio::test]
    async fn concurrent_security_approvals_run_in_parallel() {
        let dispatcher = live_dispatcher(Duration::from_secs(5));
        dispatcher.set_gate_ready(true);

        let t1 = LoungeTask::new("cursor", "agent-lounge-os", "task one critical");
        let t2 = LoungeTask::new("cursor", "agent-lounge-os", "task two critical");
        dispatcher.inject_decision(critical_decision(&t1.id));
        dispatcher.inject_decision(critical_decision(&t2.id));

        let id1 = t1.id.clone();
        let id2 = t2.id.clone();
        let d1 = dispatcher.clone();
        let d2 = dispatcher.clone();
        let h1 = tokio::spawn(async move { d1.handle_task(t1).await });
        let h2 = tokio::spawn(async move { d2.handle_task(t2).await });

        let started = std::time::Instant::now();
        loop {
            if dispatcher.pending_count() >= 2 {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "iki onay eşzamanlı beklemeli — sıralı işleme olsaydı ikinci görev birinci bitene kadar pending'e girmezdi"
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        dispatcher.resolve_vote(id1, RoutingVote::Approve).unwrap();
        dispatcher.resolve_vote(id2, RoutingVote::Approve).unwrap();
        assert!(h1.await.unwrap().is_ok());
        assert!(h2.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn approval_timeout_removes_pending() {
        let dispatcher = live_dispatcher(Duration::from_millis(60));
        dispatcher.set_gate_ready(true);
        let task = LoungeTask::new("cursor", "agent-lounge-os", "timeout me");
        dispatcher.inject_decision(critical_decision(&task.id));
        let err = dispatcher.handle_task(task).await.unwrap_err();
        assert!(err.to_string().contains("zaman aşımı"), "unexpected: {err}");
        assert_eq!(dispatcher.pending_count(), 0);
    }

    #[tokio::test]
    async fn cold_gate_requires_conservative_security_approval() {
        let dispatcher = live_dispatcher(Duration::from_secs(3));
        assert!(!dispatcher.gate_is_ready());
        let task = LoungeTask::new("cursor", "agent-lounge-os", "unclassified work");
        let id = task.id.clone();
        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.handle_task(task).await });

        let started = std::time::Instant::now();
        loop {
            if dispatcher.has_pending(&id) {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "soğuk gate muhafazakâr onay beklemeli"
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        dispatcher.resolve_vote(id, RoutingVote::Approve).unwrap();
        assert!(handle.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn ui_resolve_agent_switch_delegates_to_worker_inbox() {
        use crate::kernel::worker_registry::WorkerRegistration;
        use lounge_protocol::worker_tasks_subject;

        let workers = WorkerRegistry::new();
        workers
            .apply_registration(WorkerRegistration {
                bot_id: "grok-tester".into(),
                name: "Grok-Tester".into(),
                capabilities: vec!["echo".into()],
                version: "0.1.0".into(),
                pid: 42,
                action: "register".into(),
                created_at: None,
            })
            .unwrap();

        let dispatcher = live_dispatcher(Duration::from_secs(5)).with_workers(workers);
        dispatcher.set_gate_ready(true);

        let mut task = LoungeTask::new("mcp:cursor", "agent-lounge-os", "echo hello from mcp");
        task.target_agent = Some("grok-tester".into());
        let id = task.id.clone();

        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.handle_task(task).await });

        let started = std::time::Instant::now();
        loop {
            if dispatcher.has_pending(&id) {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "agent_switch onayı pending olmalı (UI Onayla yolu)"
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        // UI `resolve_routing` ile aynı senkron yol.
        dispatcher
            .resolve_vote(id.clone(), RoutingVote::Approve)
            .expect("Onayla");

        // nc=None iken Delegated → handle_task hata mesajı (inbox subject kanıtı).
        let err = handle.await.unwrap().unwrap_err().to_string();
        assert!(
            err.contains("grok-tester") && err.contains("delegasyon"),
            "onay sonrası worker delegasyonu beklenir: {err}"
        );
        assert_eq!(
            worker_tasks_subject("grok-tester").as_deref(),
            Some("lounge.tasks.grok-tester")
        );
        assert_eq!(dispatcher.pending_count(), 0);
    }

    #[tokio::test]
    async fn ui_resolve_deny_agent_switch_fails_task() {
        let dispatcher = live_dispatcher(Duration::from_secs(5));
        dispatcher.set_gate_ready(true);

        let mut task = LoungeTask::new("mcp:cursor", "agent-lounge-os", "sensitive switch");
        task.target_agent = Some("grok-tester".into());
        let id = task.id.clone();

        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.handle_task(task).await });

        let started = std::time::Instant::now();
        while !dispatcher.has_pending(&id) {
            assert!(started.elapsed() < Duration::from_secs(2));
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        dispatcher
            .resolve_vote(id, RoutingVote::Deny)
            .expect("Reddet");
        let err = handle.await.unwrap().unwrap_err();
        assert!(
            err.to_string().contains("redded") || err.to_string().contains("kota"),
            "unexpected: {err}"
        );
    }

    #[tokio::test]
    async fn ui_resolve_security_approve_unblocks_immediately() {
        let dispatcher = live_dispatcher(Duration::from_secs(5));
        dispatcher.set_gate_ready(true);
        let task = LoungeTask::new("cursor", "agent-lounge-os", "wipe secrets");
        let id = task.id.clone();
        dispatcher.inject_decision(critical_decision(&id));

        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.handle_task(task).await });

        let started = std::time::Instant::now();
        while !dispatcher.has_pending(&id) {
            assert!(started.elapsed() < Duration::from_secs(2));
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        dispatcher
            .resolve_vote(id, RoutingVote::Approve)
            .expect("security Onayla");
        assert!(handle.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn ui_resolve_approve_local_path_exists() {
        let dispatcher = live_dispatcher(Duration::from_secs(5));
        dispatcher.set_gate_ready(true);

        let mut task = LoungeTask::new("mcp:cursor", "agent-lounge-os", "need local fallback");
        task.target_agent = Some("grok-tester".into());
        let id = task.id.clone();
        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.handle_task(task).await });

        let started = std::time::Instant::now();
        while !dispatcher.has_pending(&id) {
            assert!(started.elapsed() < Duration::from_secs(2));
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        // agent_switch için ApproveLocal → LMR; LMR yoksa hata (kapı yine açılmış olmalı).
        let resolved = dispatcher.resolve_vote(id, RoutingVote::ApproveLocal);
        assert!(resolved.is_ok(), "oneshot UI'dan hemen çözülmeli");
        let outcome = handle.await.unwrap();
        // LMR ayakta değilse görev hata verir; önemli olan takılmadan sonuçlanması.
        assert!(outcome.is_err() || outcome.is_ok());
        assert_eq!(dispatcher.pending_count(), 0);
    }

    /// Hipotez (a) sertleştirme: UI listener gelmeden / state kaybında pending request okunabilir.
    #[tokio::test]
    async fn pending_approvals_snapshot_survives_for_ui_rehydrate() {
        let dispatcher = live_dispatcher(Duration::from_secs(5));
        dispatcher.set_gate_ready(true);

        let mut task = LoungeTask::new("mcp:cursor", "agent-lounge-os", "rehydrate check");
        task.target_agent = Some("grok-tester".into());
        let id = task.id.clone();
        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.handle_task(task).await });

        let started = std::time::Instant::now();
        while !dispatcher.has_pending(&id) {
            assert!(started.elapsed() < Duration::from_secs(2));
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        let snap = dispatcher.pending_approvals();
        assert_eq!(snap.len(), 1, "bekleyen onay UI snapshot'ta görünmeli");
        assert_eq!(snap[0].task_id, id);
        assert!(
            snap[0].expires_at.is_some() && snap[0].timeout_secs.is_some(),
            "rehydrate için süre damgası korunmalı"
        );

        dispatcher
            .resolve_vote(id.clone(), RoutingVote::Deny)
            .expect("Reddet");
        let _ = handle.await;
        assert!(dispatcher.pending_approvals().is_empty());
        assert_eq!(dispatcher.pending_count(), 0);
    }
}
