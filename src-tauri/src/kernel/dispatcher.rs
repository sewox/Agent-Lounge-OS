#![allow(deprecated)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::{Context, Result};
use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot, RwLock};

use crate::db::{
    configured_agent_silence, configured_max_hops, configured_silence_scan_interval,
    lexical_embedding, AdmitError, AdmitOutcome, ExperienceStore, FastRetrieveQuery,
    ABSOLUTE_MAX_HOPS, DEFAULT_IDEMPOTENCY_TTL,
};
use crate::infra::probe_quotas;
use crate::kernel::decision_engine::{
    lookup_decision, DecisionCache, DecisionResult, RoutingType, SecurityLevel,
};
use crate::kernel::policy_manager::{
    alert_envelope_payload, cancel_envelope_payload, evaluate_security, is_security_approval,
    is_security_denied_error, resume_envelope_payload, ALERT_SECURITY, PENDING_APPROVAL,
    SECURITY_DENIED_MARKER, TASK_CANCEL, TASK_RESUME,
};
use crate::kernel::worker_registry::{route_subject_for, WorkerRegistry};
use crate::models::{
    decide_route, default_ollama_model, is_kernel, AnalysisDecision, ApprovalKind, ApprovalRequest,
    ExperienceContext, ExperienceOutcome, ExperienceRecord, LoungeExperience, LoungeTask,
    QuotaVerdict, RouteIntent, RoutingVote, TaskAssignment, TaskStatus, ALERT_QUOTA, CONTROL_STOP,
    EXPERIENCE_REPORTED, KERNEL_AGENT, TASK_ASSIGNED, TASK_COMPLETED, TASK_FAILED, TASK_REQUESTED,
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
/// Sessiz ajan → NEEDS_HUMAN olayı (UI kartı).
pub const TASK_NEEDS_HUMAN_EVENT: &str = "lounge://task-needs-human";

/// NATS giriş damgası — güven kararı payload/`source_agent`'tan türetilmez.
/// Her dış NATS mesajı sabit `source_verified=false`.
/// Geçersiz `session_id` yok sayılır (P2-h).
pub fn stamp_external_nats_ingress(task: &mut LoungeTask) {
    task.source_verified = false;
    if let Some(ref sid) = task.session_id {
        if !crate::bridge::session_id::is_valid_session_id(sid) {
            task.session_id = None;
        }
    }
}

/// NATS JSON → LoungeTask + dış giriş damgası (spoof `source_verified` yok sayılır).
pub fn parse_nats_task(data: &[u8]) -> Result<LoungeTask> {
    let mut task: LoungeTask =
        serde_json::from_slice(data).context("İş Emri shared task şemasına uymuyor")?;
    stamp_external_nats_ingress(&mut task);
    Ok(task)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ApprovalCleared {
    pub task_id: String,
    pub reason: String,
}

/// `execute_task` sonucu — yerel tamamlandı veya dış worker'a delege edildi.
#[derive(Debug, Clone)]
enum TaskExecution {
    Local(Box<LoungeExperience>),
    Delegated {
        bot_id: String,
        subject: String,
    },
    /// Aynı idempotency anahtarı — orijinal görev; NATS FAILED yayınlanmaz.
    IdempotentReplay {
        existing_task_id: String,
        status: TaskStatus,
    },
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
    /// A2A hop üst sınırı (varsayılan 10; `LOUNGE_MAX_HOPS` / with_max_hops; tavan 64).
    max_hops: u32,
    /// EXECUTING sessizliği sonrası NEEDS_HUMAN eşiği.
    agent_silence: Duration,
    /// Periyodik zombi tarama aralığı.
    silence_scan_interval: Duration,
    /// Paylaşılan NATS bağlantısı (lazy); her trusted görevde yeniden bağlanma yok.
    nats_conn: Arc<StdMutex<Option<nats::Connection>>>,
    /// Test: kota tükendi senaryosu (RouteIntent::Stop vb.).
    #[cfg(test)]
    force_quota_blocked: bool,
    /// Test: admit sonrası kontrollü hata (`analyze`/`publish`/`experience`/`nats_err`).
    #[cfg(test)]
    test_fail_after_admit: Option<&'static str>,
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
            max_hops: configured_max_hops(),
            agent_silence: configured_agent_silence(),
            silence_scan_interval: configured_silence_scan_interval(),
            nats_conn: Arc::new(StdMutex::new(None)),
            #[cfg(test)]
            force_quota_blocked: false,
            #[cfg(test)]
            test_fail_after_admit: None,
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

    pub fn with_max_hops(mut self, max_hops: u32) -> Self {
        self.max_hops = max_hops.clamp(1, ABSOLUTE_MAX_HOPS);
        self
    }

    pub fn with_agent_silence(mut self, silence: Duration) -> Self {
        self.agent_silence = silence;
        self
    }

    pub fn with_silence_scan_interval(mut self, interval: Duration) -> Self {
        self.silence_scan_interval = interval;
        self
    }

    #[cfg(test)]
    pub fn with_force_quota_blocked(mut self) -> Self {
        self.force_quota_blocked = true;
        self
    }

    #[cfg(test)]
    pub fn with_test_fail_after_admit(mut self, kind: &'static str) -> Self {
        self.test_fail_after_admit = Some(kind);
        self
    }

    pub fn max_hops(&self) -> u32 {
        self.max_hops
    }

    pub fn store(&self) -> &ExperienceStore {
        &self.store
    }

    /// Sessiz EXECUTING/DISPATCHED görevleri NEEDS_HUMAN yapar (test edilebilir saat).
    pub fn recover_silent_tasks(&self, now_rfc3339: &str) -> Result<Vec<String>> {
        let marked = self
            .store
            .recover_silent_a2a_tasks(now_rfc3339, self.agent_silence)?;
        for id in &marked {
            self.emit_needs_human(id);
        }
        Ok(marked)
    }

    fn emit_needs_human(&self, task_id: &str) {
        log::warn!("görev NEEDS_HUMAN (ajan sessiz/zombi): {task_id}");
        if let Some(app) = self.app.lock().expect("dispatcher app lock").clone() {
            let payload = serde_json::json!({
                "task_id": task_id,
                "status": TaskStatus::NeedsHuman.as_str(),
            });
            let _ = app.emit(TASK_NEEDS_HUMAN_EVENT, &payload);
        }
    }

    fn persist_status(&self, task_id: &str, status: TaskStatus) {
        if let Err(err) = self.store.set_a2a_task_status(task_id, status.clone()) {
            log::warn!("a2a status yazılamadı {task_id}→{}: {err}", status.as_str());
        }
    }

    /// Kernel açılışında: periyodik zombi tarama + idempotency GC.
    pub fn spawn_silence_watchdog(self: &Dispatcher) {
        let this = self.clone();
        let interval = self.silence_scan_interval;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(interval).await;
                let now = crate::models::now_rfc3339();
                if let Err(err) = this.recover_silent_tasks(&now) {
                    log::warn!("silence watchdog: {err}");
                }
                let cutoff = (chrono::Utc::now()
                    - chrono::Duration::from_std(DEFAULT_IDEMPOTENCY_TTL)
                        .unwrap_or_else(|_| chrono::Duration::hours(24)))
                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
                match this.store.gc_a2a_idempotency(&cutoff) {
                    Ok(n) if n > 0 => log::info!("idempotency GC: {n} anahtar silindi"),
                    Ok(_) => {}
                    Err(err) => log::warn!("idempotency GC: {err}"),
                }
            }
        });
    }

    /// Workflow / kernel iç kanal — DB parent’tan gelen `source_verified` korunur
    /// (zorla true yapılmaz; unverified parent → SourceUnverified kapısı).
    pub fn spawn_trusted_ingress(self: &Dispatcher, mut rx: mpsc::Receiver<LoungeTask>) {
        let this = self.clone();
        tokio::spawn(async move {
            while let Some(task) = rx.recv().await {
                if let Err(err) = this.handle_workflow_followup(task).await {
                    log::error!("workflow ingress görev hatası: {err}");
                }
            }
        });
    }

    /// Worker TASK_COMPLETED / TASK_FAILED → `a2a_tasks` status bağlama.
    pub fn spawn_lifecycle_listener(self: &Dispatcher) {
        let this = self.clone();
        let url = self.nats_url.clone();
        tokio::spawn(async move {
            loop {
                if let Err(err) = this.listen_lifecycle_once(&url).await {
                    log::warn!("task lifecycle dinleyici: {err}");
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    /// `lounge.control.stop` → görev CANCELLED (MCP context canceled yayınını tüketir).
    pub fn spawn_control_stop_listener(self: &Dispatcher) {
        let this = self.clone();
        let url = self.nats_url.clone();
        tokio::spawn(async move {
            loop {
                if let Err(err) = this.listen_control_stop_once(&url).await {
                    log::warn!("control.stop dinleyici: {err}");
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        });
    }

    async fn listen_control_stop_once(&self, url: &str) -> Result<()> {
        let url_owned = url.to_string();
        let nc = tokio::task::spawn_blocking(move || nats::connect(&url_owned))
            .await
            .context("control.stop NATS connect join")?
            .context("control.stop NATS bağlantısı kurulamadı")?;

        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(64);
        let sub_nc = nc.clone();
        let subject = CONTROL_STOP.to_string();
        tokio::task::spawn_blocking(move || {
            let sub = sub_nc.subscribe(&subject)?;
            for msg in sub.messages() {
                if tx.blocking_send(msg.data.to_vec()).is_err() {
                    break;
                }
            }
            Ok::<_, std::io::Error>(())
        });

        log::info!("dispatcher control.stop dinliyor: {url} ({CONTROL_STOP})");
        while let Some(data) = rx.recv().await {
            if let Err(err) = self.apply_control_stop(&data) {
                log::warn!("control.stop uygulama: {err}");
            }
        }
        Ok(())
    }

    /// CONTROL_STOP payload → görev iptal (oturum eşleşirse).
    pub fn apply_control_stop(&self, data: &[u8]) -> Result<()> {
        let value: serde_json::Value =
            serde_json::from_slice(data).context("control.stop payload json")?;
        let task_id = value
            .get("task_id")
            .or_else(|| value.get("id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("control.stop: task_id yok"))?
            .to_string();
        let session_id = value
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if session_id.is_empty() {
            // Oturumsuz stop — yalnız status Cancelled (fail-open değil: session yoksa
            // cancel_a2a_task yetkisiz der; doğrudan status yaz).
            self.persist_status(&task_id, TaskStatus::Cancelled);
            let _ = self.store.release_a2a_idempotency(&task_id);
            return Ok(());
        }
        self.store
            .cancel_a2a_task(&task_id, session_id)
            .with_context(|| format!("control.stop cancel {task_id}"))?;
        Ok(())
    }

    async fn listen_lifecycle_once(&self, url: &str) -> Result<()> {
        let url_owned = url.to_string();
        let nc = tokio::task::spawn_blocking(move || nats::connect(&url_owned))
            .await
            .context("lifecycle NATS connect join")?
            .context("lifecycle NATS bağlantısı kurulamadı")?;

        let (tx, mut rx) = mpsc::channel::<(String, Vec<u8>)>(64);
        for subject in [TASK_COMPLETED, TASK_FAILED] {
            let sub_nc = nc.clone();
            let tx = tx.clone();
            let subject = subject.to_string();
            tokio::task::spawn_blocking(move || {
                let sub = sub_nc.subscribe(&subject)?;
                for msg in sub.messages() {
                    if tx
                        .blocking_send((msg.subject.clone(), msg.data.to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
                Ok::<_, std::io::Error>(())
            });
        }
        drop(tx);

        log::info!("dispatcher lifecycle dinliyor: {url} ({TASK_COMPLETED}|{TASK_FAILED})");
        while let Some((subject, data)) = rx.recv().await {
            let failed = subject == TASK_FAILED;
            match self.apply_bus_terminal(&data, failed) {
                Ok(()) => {}
                Err(err) => {
                    let msg = err.to_string();
                    // Kendi TASK_COMPLETED yankısı (zaten Completed) — log gürültüsü yok.
                    if msg.contains("lifecycle_echo_completed") {
                        log::debug!("lifecycle echo ignored: {msg}");
                    } else {
                        log::warn!("lifecycle status güncellemesi: {err}");
                    }
                }
            }
        }
        Ok(())
    }

    /// NATS tamamlanma/hata zarfından `a2a_tasks` güncelle — yalnız izinli geçişler.
    ///
    /// İzinli: DISPATCHED | EXECUTING | QUEUED | RECOVERY_PENDING | **NeedsHuman**
    /// → Completed/Failed. NeedsHuman kurtarma: watchdog sonrası geç gelen worker sonucu.
    /// Zaten Completed + !failed → sessiz no-op (kendi yayın yankısı).
    ///
    /// Not (PR-5): NATS yayıncı kimlik doğrulaması yok; `target_agent` eşleşmesi
    /// mümkün olduğunda kontrol edilir, aksi halde durum makinesi tek kapıdır.
    pub fn apply_bus_terminal(&self, data: &[u8], failed: bool) -> Result<()> {
        let value: serde_json::Value =
            serde_json::from_slice(data).context("lifecycle payload json")?;
        let task_id = value
            .get("id")
            .or_else(|| value.get("task_id"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("lifecycle payload'da id/task_id yok"))?
            .to_string();

        let current = self
            .store
            .a2a_task_status(&task_id)?
            .ok_or_else(|| anyhow::anyhow!("lifecycle: a2a_tasks'ta id yok: {task_id}"))?;

        // Kendi Local TASK_COMPLETED yankısı — sessiz yok say.
        if matches!(current, TaskStatus::Completed) && !failed {
            anyhow::bail!("lifecycle_echo_completed: {task_id}");
        }

        let allowed = matches!(
            current,
            TaskStatus::Dispatched
                | TaskStatus::Executing
                | TaskStatus::Queued
                | TaskStatus::RecoveryPending
                | TaskStatus::NeedsHuman
                | TaskStatus::WaitTimeoutReached
        );
        if !allowed {
            anyhow::bail!(
                "lifecycle geçiş reddedildi: {task_id} status={} → terminal",
                current.as_str()
            );
        }

        // Mümkünse target_agent eşleşmesi (payload.target_agent veya task.target_agent).
        if let Ok(Some(row)) = self.store.load_a2a_task(&task_id) {
            if let Some(expected) = row.target_agent.as_deref().filter(|s| !s.trim().is_empty()) {
                let claimed = value
                    .get("target_agent")
                    .and_then(|v| v.as_str())
                    .or_else(|| value.pointer("/task/target_agent").and_then(|v| v.as_str()));
                if let Some(claimed) = claimed {
                    if !claimed.eq_ignore_ascii_case(expected) {
                        anyhow::bail!(
                            "lifecycle target_agent uyuşmazlığı: task={task_id} expected={expected} claimed={claimed}"
                        );
                    }
                }
                // claimed yoksa PR-5 notu: NATS auth yok; durum makinesi yeterli.
            }
        }

        // Sonuç + durum atomik (P1-A): önce Completed sonra ayrı result_json → wake yarışı.
        let result_raw = value
            .get("result")
            .cloned()
            .or_else(|| value.get("output").cloned())
            .or_else(|| {
                value
                    .get("result_json")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(s).ok())
            })
            .map(|result_val| {
                if result_val.is_string() {
                    result_val.as_str().unwrap_or("").to_string()
                } else {
                    result_val.to_string()
                }
            })
            .filter(|s| !s.is_empty());

        if failed {
            self.store
                .complete_a2a_with_result(
                    &task_id,
                    TaskStatus::Failed,
                    result_raw.as_deref(),
                    /*release_idempotency*/ true,
                )
                .map_err(|e| anyhow::anyhow!("bus terminal failed yazılamadı: {e}"))?;
        } else {
            self.store
                .complete_a2a_with_result(
                    &task_id,
                    TaskStatus::Completed,
                    result_raw.as_deref(),
                    /*release_idempotency*/ false,
                )
                .map_err(|e| anyhow::anyhow!("bus terminal completed yazılamadı: {e}"))?;
        }
        Ok(())
    }

    /// Paylaşılan NATS bağlantısı (lazy connect + cache; async yolunda spawn_blocking).
    async fn shared_nats(&self) -> Option<nats::Connection> {
        {
            let guard = self.nats_conn.lock().expect("nats_conn lock");
            if let Some(nc) = guard.as_ref() {
                return Some(nc.clone());
            }
        }
        let url = self.nats_url.clone();
        match tokio::task::spawn_blocking(move || nats::connect(&url)).await {
            Ok(Ok(nc)) => {
                let mut guard = self.nats_conn.lock().expect("nats_conn lock");
                *guard = Some(nc.clone());
                Some(nc)
            }
            Ok(Err(err)) => {
                log::warn!("NATS paylaşılan bağlantı kurulamadı: {err}");
                let mut guard = self.nats_conn.lock().expect("nats_conn lock");
                *guard = None;
                None
            }
            Err(err) => {
                log::warn!("NATS paylaşılan bağlantı join: {err}");
                let mut guard = self.nats_conn.lock().expect("nats_conn lock");
                *guard = None;
                None
            }
        }
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
                // Clear OS-notification pending slot immediately so Focused/Reopen
                // cannot re-raise the window with a stale id after a user decision.
                crate::services::clear_pending_approval_if_matches(&task_id);
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
        let task = parse_nats_task(&msg.data)?;
        let context = self.recall_context(&task).await.unwrap_or_else(|err| {
            log::warn!("tecrübe araması atlandı: {err}");
            ExperienceContext::default()
        });

        match self.execute_task(task.clone(), context, Some(nc)).await {
            Ok(TaskExecution::Local(experience)) => {
                publish_json(nc, TASK_COMPLETED, &task).await?;
                publish_json(nc, EXPERIENCE_REPORTED, experience.as_ref()).await?;
            }
            Ok(TaskExecution::Delegated { bot_id, subject }) => {
                log::info!(
                    "görev {} onay sonrası dış worker'a yönlendirildi: {bot_id} → {subject}",
                    task.id
                );
            }
            Ok(TaskExecution::IdempotentReplay {
                existing_task_id,
                status,
            }) => {
                log::info!(
                    "idempotency replay: existing={existing_task_id} status={} (TASK_FAILED yayınlanmaz)",
                    status.as_str()
                );
            }
            Err(err) => {
                log::error!("görev başarısız {}: {err}", task.id);
                self.emit_approval_cleared(&task.id, "failed");
                // Güvenlik Red zaten lounge.task.failed (cancel) yayınladı.
                // fail_and_release_idempotency execute_task Err sarmalayıcısında.
                if !is_security_denied_error(&err) {
                    publish_json(nc, TASK_FAILED, &task).await?;
                }
            }
        }
        Ok(())
    }

    /// Kernel / test senkron yolu — görev zaten güvenilir oluşturulmuş olmalı.
    pub async fn handle_task(&self, task: LoungeTask) -> Result<LoungeExperience> {
        let context = self.recall_context(&task).await.unwrap_or_default();
        match self.execute_task(task, context, None).await? {
            TaskExecution::Local(experience) => Ok(*experience),
            TaskExecution::Delegated { bot_id, .. } => {
                anyhow::bail!(
                    "görev dış worker'a delegasyon bekliyor: {bot_id} (NATS bağlantısı yok)"
                )
            }
            TaskExecution::IdempotentReplay {
                existing_task_id,
                status,
            } => anyhow::bail!(
                "idempotency_key replay: existing_task={existing_task_id} status={}",
                status.as_str()
            ),
        }
    }

    /// Explicit trusted entry — yalnız testler (üretim workflow DB `source_verified` kullanır).
    #[cfg(test)]
    pub async fn handle_trusted_task(&self, mut task: LoungeTask) -> Result<()> {
        task.source_verified = true;
        self.dispatch_with_shared_nats(task).await
    }

    /// Workflow follow-up — DB parent’tan gelen `source_verified` korunur (zorla true yok).
    pub async fn handle_workflow_followup(&self, task: LoungeTask) -> Result<()> {
        self.dispatch_with_shared_nats(task).await
    }

    async fn dispatch_with_shared_nats(&self, task: LoungeTask) -> Result<()> {
        let context = self.recall_context(&task).await.unwrap_or_else(|err| {
            log::warn!("tecrübe araması atlandı: {err}");
            ExperienceContext::default()
        });
        let nc = self.shared_nats().await;
        let nc_ref = nc.as_ref();
        match self.execute_task(task.clone(), context, nc_ref).await {
            Ok(TaskExecution::Local(experience)) => {
                if let Some(nc) = nc_ref {
                    publish_json(nc, TASK_COMPLETED, &task).await?;
                    publish_json(nc, EXPERIENCE_REPORTED, experience.as_ref()).await?;
                } else {
                    // NATS yok: yerel tamamlandı ama bus’a yazılmadı — sessizlik / workflow tetiklenmez.
                    log::warn!(
                        "görev {} Local tamamlandı ama NATS yok — TASK_COMPLETED/EXPERIENCE yayınlanmadı \
                         (workflow follow-up ve dış tüketiciler bu tamamlanmayı görmez)",
                        task.id
                    );
                }
                Ok(())
            }
            Ok(TaskExecution::Delegated { bot_id, subject }) => {
                log::info!(
                    "internal görev {} dış worker'a: {bot_id} → {subject}",
                    task.id
                );
                Ok(())
            }
            Ok(TaskExecution::IdempotentReplay {
                existing_task_id,
                status,
            }) => {
                log::info!(
                    "internal idempotency replay: existing={existing_task_id} status={}",
                    status.as_str()
                );
                Ok(())
            }
            Err(err) => {
                self.emit_approval_cleared(&task.id, "failed");
                if let Some(nc) = nc_ref {
                    if !is_security_denied_error(&err) {
                        let _ = publish_json(nc, TASK_FAILED, &task).await;
                    }
                }
                Err(err)
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

        if let Some(replay) = self.admit_inbound(&mut task)? {
            return Ok(replay);
        }

        let task_id = task.id.clone();
        match self.execute_admitted_task(task, context, nc).await {
            Ok(result) => Ok(result),
            Err(err) => {
                // Analiz / Stop / worker offline / publish / tecrübe yazma / onay red —
                // admit sonrası her hata anahtarı yakmasın.
                self.fail_and_release_idempotency(&task_id);
                Err(err)
            }
        }
    }

    async fn execute_admitted_task(
        &self,
        mut task: LoungeTask,
        context: ExperienceContext,
        nc: Option<&nats::Connection>,
    ) -> Result<TaskExecution> {
        #[cfg(test)]
        if let Some(kind) = self.test_fail_after_admit {
            match kind {
                "analyze" => anyhow::bail!("analiz hatası (test)"),
                "route_stop" => anyhow::bail!("route stop (test)"),
                "publish" => anyhow::bail!("NATS publish başarısız (test)"),
                "experience" => anyhow::bail!("tecrübe SQLite'a yazılamadı (test)"),
                "nats_err" => anyhow::bail!("handle_nats_message Err dalı (test)"),
                _ => {}
            }
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
                self.persist_status(&task.id, TaskStatus::Dispatched);
                publish_json(nc, &subject, &assignment).await?;
                return Ok(TaskExecution::Delegated {
                    bot_id: target,
                    subject,
                });
            }
        } else if let Some(subject) = worker_inbox {
            self.persist_status(&task.id, TaskStatus::Dispatched);
            // Test / senkron yol: NATS yokken de yerel çalıştırmayı atla.
            return Ok(TaskExecution::Delegated {
                bot_id: target,
                subject,
            });
        }

        self.persist_status(&task.id, TaskStatus::Executing);

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
        self.persist_status(&task.id, TaskStatus::Completed);
        Ok(TaskExecution::Local(Box::new(record.to_lounge())))
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

    /// A2A kabul: hop zinciri, idempotency (tek txn), lineage normalizasyonu.
    /// `Some(IdempotentReplay)` → yürütme yok.
    fn admit_inbound(&self, task: &mut LoungeTask) -> Result<Option<TaskExecution>> {
        match self.store.admit_a2a_task(task, self.max_hops) {
            Ok(AdmitOutcome::Accepted(_)) => Ok(None),
            Ok(AdmitOutcome::Replay {
                existing_task_id,
                status,
            }) => Ok(Some(TaskExecution::IdempotentReplay {
                existing_task_id,
                status,
            })),
            Err(AdmitError::HopLimitExceeded {
                hop_count,
                max_hops,
            }) => anyhow::bail!("hop limiti aşıldı: hop_count={hop_count} max_hops={max_hops}"),
            Err(AdmitError::ParentMissing { parent_id }) => {
                anyhow::bail!("parent_id zinciri kırık: {parent_id}")
            }
            Err(AdmitError::ParentInvalid { parent_id, reason }) => {
                anyhow::bail!("parent_id geçersiz ({parent_id}): {reason}")
            }
            Err(AdmitError::IdConflict { task_id }) => {
                // MCP köprüsü önceden admit etmiş olabilir — aynı id + session ise devam.
                // P1-1: NATS girişi verified yükseltmez (#82 garantisi).
                if let Ok(Some(existing)) = self.store.load_a2a_task(&task_id) {
                    if existing.session_id.is_some()
                        && existing.session_id == task.session_id
                        && !matches!(
                            existing.status,
                            TaskStatus::Completed
                                | TaskStatus::Failed
                                | TaskStatus::Cancelled
                                | TaskStatus::Expired
                        )
                    {
                        let preserved_session = existing.session_id.clone();
                        *task = existing;
                        // NATS stamp again — DB'deki verified=true MCP satırı trust yükseltmez.
                        task.source_verified = false;
                        // Geçersiz NATS session_id yok sayılır (P2-h).
                        if let Some(ref sid) = preserved_session {
                            if !crate::bridge::session_id::is_valid_session_id(sid) {
                                task.session_id = None;
                            }
                        }
                        return Ok(None);
                    }
                }
                anyhow::bail!("a2a_tasks id çakışması: {task_id}")
            }
            Err(AdmitError::Storage { message }) => {
                anyhow::bail!("a2a storage hatası (fail-closed): {message}")
            }
        }
    }

    /// Kullanıcı tercihi + kota; harici ajan geçişi onaysız olamaz.
    /// `source_verified=false` → **ilk ek kapı**; onay sonrası güvenlik/kota/routing aynen devam eder.
    async fn apply_routing_policy(
        &self,
        task: &mut LoungeTask,
        decision: &AnalysisDecision,
    ) -> Result<()> {
        if !task.source_verified {
            task.status = TaskStatus::PendingApproval;
            self.persist_status(&task.id, TaskStatus::PendingApproval);
            let request = ApprovalRequest {
                task_id: task.id.clone(),
                summary: task.summary.clone(),
                from_agent: task.source_agent.clone(),
                to_agent: task
                    .target_agent
                    .clone()
                    .unwrap_or_else(|| KERNEL_AGENT.into()),
                kind: ApprovalKind::SourceUnverified,
                reason: format!(
                    "source_verified=false — doğrulanmamış NATS/kaynak talebi · status={PENDING_APPROVAL}"
                ),
                expires_at: None,
                timeout_secs: None,
            };
            let policy = self.store.get_routing_policy().await.unwrap_or_default();
            // İlk kapı — return etme; güvenlik/kota aşağıda devam eder.
            self.await_approval(
                task,
                request,
                None,
                None,
                &policy.local_fallback_agent,
                &policy.local_fallback_model,
            )
            .await?;
            task.status = TaskStatus::Queued;
            self.persist_status(&task.id, TaskStatus::Queued);
        }

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
            #[cfg(test)]
            if !self.force_quota_blocked {
                return Ok(());
            }
            #[cfg(not(test))]
            {
                return Ok(());
            }
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
        #[cfg(test)]
        let force_blocked = self.force_quota_blocked;
        #[cfg(not(test))]
        let force_blocked = false;
        let verdict = if force_blocked {
            QuotaVerdict::Block {
                reason: format!("{to} kotası engellendi (test)"),
                tool: to.into(),
                percent: None,
            }
        } else if self.skip_quota_probe {
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
        // Always clear the pending-notification slot (id-matched), even when the
        // AppHandle is not wired (unit tests / early boot).
        crate::services::clear_pending_approval_if_matches(task_id);
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
        // Her onay beklerken PENDING_APPROVAL — QUEUED watchdog NEEDS_HUMAN üretmesin.
        task.status = TaskStatus::PendingApproval;
        self.persist_status(&task.id, TaskStatus::PendingApproval);

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
                self.fail_and_release_idempotency(&task_id);
                anyhow::bail!("routing onay kanalı kapandı")
            }
            Err(_) => {
                self.pending
                    .lock()
                    .expect("dispatcher pending lock")
                    .remove(&task_id);
                self.emit_approval_cleared(&task_id, "timeout");
                self.fail_and_release_idempotency(&task_id);
                anyhow::bail!("routing onayı zaman aşımı")
            }
        };

        // Approve / ApproveLocal / Deny — clear slot + notify UI (resolve_vote also
        // clears immediately; this covers any future non-oneshot resolution paths).
        let clear_reason = match &vote {
            RoutingVote::Approve => "approved",
            RoutingVote::ApproveLocal => "approved_local",
            RoutingVote::Deny => "denied",
        };
        self.emit_approval_cleared(&task_id, clear_reason);

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
                task.status = TaskStatus::Queued;
                self.persist_status(&task.id, TaskStatus::Queued);
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
                task.status = TaskStatus::Queued;
                self.persist_status(&task.id, TaskStatus::Queued);
                Ok(())
            }
            RoutingVote::Deny => {
                self.fail_and_release_idempotency(&task_id);
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

    /// Reddet / zaman aşımı / FAILED — idempotency anahtarı yanmasın (yeniden denenebilir).
    fn fail_and_release_idempotency(&self, task_id: &str) {
        self.persist_status(task_id, TaskStatus::Failed);
        if let Err(err) = self.store.release_a2a_idempotency(task_id) {
            log::warn!("idempotency release failed for {task_id}: {err}");
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
                started.elapsed() < Duration::from_secs(10),
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
    async fn resolve_vote_clears_notification_pending_slot() {
        use crate::services::{
            approval_notify::pending_slot_test_lock, clear_pending_approval_if_matches,
            pending_approval_task_id, set_pending_approval_task_id, should_focus_on_activation,
        };

        let dispatcher = live_dispatcher(Duration::from_secs(5));
        dispatcher.set_gate_ready(true);
        let task = LoungeTask::new("cursor", "agent-lounge-os", "slot clear me");
        let id = task.id.clone();
        dispatcher.inject_decision(critical_decision(&id));
        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.handle_task(task).await });

        let started = std::time::Instant::now();
        loop {
            if dispatcher.has_pending(&id) {
                break;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "approval should become pending"
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        // Hold the process-wide slot lock only around synchronous slot mutations —
        // never across `.await` (clippy::await_holding_lock).
        {
            let _slot_guard = pending_slot_test_lock();
            set_pending_approval_task_id(Some(id.clone()));
            assert!(should_focus_on_activation());

            dispatcher
                .resolve_vote(id.clone(), RoutingVote::Approve)
                .unwrap();
            assert_eq!(
                pending_approval_task_id(),
                None,
                "resolve_vote must clear the notification pending slot"
            );
            assert!(
                !should_focus_on_activation(),
                "activation after resolve must be inert"
            );
            set_pending_approval_task_id(Some("other".into()));
            assert!(!clear_pending_approval_if_matches(&id));
            assert_eq!(pending_approval_task_id().as_deref(), Some("other"));
            let _ = clear_pending_approval_if_matches("other");
        }

        assert!(handle.await.unwrap().is_ok());
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
                started.elapsed() < Duration::from_secs(10),
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
                started.elapsed() < Duration::from_secs(10),
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
            assert!(started.elapsed() < Duration::from_secs(10));
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
            assert!(started.elapsed() < Duration::from_secs(10));
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
            assert!(started.elapsed() < Duration::from_secs(10));
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
            assert!(started.elapsed() < Duration::from_secs(10));
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

    #[tokio::test]
    async fn hop_limit_blocks_chain_at_dispatcher() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "hop".into(),
            adr_summary: "hop".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        })
        .with_max_hops(10);

        let root = LoungeTask::new("a", "p", "root");
        dispatcher.handle_task(root.clone()).await.unwrap();

        let store = dispatcher.store();
        let mut parent_id = root.id.clone();
        for hop in 1..10 {
            let mut child = LoungeTask::new("b", "p", format!("hop-{hop}"));
            child.parent_task_id = Some(parent_id.clone());
            child.source_verified = true;
            store.admit_a2a_task(&mut child, 10).unwrap();
            assert_eq!(child.hop_count, hop);
            parent_id = child.id;
        }
        let mut over = LoungeTask::new("c", "p", "blocked");
        over.parent_task_id = Some(parent_id);
        over.source_verified = true;
        let err = dispatcher.handle_task(over).await.unwrap_err().to_string();
        assert!(
            err.contains("hop limiti") || err.contains("hop_count"),
            "unexpected: {err}"
        );
    }

    #[tokio::test]
    async fn duplicate_idempotency_key_rejected_at_dispatcher() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "idem".into(),
            adr_summary: "idem".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });
        let mut t1 = LoungeTask::new("a", "p", "first");
        t1.idempotency_key = Some("dispatch-key".into());
        dispatcher.handle_task(t1.clone()).await.unwrap();

        let mut t2 = LoungeTask::new("a", "p", "second");
        t2.idempotency_key = Some("dispatch-key".into());
        let err = dispatcher.handle_task(t2).await.unwrap_err().to_string();
        assert!(
            err.contains("idempotency_key replay") || err.contains("existing_task"),
            "unexpected: {err}"
        );
    }

    #[tokio::test]
    async fn source_verified_false_goes_pending_approval() {
        let dispatcher = live_dispatcher(Duration::from_secs(5));
        dispatcher.set_gate_ready(true);
        let mut task = LoungeTask::new("nats-agent", "agent-lounge-os", "unverified nats");
        task.source_verified = false;
        let id = task.id.clone();
        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.handle_task(task).await });

        let started = std::time::Instant::now();
        while !dispatcher.has_pending(&id) {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "source_verified=false PENDING_APPROVAL beklemeli"
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        let snap = dispatcher.pending_approvals();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].kind, ApprovalKind::SourceUnverified);
        assert!(
            snap[0].reason.contains(PENDING_APPROVAL),
            "reason should mention PENDING_APPROVAL: {}",
            snap[0].reason
        );

        dispatcher
            .resolve_vote(id, RoutingVote::Approve)
            .expect("Onayla");
        assert!(handle.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn unverified_then_critical_requires_second_gate() {
        let dispatcher = live_dispatcher(Duration::from_secs(8));
        dispatcher.set_gate_ready(true);
        let mut task = LoungeTask::new("nats-agent", "agent-lounge-os", "wipe secrets unverified");
        task.source_verified = false;
        let id = task.id.clone();
        dispatcher.inject_decision(critical_decision(&id));

        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.handle_task(task).await });

        let started = std::time::Instant::now();
        while !dispatcher.has_pending(&id) {
            assert!(started.elapsed() < Duration::from_secs(10));
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        assert_eq!(
            dispatcher.pending_approvals()[0].kind,
            ApprovalKind::SourceUnverified
        );
        dispatcher
            .resolve_vote(id.clone(), RoutingVote::Approve)
            .unwrap();

        // İkinci kapı: güvenlik Critical
        let started = std::time::Instant::now();
        while !dispatcher.has_pending(&id) {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "unverified onayından sonra Critical kapısı beklenir"
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        assert_eq!(
            dispatcher.pending_approvals()[0].kind,
            ApprovalKind::SecurityCritical
        );
        dispatcher.resolve_vote(id, RoutingVote::Approve).unwrap();
        assert!(handle.await.unwrap().is_ok());
    }

    #[test]
    fn spoofed_source_agent_cannot_bypass_nats_ingress_stamp() {
        for spoof in [
            "workflow_engine",
            "Workflow_Engine",
            "WORKFLOW_ENGINE",
            "lounge-kernel",
            "kernel",
            "KERNEL",
            "",
            "  ",
            "dispatcher",
        ] {
            let raw = serde_json::json!({
                "id": "spoof-id",
                "type": "task",
                "source_agent": spoof,
                "project_id": "p",
                "summary": "spoofed",
                "source_verified": true,
                "created_at": "2026-10-04T12:00:00.000Z",
            });
            let task = parse_nats_task(raw.to_string().as_bytes()).unwrap();
            assert!(
                !task.source_verified,
                "parse_nats_task spoof source_agent={spoof:?} için false olmalı"
            );
        }
    }

    #[tokio::test]
    async fn spoofed_nats_workflow_engine_requires_unverified_approval() {
        let dispatcher = live_dispatcher(Duration::from_secs(5));
        dispatcher.set_gate_ready(true);
        let raw = serde_json::json!({
            "id": "spoof-nats-wf-engine",
            "type": "task",
            "source_agent": "workflow_engine",
            "project_id": "agent-lounge-os",
            "summary": "nats spoof",
            "source_verified": true,
            "created_at": "2026-10-04T12:00:00.000Z",
        });
        let task = parse_nats_task(raw.to_string().as_bytes()).unwrap();
        assert!(!task.source_verified);
        let id = task.id.clone();
        // parse keeps client id from JSON
        assert_eq!(id, "spoof-nats-wf-engine");
        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.handle_task(task).await });

        let started = std::time::Instant::now();
        while !dispatcher.has_pending(&id) {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "spoof workflow_engine SourceUnverified beklemeli"
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        assert_eq!(
            dispatcher.pending_approvals()[0].kind,
            ApprovalKind::SourceUnverified
        );
        assert_eq!(
            dispatcher.store().a2a_task_status(&id).unwrap(),
            Some(TaskStatus::PendingApproval)
        );
        dispatcher
            .resolve_vote(id, RoutingVote::Approve)
            .expect("Onayla");
        assert!(handle.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn trusted_internal_path_skips_unverified_gate() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "wf".into(),
            adr_summary: "wf".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });
        let mut task = LoungeTask::new("workflow_engine", "agent-lounge-os", "test followup");
        // NATS spoof denemesi — trusted path zorla true yapar.
        stamp_external_nats_ingress(&mut task);
        assert!(!task.source_verified);
        assert!(dispatcher.handle_trusted_task(task).await.is_ok());
        assert_eq!(dispatcher.pending_count(), 0);
    }

    #[tokio::test]
    async fn agent_silence_marks_needs_human() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "silence".into(),
            adr_summary: "silence".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        })
        .with_agent_silence(Duration::from_secs(60));

        let task = LoungeTask::new("a", "p", "zombie-agent");
        dispatcher.handle_task(task.clone()).await.unwrap();
        assert_eq!(
            dispatcher.store().a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::Completed)
        );
        dispatcher
            .store()
            .set_a2a_task_status(&task.id, TaskStatus::Executing)
            .unwrap();
        dispatcher
            .store()
            .touch_a2a_updated_at(&task.id, "2026-10-04T10:00:00.000Z")
            .unwrap();

        let marked = dispatcher
            .recover_silent_tasks("2026-10-04T10:02:00.000Z")
            .unwrap();
        assert_eq!(marked, vec![task.id.clone()]);
        assert_eq!(
            dispatcher.store().a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::NeedsHuman)
        );
    }

    #[tokio::test]
    async fn integration_admit_executing_silence_needs_human() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "flow".into(),
            adr_summary: "flow".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        })
        .with_agent_silence(Duration::from_secs(30));

        let task = LoungeTask::new("cursor", "proj", "full flow");
        let id = task.id.clone();
        dispatcher.handle_task(task).await.unwrap();
        // Gerçek akış: gönder → EXECUTING → sessiz → NEEDS_HUMAN
        dispatcher
            .store()
            .set_a2a_task_status(&id, TaskStatus::Executing)
            .unwrap();
        dispatcher
            .store()
            .touch_a2a_updated_at(&id, "2026-10-04T09:00:00.000Z")
            .unwrap();
        let marked = dispatcher
            .recover_silent_tasks("2026-10-04T09:01:00.000Z")
            .unwrap();
        assert_eq!(marked, vec![id.clone()]);
        assert_eq!(
            dispatcher.store().a2a_task_status(&id).unwrap(),
            Some(TaskStatus::NeedsHuman),
            "zombi görev kalmamalı"
        );
    }

    fn sample_worker(action: &str) -> crate::kernel::worker_registry::WorkerRegistration {
        crate::kernel::worker_registry::WorkerRegistration {
            bot_id: "grok-tester".into(),
            name: "Grok-Tester".into(),
            capabilities: vec!["echo".into()],
            version: "0.1.0".into(),
            pid: 42,
            action: action.into(),
            created_at: Some("2026-10-04T12:00:00.000Z".into()),
        }
    }

    #[tokio::test]
    async fn delegated_heartbeat_and_complete_avoids_needs_human() {
        let store = ExperienceStore::memory().unwrap();
        let workers = WorkerRegistry::with_store(store.clone());
        workers
            .apply_registration(sample_worker("register"))
            .unwrap();
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "dispatch".into(),
            is_code_analysis: false,
            reason: "delegate".into(),
            adr_summary: "delegate".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: Some("grok-tester".into()),
            repo_path: None,
        })
        .with_workers(workers.clone())
        .with_agent_silence(Duration::from_secs(60));

        let mut task = LoungeTask::new("a", "p", "delegate-me");
        task.target_agent = Some("grok-tester".into());
        task.idempotency_key = Some("deleg-hb-key".into());
        let id = task.id.clone();
        let err = dispatcher.handle_task(task).await.unwrap_err().to_string();
        assert!(
            err.contains("delegasyon"),
            "nc=None delegasyon beklenir: {err}"
        );
        assert_eq!(
            dispatcher.store().a2a_task_status(&id).unwrap(),
            Some(TaskStatus::Dispatched)
        );

        // Heartbeat → updated_at yenilenir; sessizlik eşiği aşılmaz.
        workers
            .apply_registration(sample_worker("heartbeat"))
            .unwrap();
        let marked = dispatcher
            .recover_silent_tasks("2026-10-04T12:00:30.000Z")
            .unwrap();
        assert!(
            !marked.contains(&id),
            "heartbeat sonrası NEEDS_HUMAN olmamalı: {marked:?}"
        );

        // Worker tamamladı.
        let payload = serde_json::json!({ "id": id, "type": "task" });
        dispatcher
            .apply_bus_terminal(payload.to_string().as_bytes(), false)
            .unwrap();
        assert_eq!(
            dispatcher.store().a2a_task_status(&id).unwrap(),
            Some(TaskStatus::Completed)
        );
        let marked = dispatcher
            .recover_silent_tasks("2026-10-04T12:05:00.000Z")
            .unwrap();
        assert!(
            !marked.contains(&id),
            "COMPLETED sessizlik taramasına girmez"
        );
    }

    #[tokio::test]
    async fn delegated_silent_becomes_needs_human() {
        let store = ExperienceStore::memory().unwrap();
        let workers = WorkerRegistry::with_store(store.clone());
        workers
            .apply_registration(sample_worker("register"))
            .unwrap();
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "dispatch".into(),
            is_code_analysis: false,
            reason: "delegate".into(),
            adr_summary: "delegate".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: Some("grok-tester".into()),
            repo_path: None,
        })
        .with_workers(workers)
        .with_agent_silence(Duration::from_secs(60));

        let mut task = LoungeTask::new("a", "p", "silent-delegate");
        task.target_agent = Some("grok-tester".into());
        let id = task.id.clone();
        let _ = dispatcher.handle_task(task).await;
        assert_eq!(
            dispatcher.store().a2a_task_status(&id).unwrap(),
            Some(TaskStatus::Dispatched)
        );
        dispatcher
            .store()
            .touch_a2a_updated_at(&id, "2026-10-04T10:00:00.000Z")
            .unwrap();
        let marked = dispatcher
            .recover_silent_tasks("2026-10-04T10:02:00.000Z")
            .unwrap();
        assert_eq!(marked, vec![id.clone()]);
        assert_eq!(
            dispatcher.store().a2a_task_status(&id).unwrap(),
            Some(TaskStatus::NeedsHuman)
        );
    }

    async fn assert_fail_releases_key(dispatcher: &Dispatcher, task: LoungeTask, key: &str) {
        let id = task.id.clone();
        let err = dispatcher.handle_task(task).await.unwrap_err();
        assert!(
            dispatcher.store().a2a_task_status(&id).unwrap() == Some(TaskStatus::Failed)
                || err.to_string().contains("idempotency"),
            "status Failed beklenir ({id}): {err}"
        );
        assert_eq!(
            dispatcher.store().a2a_task_status(&id).unwrap(),
            Some(TaskStatus::Failed)
        );
        let mut retry = LoungeTask::new("a", "p", "retry-after-fail");
        retry.idempotency_key = Some(key.into());
        match dispatcher.store().admit_a2a_task(&mut retry, 10).unwrap() {
            AdmitOutcome::Accepted(_) => {}
            other => panic!("anahtar serbest olmalı, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn error_paths_fail_and_release_idempotency() {
        for (kind, key) in [
            ("analyze", "err-analyze"),
            ("route_stop", "err-stop"),
            ("publish", "err-publish"),
            ("experience", "err-exp"),
            ("nats_err", "err-nats"),
        ] {
            let dispatcher = dispatcher(AnalysisDecision {
                intent: "acknowledge".into(),
                is_code_analysis: false,
                reason: kind.into(),
                adr_summary: kind.into(),
                outcome: Some(ExperienceOutcome::Success),
                target_agent: None,
                repo_path: None,
            })
            .with_test_fail_after_admit(kind);
            let mut task = LoungeTask::new("a", "p", format!("fail-{kind}"));
            task.idempotency_key = Some(key.into());
            assert_fail_releases_key(&dispatcher, task, key).await;
        }
    }

    #[tokio::test]
    async fn worker_offline_fails_and_releases_idempotency() {
        let workers = WorkerRegistry::new();
        workers
            .apply_registration(sample_worker("register"))
            .unwrap();
        workers
            .apply_registration(sample_worker("unregister"))
            .unwrap();
        assert!(workers.has_worker("grok-tester"));
        assert!(!workers.is_online("grok-tester"));

        let dispatcher = dispatcher(AnalysisDecision {
            intent: "dispatch".into(),
            is_code_analysis: false,
            reason: "offline".into(),
            adr_summary: "offline".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: Some("grok-tester".into()),
            repo_path: None,
        })
        .with_workers(workers);

        let mut task = LoungeTask::new("a", "p", "offline-target");
        task.target_agent = Some("grok-tester".into());
        task.idempotency_key = Some("offline-key".into());
        let err = dispatcher
            .handle_task(task.clone())
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("çevrimdışı") || err.contains("heartbeat"),
            "unexpected: {err}"
        );
        assert_eq!(
            dispatcher.store().a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::Failed)
        );
        let mut retry = LoungeTask::new("a", "p", "retry-offline");
        retry.idempotency_key = Some("offline-key".into());
        assert!(matches!(
            dispatcher.store().admit_a2a_task(&mut retry, 10).unwrap(),
            AdmitOutcome::Accepted(_)
        ));
    }

    #[tokio::test]
    async fn route_stop_fails_and_releases_idempotency() {
        use crate::models::{QuotaExhaustedAction, RoutingPolicy};

        let store = ExperienceStore::memory().unwrap();
        let policy = RoutingPolicy {
            on_quota_exhausted: QuotaExhaustedAction::Stop,
            ..RoutingPolicy::default()
        };
        store.set_routing_policy(&policy).await.unwrap();

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
            reason: "stop".into(),
            adr_summary: "stop".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        })
        .with_skip_quota_probe()
        .with_force_quota_blocked();

        let mut task = LoungeTask::new("a", "p", "quota-stop");
        task.idempotency_key = Some("stop-key".into());
        let id = task.id.clone();
        let err = dispatcher.handle_task(task).await.unwrap_err().to_string();
        assert!(
            err.contains("kotası") || err.contains("engellendi"),
            "RouteIntent::Stop beklenir: {err}"
        );
        assert_eq!(
            dispatcher.store().a2a_task_status(&id).unwrap(),
            Some(TaskStatus::Failed)
        );
        let mut retry = LoungeTask::new("a", "p", "retry-stop");
        retry.idempotency_key = Some("stop-key".into());
        assert!(matches!(
            dispatcher.store().admit_a2a_task(&mut retry, 10).unwrap(),
            AdmitOutcome::Accepted(_)
        ));
    }

    #[tokio::test]
    async fn bus_terminal_failed_releases_idempotency() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "bus".into(),
            adr_summary: "bus".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });
        let mut task = LoungeTask::new("a", "p", "bus-fail");
        task.idempotency_key = Some("bus-fail-key".into());
        dispatcher.handle_task(task.clone()).await.unwrap();
        // Completed → Failed yasak
        let payload = serde_json::json!({ "id": task.id, "type": "task" });
        let err = dispatcher
            .apply_bus_terminal(payload.to_string().as_bytes(), true)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("geçiş reddedildi") || err.contains("terminal"),
            "Completed→Failed reddedilmeli: {err}"
        );
        assert_eq!(
            dispatcher.store().a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::Completed)
        );
        // Anahtar hâlâ yanık
        let mut t2 = LoungeTask::new("a", "p", "still-held");
        t2.idempotency_key = Some("bus-fail-key".into());
        assert!(matches!(
            dispatcher.store().admit_a2a_task(&mut t2, 10).unwrap(),
            AdmitOutcome::Replay { .. }
        ));

        // İzinli yol: Dispatched → Failed → anahtar serbest
        let mut t3 = LoungeTask::new("a", "p", "dispatched-fail");
        t3.idempotency_key = Some("bus-disp-fail".into());
        dispatcher.handle_task(t3.clone()).await.unwrap();
        dispatcher
            .store()
            .set_a2a_task_status(&t3.id, TaskStatus::Dispatched)
            .unwrap();
        dispatcher
            .apply_bus_terminal(
                serde_json::json!({ "id": t3.id }).to_string().as_bytes(),
                true,
            )
            .unwrap();
        assert_eq!(
            dispatcher.store().a2a_task_status(&t3.id).unwrap(),
            Some(TaskStatus::Failed)
        );
        let mut t4 = LoungeTask::new("a", "p", "after-disp-fail");
        t4.idempotency_key = Some("bus-disp-fail".into());
        assert!(matches!(
            dispatcher.store().admit_a2a_task(&mut t4, 10).unwrap(),
            AdmitOutcome::Accepted(_)
        ));
    }

    #[tokio::test]
    async fn bus_terminal_rejects_completed_to_failed() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "term".into(),
            adr_summary: "term".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });
        let task = LoungeTask::new("a", "p", "done");
        dispatcher.handle_task(task.clone()).await.unwrap();
        let err = dispatcher
            .apply_bus_terminal(
                serde_json::json!({ "id": task.id }).to_string().as_bytes(),
                true,
            )
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("geçiş reddedildi") || err.contains("COMPLETED"),
            "unexpected: {err}"
        );
    }

    #[tokio::test]
    async fn bus_terminal_echo_completed_is_noop_error() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "echo".into(),
            adr_summary: "echo".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });
        let task = LoungeTask::new("a", "p", "local-done");
        dispatcher.handle_task(task.clone()).await.unwrap();
        let err = dispatcher
            .apply_bus_terminal(
                serde_json::json!({ "id": task.id }).to_string().as_bytes(),
                false,
            )
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("lifecycle_echo_completed"),
            "Completed→Completed yankı: {err}"
        );
        assert_eq!(
            dispatcher.store().a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::Completed)
        );
    }

    #[tokio::test]
    async fn needs_human_allows_late_worker_completion() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "late".into(),
            adr_summary: "late".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });
        let task = LoungeTask::new("a", "p", "zombie-then-done");
        dispatcher.handle_task(task.clone()).await.unwrap();
        dispatcher
            .store()
            .set_a2a_task_status(&task.id, TaskStatus::NeedsHuman)
            .unwrap();
        dispatcher
            .apply_bus_terminal(
                serde_json::json!({
                    "id": task.id,
                    "result": {"late": true}
                })
                .to_string()
                .as_bytes(),
                false,
            )
            .unwrap();
        assert_eq!(
            dispatcher.store().a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::Completed)
        );
        assert_eq!(
            dispatcher
                .store()
                .a2a_task_result(&task.id)
                .unwrap()
                .as_deref(),
            Some(r#"{"late":true}"#)
        );
    }

    #[tokio::test]
    async fn bus_terminal_writes_result_with_status_atomically() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "atom".into(),
            adr_summary: "atom".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });
        let task = LoungeTask::new("a", "p", "with-result");
        dispatcher.handle_task(task.clone()).await.unwrap();
        dispatcher
            .store()
            .set_a2a_task_status(&task.id, TaskStatus::Dispatched)
            .unwrap();
        dispatcher
            .apply_bus_terminal(
                serde_json::json!({
                    "id": task.id,
                    "result": {"v": 1}
                })
                .to_string()
                .as_bytes(),
                false,
            )
            .unwrap();
        assert_eq!(
            dispatcher.store().a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::Completed)
        );
        let result = dispatcher.store().a2a_task_result(&task.id).unwrap();
        assert!(
            result.as_deref().is_some_and(|s| s.contains("\"v\":1")),
            "result_json must be present with Completed: {result:?}"
        );
    }

    #[test]
    fn id_conflict_recovery_forces_unverified() {
        let store = ExperienceStore::memory().unwrap();
        let mut existing = LoungeTask::new("mcp:cursor", "p", "pre-admit");
        existing.session_id = Some("sess-1".into());
        existing.source_verified = true;
        store.admit_a2a_task(&mut existing, 10).unwrap();

        let mut inbound = existing.clone();
        stamp_external_nats_ingress(&mut inbound);
        assert!(!inbound.source_verified);

        // Aynı id → IdConflict
        let err = store.admit_a2a_task(&mut inbound, 10).unwrap_err();
        assert!(matches!(err, AdmitError::IdConflict { .. }));

        // Dispatcher recovery: existing yükle + NATS stamp false (verified yükseltme yok).
        let loaded = store.load_a2a_task(&existing.id).unwrap().unwrap();
        assert_eq!(loaded.session_id.as_deref(), Some("sess-1"));
        let mut recovered = loaded;
        recovered.source_verified = false;
        assert!(
            !recovered.source_verified,
            "IdConflict sonrası NATS yolu verified kalmamalı"
        );
    }

    #[test]
    fn apply_control_stop_cancels_task() {
        let dispatcher = dispatcher(AnalysisDecision {
            intent: "acknowledge".into(),
            is_code_analysis: false,
            reason: "stop".into(),
            adr_summary: "stop".into(),
            outcome: Some(ExperienceOutcome::Success),
            target_agent: None,
            repo_path: None,
        });
        let mut task = LoungeTask::new("mcp:cursor", "p", "stop-me");
        task.session_id = Some("sess-stop".into());
        dispatcher.store().admit_a2a_task(&mut task, 10).unwrap();
        dispatcher
            .apply_control_stop(
                serde_json::json!({
                    "task_id": task.id,
                    "session_id": "sess-stop",
                    "reason": "context canceled"
                })
                .to_string()
                .as_bytes(),
            )
            .unwrap();
        assert_eq!(
            dispatcher.store().a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::Cancelled)
        );
    }

    #[tokio::test]
    async fn security_approval_stays_pending_not_needs_human() {
        let dispatcher =
            live_dispatcher(Duration::from_secs(30)).with_agent_silence(Duration::from_secs(60));
        dispatcher.set_gate_ready(true);
        let task = LoungeTask::new("cursor", "agent-lounge-os", "wipe secrets critical");
        let id = task.id.clone();
        dispatcher.inject_decision(critical_decision(&id));
        let d = dispatcher.clone();
        let handle = tokio::spawn(async move { d.handle_task(task).await });

        let started = std::time::Instant::now();
        while !dispatcher.has_pending(&id) {
            assert!(started.elapsed() < Duration::from_secs(10));
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        assert_eq!(
            dispatcher.store().a2a_task_status(&id).unwrap(),
            Some(TaskStatus::PendingApproval),
            "security onayı beklerken PENDING_APPROVAL"
        );
        // Eski updated_at simüle et — silence eşiği aşılsın
        dispatcher
            .store()
            .touch_a2a_updated_at(&id, "2026-10-04T10:00:00.000Z")
            .unwrap();
        let marked = dispatcher
            .recover_silent_tasks("2026-10-04T10:05:00.000Z")
            .unwrap();
        assert!(
            !marked.contains(&id),
            "PENDING_APPROVAL NEEDS_HUMAN olmamalı: {marked:?}"
        );
        assert_eq!(
            dispatcher.store().a2a_task_status(&id).unwrap(),
            Some(TaskStatus::PendingApproval)
        );
        dispatcher.resolve_vote(id, RoutingVote::Deny).unwrap();
        let _ = handle.await;
    }
}
