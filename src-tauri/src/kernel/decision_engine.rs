//! Native Rust DecisionGate: her NATS `LoungeMessage` için Laya Fast Inference.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use lounge_protocol::LoungeMessage;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::laya::{
    build_sequence, confidence_from_probs, softmax_temp, temperature_for, LayaRuntimeConfig,
    LayaSession, PackedQuestion, QuestionSpec, TokenEncode, QTYPE_CHOICE, QTYPE_NOUL,
};
use crate::services::lounge_laya_dir;

const JOB_CAP: usize = 32;
const CACHE_TTL: Duration = Duration::from_secs(30);
const CONFIDENCE_SKIP_OLLAMA: f32 = 0.7;
const KNOWLEDGE_HIT_LOW: f32 = 0.3;
const MIN_RAM_BYTES: u64 = 1610612736; // 1.5 GiB
const HF_REPO: &str = "convaiinnovations/laya";

pub const ROUTING_ID: &str = "ROUTING_TYPE";
pub const SECURITY_ID: &str = "SECURITY_LEVEL";
pub const KNOWLEDGE_ID: &str = "KNOWLEDGE_HIT";
pub const DECISION_GATE_EVENT: &str = "decision-gate";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RoutingType {
    Task,
    Experience,
    Review,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SecurityLevel {
    Safe,
    Risky,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scored<T> {
    pub value: T,
    pub confidence: f32,
    pub probabilities: HashMap<String, f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DecisionResult {
    pub message_id: String,
    pub subject: String,
    pub routing: Scored<RoutingType>,
    pub security: Scored<SecurityLevel>,
    pub knowledge_hit: f32,
    pub elapsed_ms: u128,
    pub device: String,
}

impl DecisionResult {
    pub fn confident(&self) -> bool {
        self.routing.confidence >= CONFIDENCE_SKIP_OLLAMA
            && self.security.confidence >= CONFIDENCE_SKIP_OLLAMA
    }

    pub fn knowledge_is_low(&self) -> bool {
        self.knowledge_hit < KNOWLEDGE_HIT_LOW
    }
}

pub type DecisionCache = Arc<StdMutex<HashMap<String, (Instant, DecisionResult)>>>;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionGatePhase {
    Loading,
    Ready,
    Failed,
    Available,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DecisionGateStatus {
    pub phase: DecisionGatePhase,
    pub title: String,
    pub message: String,
    pub detail: Option<String>,
    pub device: Option<String>,
    pub reason: Option<String>,
}

impl DecisionGateStatus {
    pub fn loading() -> Self {
        Self {
            phase: DecisionGatePhase::Loading,
            title: "OpenJev Laya yükleniyor".into(),
            message: "Karar motoru ağırlıkları okunuyor. Bu sırada kernel LMR ile çalışır.".into(),
            detail: None,
            device: None,
            reason: None,
        }
    }

    pub fn ready(device: impl Into<String>) -> Self {
        let device = device.into();
        Self {
            phase: DecisionGatePhase::Ready,
            title: "OpenJev Laya hazır".into(),
            message: format!("DecisionGate {device} üzerinde çalışıyor."),
            detail: None,
            device: Some(device),
            reason: None,
        }
    }

    pub fn available() -> Self {
        Self {
            phase: DecisionGatePhase::Available,
            title: "OpenJev Laya kullanılabilir".into(),
            message: "Bellek açıldı. Karar motoruna geçiş yapmak ister misiniz? Onaylamadan mevcut LMR akışı değişmez.".into(),
            detail: None,
            device: None,
            reason: Some("ram".into()),
        }
    }

    pub fn deferred() -> Self {
        Self {
            phase: DecisionGatePhase::Failed,
            title: "OpenJev Laya ertelendi".into(),
            message: "Laya'ya geçiş reddedildi. Kernel LMR ile akışa devam ediyor.".into(),
            detail: None,
            device: None,
            reason: Some("ram".into()),
        }
    }

    pub fn ram_blocked(&self) -> bool {
        self.reason.as_deref() == Some("ram")
    }
}

pub fn classify_load_reason(err: &str) -> &'static str {
    let lower = err.to_lowercase();
    if lower.contains("yetersiz ram") {
        "ram"
    } else if lower.contains("ağırlık") {
        "weights"
    } else if lower.contains("hf ")
        || lower.contains("hugging face")
        || lower.contains("huggingface")
    {
        "download"
    } else {
        "other"
    }
}

pub fn status_from_load_error(err: &str) -> DecisionGateStatus {
    let reason = classify_load_reason(err);
    let (title, message) = match reason {
        "ram" => (
            "OpenJev Laya yüklenemedi".into(),
            "Karar motoru için en az 1.5 GiB boş bellek yok. Yönlendirme LMR ve sezgisel kurallarla sürüyor."
                .into(),
        ),
        "weights" => (
            "OpenJev Laya ağırlıkları yok".into(),
            "Gerekli model dosyaları bulunamadı. DecisionGate kapalı; kernel LMR ile çalışıyor."
                .into(),
        ),
        "download" => (
            "OpenJev Laya indirilemedi".into(),
            "Hugging Face deposundan convaiinnovations/laya alınamadı. Ağ veya kimlik bilgisi gerekebilir."
                .into(),
        ),
        _ => (
            "OpenJev Laya yüklenemedi".into(),
            "Karar motoru başlatılamadı. Kernel LMR ve mevcut kurallarla çalışmaya devam ediyor."
                .into(),
        ),
    };
    let detail = err.trim();
    DecisionGateStatus {
        phase: DecisionGatePhase::Failed,
        title,
        message,
        detail: if detail.is_empty() {
            None
        } else {
            Some(detail.to_string())
        },
        device: None,
        reason: Some(reason.into()),
    }
}

#[derive(Clone)]
pub struct DecisionGate {
    jobs: std_mpsc::SyncSender<LoungeMessage>,
    session: Arc<StdMutex<Option<LayaSession>>>,
    cache: DecisionCache,
    results: mpsc::Sender<DecisionResult>,
    status: Arc<StdMutex<DecisionGateStatus>>,
    declined: Arc<AtomicBool>,
    load_in_flight: Arc<AtomicBool>,
}

impl DecisionGate {
    pub fn new(results: mpsc::Sender<DecisionResult>) -> Self {
        let (jobs, job_rx) = std_mpsc::sync_channel(JOB_CAP);
        let session = Arc::new(StdMutex::new(None));
        let cache: DecisionCache = Arc::new(StdMutex::new(HashMap::new()));
        let status = Arc::new(StdMutex::new(DecisionGateStatus::loading()));
        let declined = Arc::new(AtomicBool::new(false));
        let load_in_flight = Arc::new(AtomicBool::new(false));
        let worker_session = session.clone();
        let worker_cache = cache.clone();
        let worker_tx = results.clone();
        let _ = std::thread::Builder::new()
            .name("laya-infer".into())
            .spawn(move || infer_worker(job_rx, worker_session, worker_cache, worker_tx));
        Self {
            jobs,
            session,
            cache,
            results,
            status,
            declined,
            load_in_flight,
        }
    }

    pub fn cache(&self) -> DecisionCache {
        self.cache.clone()
    }

    pub fn results_sender(&self) -> mpsc::Sender<DecisionResult> {
        self.results.clone()
    }

    pub fn install(&self, session: LayaSession) {
        log::info!(
            "DecisionGate yüklendi ({}, max_len={})",
            session.device_name,
            session.cfg.max_len
        );
        self.declined.store(false, Ordering::SeqCst);
        self.load_in_flight.store(false, Ordering::SeqCst);
        self.set_status(DecisionGateStatus::ready(&session.device_name));
        *self.session.lock().expect("laya session lock") = Some(session);
    }

    pub fn fail_load(&self, err: &str) {
        let status = status_from_load_error(err);
        log::warn!(
            "DecisionGate soğuk: {}",
            status.detail.as_deref().unwrap_or(err)
        );
        self.load_in_flight.store(false, Ordering::SeqCst);
        self.set_status(status);
    }

    pub fn begin_load(&self) -> bool {
        if self.is_ready() {
            return false;
        }
        if self
            .load_in_flight
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return false;
        }
        self.declined.store(false, Ordering::SeqCst);
        self.set_status(DecisionGateStatus::loading());
        true
    }

    pub fn mark_available(&self) -> bool {
        if self.declined.load(Ordering::SeqCst) || self.is_ready() {
            return false;
        }
        let status = self.status();
        if status.phase == DecisionGatePhase::Available {
            return false;
        }
        if status.phase != DecisionGatePhase::Failed || !status.ram_blocked() {
            return false;
        }
        self.set_status(DecisionGateStatus::available());
        true
    }

    pub fn decline_offer(&self) {
        self.declined.store(true, Ordering::SeqCst);
        self.set_status(DecisionGateStatus::deferred());
    }

    pub fn clear_declined(&self) {
        self.declined.store(false, Ordering::SeqCst);
    }

    pub fn declined(&self) -> bool {
        self.declined.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> DecisionGateStatus {
        self.status.lock().expect("laya status lock").clone()
    }

    fn set_status(&self, status: DecisionGateStatus) {
        *self.status.lock().expect("laya status lock") = status;
    }

    pub fn is_ready(&self) -> bool {
        self.session.lock().expect("laya session lock").is_some()
    }

    pub fn infer_async(&self, msg: &LoungeMessage) {
        if skip_bus_subject(&msg.subject) {
            return;
        }
        if let Err(err) = self.jobs.try_send(msg.clone()) {
            log::debug!("DecisionGate kuyruk dolu: {err}");
        }
    }

    pub fn inject(&self, result: DecisionResult) {
        remember(&self.cache, &result);
        let _ = self.results.try_send(result);
    }
}

pub fn skip_bus_subject(subject: &str) -> bool {
    subject.starts_with("lounge.bus.")
}

pub fn lookup_decision(cache: &DecisionCache, id: &str) -> Option<DecisionResult> {
    let mut map = cache.lock().expect("decision cache");
    let now = Instant::now();
    map.retain(|_, (at, _)| now.duration_since(*at) < CACHE_TTL);
    map.get(id).map(|(_, result)| result.clone())
}

pub fn remember(cache: &DecisionCache, result: &DecisionResult) {
    let mut map = cache.lock().expect("decision cache");
    let now = Instant::now();
    map.insert(result.message_id.clone(), (now, result.clone()));
}

pub fn state_from_message(msg: &LoungeMessage) -> String {
    let payload = match &msg.payload {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let mut payload = payload;
    if payload.len() > 1500 {
        payload.truncate(1500);
    }
    format!(
        "subject={} type={} source={} payload={}",
        msg.subject, msg.msg_type, msg.source_agent, payload
    )
}

pub fn lounge_questions() -> [QuestionSpec; 3] {
    [
        QuestionSpec {
            id: ROUTING_ID,
            qtype: QTYPE_CHOICE,
            instructions: "Classify this Agent Lounge OS bus message.",
            options: vec![
                ("Task", Some("a work order to execute")),
                ("Experience", Some("a remembered outcome or ADR")),
                ("Review", Some("a code or design review")),
            ],
        },
        QuestionSpec {
            id: SECURITY_ID,
            qtype: QTYPE_CHOICE,
            instructions: "How risky is acting on this message without a human in the loop?",
            options: vec![
                ("Safe", Some("routine, no secrets or destructive actions")),
                (
                    "Risky",
                    Some("needs care: credentials, production, or irreversible edits"),
                ),
                (
                    "Critical",
                    Some("exfiltration, privilege, wipe, or untrusted takeover"),
                ),
            ],
        },
        QuestionSpec {
            id: KNOWLEDGE_ID,
            qtype: QTYPE_NOUL,
            instructions: "Is a similar record already in Lounge memory for this topic?",
            options: vec![("Miss", None), ("Hit", None)],
        },
    ]
}

pub fn pack_message(
    tok: &impl TokenEncode,
    msg: &LoungeMessage,
    cfg: &LayaRuntimeConfig,
) -> Vec<PackedQuestion> {
    let state = state_from_message(msg);
    lounge_questions()
        .iter()
        .map(|q| build_sequence(tok, &state, q, cfg.max_len, cfg.head_max_len))
        .collect()
}

pub fn result_from_logits(
    msg: &LoungeMessage,
    packed: &[PackedQuestion],
    logits: &[Vec<f32>],
    cfg: &LayaRuntimeConfig,
    elapsed_ms: u128,
    device: &str,
) -> Result<DecisionResult> {
    let mut routing = None;
    let mut security = None;
    let mut knowledge_hit = 0.0;
    for (item, z) in packed.iter().zip(logits.iter()) {
        let k = item.markers.len().min(z.len());
        let slice = &z[..k];
        let temp = temperature_for(cfg, item.qtype, k);
        let probs = softmax_temp(slice, temp);
        match item.id.as_str() {
            ROUTING_ID => {
                routing = Some(scored_routing(&item.option_keys, &probs));
            }
            SECURITY_ID => {
                security = Some(scored_security(&item.option_keys, &probs));
            }
            KNOWLEDGE_ID => {
                knowledge_hit = probs.get(1).copied().unwrap_or(0.0);
            }
            _ => {}
        }
    }
    Ok(DecisionResult {
        message_id: msg.id.clone(),
        subject: msg.subject.clone(),
        routing: routing.context("ROUTING_TYPE yok")?,
        security: security.context("SECURITY_LEVEL yok")?,
        knowledge_hit,
        elapsed_ms,
        device: device.into(),
    })
}

fn scored_routing(keys: &[String], probs: &[f32]) -> Scored<RoutingType> {
    let probabilities = zip_probs(keys, probs);
    let (label, _) = argmax(&probabilities);
    let value = match label.as_str() {
        "Experience" => RoutingType::Experience,
        "Review" => RoutingType::Review,
        _ => RoutingType::Task,
    };
    Scored {
        value,
        confidence: confidence_from_probs(probs),
        probabilities,
    }
}

fn scored_security(keys: &[String], probs: &[f32]) -> Scored<SecurityLevel> {
    let probabilities = zip_probs(keys, probs);
    let (label, _) = argmax(&probabilities);
    let value = match label.as_str() {
        "Risky" => SecurityLevel::Risky,
        "Critical" => SecurityLevel::Critical,
        _ => SecurityLevel::Safe,
    };
    Scored {
        value,
        confidence: confidence_from_probs(probs),
        probabilities,
    }
}

fn zip_probs(keys: &[String], probs: &[f32]) -> HashMap<String, f32> {
    keys.iter().cloned().zip(probs.iter().copied()).collect()
}

fn argmax(map: &HashMap<String, f32>) -> (String, f32) {
    map.iter()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(k, v)| (k.clone(), *v))
        .unwrap_or_else(|| ("Task".into(), 0.0))
}

fn infer_worker(
    jobs: std_mpsc::Receiver<LoungeMessage>,
    session: Arc<StdMutex<Option<LayaSession>>>,
    cache: DecisionCache,
    tx: mpsc::Sender<DecisionResult>,
) {
    while let Ok(msg) = jobs.recv() {
        let started = Instant::now();
        let inferred = {
            let guard = session.lock().expect("laya session lock");
            let Some(sess) = guard.as_ref() else {
                continue;
            };
            match infer_message(sess, &msg) {
                Ok(result) => result,
                Err(err) => {
                    log::warn!("DecisionGate çıkarsama: {err}");
                    continue;
                }
            }
        };
        let mut result = inferred;
        result.elapsed_ms = started.elapsed().as_millis();
        if result.elapsed_ms > 10 {
            log::debug!(
                "DecisionGate {} {}ms (hedef 10ms, device={})",
                result.message_id,
                result.elapsed_ms,
                result.device
            );
        }
        remember(&cache, &result);
        if let Some(payload_id) = msg.payload.get("id").and_then(|v| v.as_str()) {
            if payload_id != result.message_id {
                let mut extra = result.clone();
                extra.message_id = payload_id.into();
                remember(&cache, &extra);
            }
        }
        let _ = tx.blocking_send(result);
    }
}

fn infer_message(session: &LayaSession, msg: &LoungeMessage) -> Result<DecisionResult> {
    let packed = pack_message(&session.tokenizer, msg, &session.cfg);
    let logits = session.infer_batch(packed.clone())?;
    result_from_logits(msg, &packed, &logits, &session.cfg, 0, &session.device_name)
}

pub fn ram_allows_load() -> bool {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.available_memory() >= MIN_RAM_BYTES
}

pub fn laya_dir() -> PathBuf {
    std::env::var("LOUNGE_LAYA_DIR")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(lounge_laya_dir)
}

pub fn skip_download() -> bool {
    matches!(
        std::env::var("LOUNGE_LAYA_SKIP_DOWNLOAD").ok().as_deref(),
        Some("1") | Some("true") | Some("TRUE")
    )
}

pub fn ensure_artifacts(dir: &Path, download: bool) -> Result<()> {
    let weights = dir.join("model.safetensors");
    let tokenizer = dir.join("tokenizer/tokenizer.json");
    let encoder = dir.join("encoder/config.json");
    let cfg = dir.join("rl_agent_config.json");
    if weights.is_file() && tokenizer.is_file() && encoder.is_file() && cfg.is_file() {
        return Ok(());
    }
    if !download || skip_download() {
        anyhow::bail!("Laya ağırlıkları yok: {}", dir.display());
    }
    std::fs::create_dir_all(dir.join("tokenizer"))?;
    std::fs::create_dir_all(dir.join("encoder"))?;
    let api = hf_hub::api::sync::ApiBuilder::new()
        .with_cache_dir(dir.join(".hf"))
        .build()
        .context("Hugging Face API")?;
    let repo = api.model(HF_REPO.into());
    copy_hub(&repo, "model.safetensors", &weights)?;
    copy_hub(&repo, "tokenizer/tokenizer.json", &tokenizer)?;
    copy_hub(&repo, "encoder/config.json", &encoder)?;
    copy_hub(&repo, "rl_agent_config.json", &cfg)?;
    Ok(())
}

fn copy_hub(repo: &hf_hub::api::sync::ApiRepo, name: &str, dest: &Path) -> Result<()> {
    if dest.is_file() {
        return Ok(());
    }
    let src = repo.get(name).with_context(|| format!("HF get {name}"))?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&src, dest)
        .with_context(|| format!("kopyala {} → {}", src.display(), dest.display()))?;
    Ok(())
}

pub fn load_session(dir: &Path) -> Result<LayaSession> {
    if !ram_allows_load() {
        anyhow::bail!("Laya için yetersiz RAM (en az 1.5 GiB boş)");
    }
    ensure_artifacts(dir, true)?;
    LayaSession::load(dir)
}

pub fn security_approval(
    task_id: &str,
    summary: &str,
    from_agent: &str,
    decision: &DecisionResult,
) -> Option<crate::models::ApprovalRequest> {
    if decision.security.value != SecurityLevel::Critical {
        return None;
    }
    Some(crate::models::ApprovalRequest {
        task_id: task_id.into(),
        summary: summary.into(),
        from_agent: from_agent.into(),
        to_agent: crate::models::KERNEL_AGENT.into(),
        kind: crate::models::ApprovalKind::SecurityCritical,
        reason: format!(
            "DecisionGate SECURITY_LEVEL=Critical (p={:.2})",
            decision
                .security
                .probabilities
                .get("Critical")
                .copied()
                .unwrap_or(decision.security.confidence)
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::laya::HashTokenizer;
    use lounge_protocol::LoungeMessage;

    fn sample_msg() -> LoungeMessage {
        LoungeMessage::new(
            "lounge.task.requested",
            "cursor",
            serde_json::json!({
                "id": "task-1",
                "type": "task",
                "summary": "index kernel dispatcher"
            }),
        )
    }

    #[test]
    fn skips_bus_heartbeats() {
        assert!(skip_bus_subject("lounge.bus.heartbeat"));
        assert!(skip_bus_subject("lounge.bus.probe"));
        assert!(skip_bus_subject("lounge.bus.connected"));
        assert!(!skip_bus_subject("lounge.task.requested"));
        assert!(!skip_bus_subject("lounge.experience.reported"));
    }

    #[test]
    fn packs_three_typed_questions() {
        let packed = pack_message(
            &HashTokenizer::default(),
            &sample_msg(),
            &LayaRuntimeConfig::default(),
        );
        assert_eq!(packed.len(), 3);
        assert_eq!(packed[0].markers.len(), 3);
        assert_eq!(packed[1].markers.len(), 3);
        assert_eq!(packed[2].markers.len(), 2);
        assert_eq!(packed[0].id, ROUTING_ID);
    }

    #[test]
    fn builds_result_from_logits() {
        let msg = sample_msg();
        let packed = pack_message(
            &HashTokenizer::default(),
            &msg,
            &LayaRuntimeConfig::default(),
        );
        let logits = vec![vec![4.0, 0.1, 0.2], vec![0.1, 0.2, 5.0], vec![0.2, 3.0]];
        let result = result_from_logits(
            &msg,
            &packed,
            &logits,
            &LayaRuntimeConfig::default(),
            8,
            "cpu",
        )
        .unwrap();
        assert_eq!(result.routing.value, RoutingType::Task);
        assert_eq!(result.security.value, SecurityLevel::Critical);
        assert!(result.knowledge_hit > 0.8);
        assert_eq!(result.elapsed_ms, 8);
        assert_eq!(result.device, "cpu");
        let json = serde_json::to_value(&result).unwrap();
        let back: DecisionResult = serde_json::from_value(json).unwrap();
        assert_eq!(back.security.value, SecurityLevel::Critical);
    }

    #[test]
    fn critical_security_requests_approval() {
        let msg = sample_msg();
        let packed = pack_message(
            &HashTokenizer::default(),
            &msg,
            &LayaRuntimeConfig::default(),
        );
        let logits = vec![vec![1.0, 0.0, 0.0], vec![0.0, 0.0, 4.0], vec![1.0, 0.0]];
        let result = result_from_logits(
            &msg,
            &packed,
            &logits,
            &LayaRuntimeConfig::default(),
            4,
            "cpu",
        )
        .unwrap();
        let request = security_approval("task-1", "wipe secrets", "cursor", &result).unwrap();
        assert_eq!(request.kind, crate::models::ApprovalKind::SecurityCritical);
    }

    #[test]
    fn load_errors_are_user_facing() {
        let ram = status_from_load_error("Laya için yetersiz RAM (en az 1.5 GiB boş)");
        assert_eq!(ram.phase, DecisionGatePhase::Failed);
        assert_eq!(ram.reason.as_deref(), Some("ram"));
        assert!(ram.title.contains("OpenJev Laya"));
        assert!(ram.message.contains("1.5 GiB"));
        assert!(ram.detail.as_deref().unwrap().contains("yetersiz RAM"));

        let weights = status_from_load_error("Laya ağırlıkları yok: /tmp/laya");
        assert_eq!(weights.reason.as_deref(), Some("weights"));
        assert!(weights.message.contains("model dosyaları"));

        let hf = status_from_load_error("HF get model.safetensors");
        assert_eq!(hf.reason.as_deref(), Some("download"));
        assert!(hf.title.contains("indirilemedi"));
    }

    #[test]
    fn ram_recovery_asks_before_loading() {
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let gate = DecisionGate::new(tx);
        gate.fail_load("Laya için yetersiz RAM (en az 1.5 GiB boş)");
        assert!(!gate.is_ready());
        assert!(gate.mark_available());
        let offer = gate.status();
        assert_eq!(offer.phase, DecisionGatePhase::Available);
        assert!(offer.message.contains("geçiş"));
        assert!(!gate.is_ready());
        assert!(!gate.mark_available());
        gate.decline_offer();
        assert!(gate.declined());
        assert_eq!(gate.status().phase, DecisionGatePhase::Failed);
        assert!(!gate.mark_available());
        gate.clear_declined();
        assert!(gate.mark_available());
    }

    #[test]
    #[ignore]
    fn live_laya_infer() {
        let dir = std::env::var("LAYA_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| laya_dir());
        let session = LayaSession::load(&dir).expect("LAYA_MODEL_DIR");
        let msg = sample_msg();
        let packed = pack_message(&session.tokenizer, &msg, &session.cfg);
        let logits = session.infer_batch(packed.clone()).unwrap();
        let result = result_from_logits(
            &msg,
            &packed,
            &logits,
            &session.cfg,
            0,
            &session.device_name,
        )
        .unwrap();
        assert!(result.elapsed_ms < 60_000);
        assert!((0.0..=1.0).contains(&result.knowledge_hit));
    }
}
