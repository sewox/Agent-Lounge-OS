#![allow(deprecated)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot, Mutex, RwLock};

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
use crate::models::{
    decide_route, default_ollama_model, AnalysisDecision, ApprovalRequest, ExperienceContext,
    ExperienceOutcome, ExperienceRecord, LoungeExperience, LoungeTask, QuotaVerdict, RouteIntent,
    RoutingVote, TaskAssignment, ALERT_QUOTA, EXPERIENCE_REPORTED, KERNEL_AGENT, TASK_ASSIGNED,
    TASK_COMPLETED, TASK_FAILED, TASK_REQUESTED,
};
use crate::services::{
    chat_json, embed_model, embed_text, evaluate_assignment, is_quota_approval,
    limit_policy_percent, lmr_endpoint_up, nats_monitor_endpoint, normalize_lmr_agent,
    quota_alert_envelope, MemoryBridge,
};

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
    decisions: DecisionCache,
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
            decisions: Arc::new(StdMutex::new(HashMap::new())),
        }
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
        self.execute_task(task, context, None).await
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
        Ok(record.to_lounge())
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
        }

        if self.stub_decision.is_some() {
            return Ok(());
        }

        let policy = self.store.get_routing_policy().await.unwrap_or_default();
        let quotas = probe_quotas(
            &self.ollama_endpoint,
            &nats_monitor_endpoint(),
            &self.memory,
        )
        .await;
        let to = decision
            .target_agent
            .as_deref()
            .or(task.target_agent.as_deref())
            .unwrap_or(KERNEL_AGENT);
        let limit = limit_policy_percent();
        let verdict = evaluate_assignment(&quotas, to, limit);
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
        let (tx, rx) = oneshot::channel();
        let task_id = request.task_id.clone();
        self.pending.lock().await.insert(task_id.clone(), tx);
        if let Some(app) = self.app.lock().expect("dispatcher app lock").clone() {
            let _ = app.emit("lounge://routing-approval", &request);
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
}
