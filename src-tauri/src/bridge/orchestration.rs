//! MCP Bridge v2 — Mod A→B async orchestration.
//!
//! `lounge_call_agent` / `delegate_and_wait`: `select!` ile `timeout_limit` ve
//! sonuç dinleyicisini bekler. Eşik dolarsa `{status: backgrounded, task_id}` —
//! istemci sert zaman aşımından **önce** dönmek esas.
//!
//! Sonuç teslimi (deadline exceeded sonrası): bağlantı açık kalır; ajan
//! `lounge_wait_task` ile aynı oturumda sonucu çeker (piggyback).
//!
//! Progress: yalnız `progress_extends` profillerinde (Cursor) ve istemci
//! `progressToken` gönderdiyse 10 sn heartbeat ile Mod A ≤280 sn tutulur.
//! Diğer istemcilerde progress süreyi uzatmaz.
//!
//! İptal: `notifications/cancelled` reason `context canceled` → görev iptal +
//! `lounge.control.stop`. `deadline exceeded` → görev arka planda sürer.
//! Oturum kopması (stdio EOF / HTTP DELETE): yalnız **in-flight** (senkron
//! beklenen) çağrılar iptal; backgrounded görevler korunur.
//!
//! NATS: paylaşılan [`NatsTerminalHub`] (çağrı başına connect yok). Sender
//! kapanınca select kolu devre dışı kalır (busy-loop yok).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::sync::{broadcast, watch};
use uuid::Uuid;

use super::nats_terminal::NatsTerminalHub;
use super::session_id::is_valid_session_id;
use super::timeout_manager::{profile_diag, TimeoutManager};
use super::wait_clock::WaitClock;
use crate::db::{
    configured_max_hops, generate_task_token, hash_task_token, AdmitOutcome, ExperienceStore,
};
use crate::models::{
    now_rfc3339, LoungeTask, TaskStatus, CONTROL_STOP, TASK_COMPLETED, TASK_FAILED, TASK_REQUESTED,
};

/// Progress heartbeat — testte sayaç; üretimde MCP `notifications/progress`.
pub trait ProgressSink: Send + Sync {
    fn emit_progress(&self, progress_token: &Value, message: &str, elapsed_secs: u64);
}

/// Sessiz (HTTP yanıt gövdesine yazılamayan) sink.
#[derive(Debug, Default)]
pub struct NoopProgressSink;

impl ProgressSink for NoopProgressSink {
    fn emit_progress(&self, _progress_token: &Value, _message: &str, _elapsed_secs: u64) {}
}

/// Test / teşhis sayacı.
#[derive(Debug, Default)]
pub struct CountingProgressSink {
    pub count: AtomicU64,
}

impl ProgressSink for CountingProgressSink {
    fn emit_progress(&self, _progress_token: &Value, _message: &str, _elapsed_secs: u64) {
        self.count.fetch_add(1, Ordering::SeqCst);
    }
}

/// Mod A bekleme seçenekleri.
#[derive(Clone)]
pub struct WaitOpts {
    pub progress_token: Option<Value>,
    pub progress_sink: Arc<dyn ProgressSink>,
}

impl Default for WaitOpts {
    fn default() -> Self {
        Self {
            progress_token: None,
            progress_sink: Arc::new(NoopProgressSink),
        }
    }
}

/// İstemci iptal nedeni ayrımı.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelKind {
    /// Kullanıcı Stop — görev öldürülür.
    UserStop,
    /// İstemci deadline — görev arka planda devam.
    DeadlineExceeded,
    /// Oturum koptu — must_deliver ise arka plana; değilse iptal.
    SessionDisconnect,
    /// Diğer / bilinmeyen.
    Other,
}

impl CancelKind {
    pub fn from_reason(reason: &str) -> Self {
        let lower = reason.to_ascii_lowercase();
        if lower.contains("context canceled") || lower.contains("context cancelled") {
            Self::UserStop
        } else if lower.contains("deadline exceeded") {
            Self::DeadlineExceeded
        } else {
            Self::Other
        }
    }
}

#[derive(Debug, Clone)]
pub struct CallAgentArgs {
    pub target_agent: String,
    pub task: String,
    pub project_id: String,
    pub idempotency_key: Option<String>,
    pub repo_path: Option<String>,
    pub parent_task_id: Option<String>,
    /// `true` → Mod A bekle; `false` → hemen task_id (dispatch uyumluluğu).
    pub wait: bool,
    /// Uzun iş — backgrounded sonrası poll_after ipucu.
    pub long_running: bool,
    /// Sonuç mutlaka teslim (kota + disconnect'te iptal yok; orphan EXPIRED yok).
    pub must_deliver: bool,
}

/// NATS / test yayın aracı — control.stop doğrulaması için enjekte edilir.
pub trait ControlStopPublisher: Send + Sync {
    fn publish_control_stop(&self, task_id: &str, session_id: &str) -> Result<()>;
}

struct NatsControlStopPublisher {
    nats_url: String,
}

impl ControlStopPublisher for NatsControlStopPublisher {
    fn publish_control_stop(&self, task_id: &str, session_id: &str) -> Result<()> {
        let url = self.nats_url.clone();
        let subject = CONTROL_STOP.to_string();
        let payload = json!({
            "task_id": task_id,
            "session_id": session_id,
            "reason": "context canceled",
            "at": now_rfc3339(),
            "id": Uuid::new_v4().to_string(),
        });
        let bytes = serde_json::to_vec(&payload)?;
        #[allow(deprecated)]
        let nc = crate::services::nats_connect(&url).map_err(|e| anyhow!("NATS connect: {e}"))?;
        nc.publish(&subject, bytes)
            .map_err(|e| anyhow!("NATS publish: {e}"))?;
        nc.flush().map_err(|e| anyhow!("NATS flush: {e}"))?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct Orchestrator {
    store: ExperienceStore,
    nats_url: String,
    timeouts: Arc<TimeoutManager>,
    clock: Arc<dyn WaitClock>,
    /// Testlerde NATS atlanır; yalnız DB poll.
    skip_nats: bool,
    terminal_hub: Arc<NatsTerminalHub>,
    control_publisher: Arc<dyn ControlStopPublisher>,
    /// `with_control_publisher` ile enjekte edildiyse skip_nats'ta da çağrılır.
    control_publisher_forced: bool,
}

impl Orchestrator {
    pub fn new(
        store: ExperienceStore,
        nats_url: impl Into<String>,
        timeouts: Arc<TimeoutManager>,
        clock: Arc<dyn WaitClock>,
    ) -> Self {
        let nats_url = nats_url.into();
        let terminal_hub = NatsTerminalHub::new(nats_url.clone());
        let control_publisher: Arc<dyn ControlStopPublisher> = Arc::new(NatsControlStopPublisher {
            nats_url: nats_url.clone(),
        });
        Self {
            store,
            nats_url,
            timeouts,
            clock,
            skip_nats: false,
            terminal_hub,
            control_publisher,
            control_publisher_forced: false,
        }
    }

    pub fn with_skip_nats(mut self, skip: bool) -> Self {
        self.skip_nats = skip;
        if skip {
            self.terminal_hub = NatsTerminalHub::inert();
        }
        self
    }

    pub fn with_terminal_hub(mut self, hub: Arc<NatsTerminalHub>) -> Self {
        self.terminal_hub = hub;
        self
    }

    pub fn with_control_publisher(mut self, pub_: Arc<dyn ControlStopPublisher>) -> Self {
        self.control_publisher = pub_;
        self.control_publisher_forced = true;
        self
    }

    pub fn skips_nats(&self) -> bool {
        self.skip_nats
    }

    pub fn timeouts(&self) -> &TimeoutManager {
        &self.timeouts
    }

    pub fn store(&self) -> &ExperienceStore {
        &self.store
    }

    /// Görev kabul + NATS yayın (+ isteğe bağlı Mod A bekleme).
    pub async fn call_agent(
        &self,
        session_id: &str,
        client_name: &str,
        args: CallAgentArgs,
        cancel_rx: Option<watch::Receiver<Option<CancelKind>>>,
    ) -> Result<Value> {
        self.call_agent_with_opts(
            session_id,
            client_name,
            args,
            cancel_rx,
            WaitOpts::default(),
        )
        .await
    }

    pub async fn call_agent_with_opts(
        &self,
        session_id: &str,
        client_name: &str,
        args: CallAgentArgs,
        mut cancel_rx: Option<watch::Receiver<Option<CancelKind>>>,
        opts: WaitOpts,
    ) -> Result<Value> {
        if !is_valid_session_id(session_id) {
            return Err(anyhow!("geçersiz session_id"));
        }
        let open = self
            .store
            .count_open_a2a_tasks_for_session(session_id)
            .unwrap_or(0);
        if open >= super::session_id::MAX_OPEN_TASKS_PER_SESSION as u64 {
            return Err(anyhow!(
                "oturum başına açık görev limiti aşıldı ({})",
                super::session_id::MAX_OPEN_TASKS_PER_SESSION
            ));
        }
        let has_progress = opts.progress_token.is_some();
        let timeout_limit = self
            .timeouts
            .timeout_limit_with_progress(client_name, has_progress);
        let diag = profile_diag(client_name, timeout_limit);
        let source = format!(
            "mcp:{}",
            super::mcp_server::normalize_client_host(client_name)
        );

        if args.must_deliver {
            // Idempotency replay kota yemez — yalnız yeni kabul öncesi kontrol.
            let peek_replay = if let Some(ref key) = args.idempotency_key {
                self.store
                    .peek_a2a_idempotency(session_id, &args.project_id, key)
                    .ok()
                    .flatten()
                    .is_some()
            } else {
                false
            };
            if !peek_replay {
                self.store
                    .check_must_deliver_quota(session_id, &source)
                    .map_err(|e| anyhow!(e.to_string()))?;
            }
        }

        let plaintext_token = generate_task_token();
        let token_hash = hash_task_token(&plaintext_token);

        let mut task = LoungeTask::new(source.clone(), args.project_id.clone(), args.task.clone());
        task.target_agent = Some(args.target_agent.clone());
        task.session_id = Some(session_id.to_string());
        // P1-1: token/parent-PID doğrulaması gelene kadar MCP görevleri unverified.
        task.source_verified = false;
        task.idempotency_key = args.idempotency_key.clone();
        task.repo_path = args.repo_path.clone();
        task.long_running = args.long_running;
        task.must_deliver = args.must_deliver;
        task.task_token_hash = Some(token_hash.clone());
        // parent_task_id istemci değeri yok sayılır — yalnız sunucu enjekte eder.
        let _ = args.parent_task_id;
        task.parent_task_id = None;

        let max_hops = configured_max_hops();
        let outcome = self
            .store
            .admit_a2a_task(&mut task, max_hops)
            .map_err(|e| anyhow!(e.to_string()))?;

        let (task_id, replay_status, issued_token) = match outcome {
            AdmitOutcome::Accepted(t) => (t.id, None, Some(plaintext_token)),
            AdmitOutcome::Replay {
                existing_task_id,
                status,
            } => {
                // İlk gelen kazanır — token yeniden üretilmez (must_deliver idempotency).
                if status.is_terminal() {
                    let result = self.store.a2a_task_result(&existing_task_id)?;
                    let mut body = json!({
                        "status": status.as_str().to_ascii_lowercase(),
                        "task_id": existing_task_id,
                        "replay": true,
                        "result": result.and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                        "timeout_limit_secs": timeout_limit.as_secs(),
                    });
                    merge_diag(&mut body, &diag);
                    return Ok(body);
                }
                (existing_task_id, Some(status), None)
            }
        };

        if !self.skip_nats {
            self.publish_task_requested(&task).await?;
        }

        if !args.wait {
            let poll_after = crate::db::next_poll_after_secs(0);
            let mut body = json!({
                "published": true,
                "task_id": task_id,
                "subject": TASK_REQUESTED,
                "target_agent": args.target_agent,
                "project_id": args.project_id,
                "summary": args.task,
                "source_agent": task.source_agent,
                "session_id": session_id,
                "source_verified": false,
                "timeout_limit_secs": timeout_limit.as_secs(),
                "long_running": args.long_running,
                "must_deliver": args.must_deliver,
                "poll_after_secs": poll_after,
                "next_action": crate::db::NEXT_ACTION_WAIT_TASK,
                "replay_status": replay_status.map(|s| s.as_str()),
                "note": "Görev NATS'a yazıldı. Sonuç için lounge_wait_task kullanın (Mod B). source_verified=false — onay kapısı uygulanabilir. Yeniden bağlanmada task_token saklayın."
            });
            if let Some(tok) = issued_token {
                body["task_token"] = json!(tok);
            }
            merge_diag(&mut body, &diag);
            return Ok(body);
        }

        // long_running: eşik beklemeden hemen backgrounded + task_id.
        if args.long_running {
            let _ = self.store.try_mark_a2a_wait_timeout(&task_id)?;
            let poll_after = crate::db::next_poll_after_secs(0);
            let mut body = json!({
                "status": "backgrounded",
                "task_id": task_id,
                "reason": "long_running",
                "poll_after_secs": poll_after,
                "next_action": crate::db::NEXT_ACTION_WAIT_TASK,
                "message": format!(
                    "long_running=true — eşik beklenmedi; ~{poll_after}sn sonra lounge_wait_task ile kontrol edin."
                ),
                "timeout_limit_secs": timeout_limit.as_secs(),
                "long_running": true,
                "must_deliver": args.must_deliver,
                "hint": "Call lounge_wait_task(task_id) on a schedule (poll_after_secs). Keep task_token if reconnecting."
            });
            if let Some(tok) = issued_token {
                body["task_token"] = json!(tok);
            }
            merge_diag(&mut body, &diag);
            return Ok(body);
        }

        let mut out = self
            .wait_until_done_or_background(
                &task_id,
                session_id,
                client_name,
                timeout_limit,
                cancel_rx.as_mut(),
                &opts,
                args.long_running,
            )
            .await?;
        if let Some(tok) = issued_token {
            if out.get("status").and_then(|s| s.as_str()) == Some("backgrounded") {
                out["task_token"] = json!(tok);
            }
        }
        out["long_running"] = json!(args.long_running);
        out["must_deliver"] = json!(args.must_deliver);
        merge_diag(&mut out, &diag);
        Ok(out)
    }

    #[allow(clippy::too_many_arguments)]
    async fn wait_until_done_or_background(
        &self,
        task_id: &str,
        session_id: &str,
        client_name: &str,
        timeout_limit: Duration,
        mut cancel_rx: Option<&mut watch::Receiver<Option<CancelKind>>>,
        opts: &WaitOpts,
        long_running: bool,
    ) -> Result<Value> {
        let poll = Duration::from_millis(50);
        let start_ms = self.clock.now_ms();
        let deadline_ms = start_ms.saturating_add(timeout_limit.as_millis() as u64);
        let profile = self.timeouts.profile_for(client_name);
        let heartbeat = if opts.progress_token.is_some() && profile.progress_extends {
            Duration::from_secs(profile.progress_heartbeat_secs.unwrap_or(10))
        } else {
            Duration::from_secs(u64::MAX / 4) // fiilen kapalı
        };
        let mut next_heartbeat_ms = start_ms.saturating_add(heartbeat.as_millis() as u64);

        let mut nats_rx = self.subscribe_terminal(task_id);
        // Sender kapandıysa kolu tamamen bırak (pending future → CPU yok).
        let mut nats_alive = !self.skip_nats;

        loop {
            // Cancel önce — progress heartbeat cancelled işlemeyi geciktirmesin.
            let pending_cancel = cancel_rx.as_ref().and_then(|rx| *rx.borrow());
            if let Some(kind) = pending_cancel {
                return self.handle_cancel(task_id, session_id, kind).await;
            }

            if let Some(payload) = self.try_read_result(task_id, session_id)? {
                return Ok(payload);
            }

            let now = self.clock.now_ms();
            if now >= deadline_ms {
                // Atomik: sonuç ile backgrounded arasında tek kazanan.
                if let Some(payload) = self.try_read_result(task_id, session_id)? {
                    return Ok(payload);
                }
                let won = self.store.try_mark_a2a_wait_timeout(task_id)?;
                if !won {
                    if let Some(payload) = self.try_read_result(task_id, session_id)? {
                        return Ok(payload);
                    }
                }
                let poll_after = crate::db::next_poll_after_secs(0);
                return Ok(json!({
                    "status": "backgrounded",
                    "task_id": task_id,
                    "poll_after_secs": poll_after,
                    "next_action": crate::db::NEXT_ACTION_WAIT_TASK,
                    "message": format!(
                        "İşlem {}sn eşiğini aştı, arka planda devam ediyor. ~{}sn sonra lounge_wait_task ile kontrol edin; sonuç gelmeden kullanıcıya bitti demeyin.",
                        timeout_limit.as_secs(),
                        poll_after
                    ),
                    "timeout_limit_secs": timeout_limit.as_secs(),
                    "long_running": long_running,
                    "hint": "Call lounge_wait_task(task_id) on a schedule (poll_after_secs). Keep task_token if reconnecting in a new session."
                }));
            }

            while opts.progress_token.is_some()
                && profile.progress_extends
                && now >= next_heartbeat_ms
            {
                // Cancel yeniden kontrol — heartbeat aralığında Stop gelmiş olabilir.
                let pending_cancel = cancel_rx.as_ref().and_then(|rx| *rx.borrow());
                if let Some(kind) = pending_cancel {
                    return self.handle_cancel(task_id, session_id, kind).await;
                }
                if let Some(ref token) = opts.progress_token {
                    let elapsed = (now - start_ms) / 1000;
                    opts.progress_sink.emit_progress(
                        token,
                        &format!("lounge waiting ({elapsed}s)"),
                        elapsed,
                    );
                }
                // Saat sıçramasında (test) kaçırılan dilimleri yakala.
                next_heartbeat_ms = next_heartbeat_ms.saturating_add(heartbeat.as_millis() as u64);
            }

            let until_deadline = deadline_ms.saturating_sub(now).max(1);
            let until_hb = next_heartbeat_ms.saturating_sub(now).max(1);
            let slice = poll.min(Duration::from_millis(until_deadline.min(until_hb)));
            let nats_fut = poll_terminal_match(&mut nats_rx, task_id, nats_alive);
            if let Some(rx) = cancel_rx.as_mut() {
                tokio::select! {
                    _ = self.clock.sleep(slice) => {}
                    closed = nats_fut => {
                        if closed {
                            nats_alive = false;
                        }
                    }
                    changed = rx.changed() => {
                        match changed {
                            Ok(()) => {
                                let kind = *rx.borrow();
                                if let Some(kind) = kind {
                                    return self.handle_cancel(task_id, session_id, kind).await;
                                }
                            }
                            Err(_) => {
                                cancel_rx = None;
                            }
                        }
                    }
                }
            } else {
                tokio::select! {
                    _ = self.clock.sleep(slice) => {}
                    closed = nats_fut => {
                        if closed {
                            nats_alive = false;
                        }
                    }
                }
            }
        }
    }

    fn subscribe_terminal(&self, _task_id: &str) -> broadcast::Receiver<String> {
        self.terminal_hub.subscribe()
    }

    pub(crate) fn try_read_result(&self, task_id: &str, session_id: &str) -> Result<Option<Value>> {
        self.try_read_result_with_token(task_id, session_id, None)
    }

    pub(crate) fn try_read_result_with_token(
        &self,
        task_id: &str,
        session_id: &str,
        task_token: Option<&str>,
    ) -> Result<Option<Value>> {
        if !self
            .store
            .session_or_token_can_read_a2a_task(task_id, session_id, task_token)?
        {
            return Err(anyhow!("yetkisiz oturum: görev okunamaz"));
        }
        let status = match self.store.a2a_task_status(task_id)? {
            Some(s) => s,
            None => return Ok(None),
        };
        // NeedsHuman: soft — wait bitmez; istemciye needs_human bayrağı + ipucu.
        // Expired/Timeout/Cancelled: sert terminal.
        // Completed: result yoksa da bitir (P1-D). complete_a2a_with_result atomik;
        // sonuçsuz Completed yolları (yerel yürütme / boş bus zarfı) artık hang olmaz.
        match status {
            TaskStatus::Completed => {
                let _ = self.store.touch_a2a_last_wait(task_id);
                let result = self.store.a2a_task_result(task_id)?;
                let missing = result.as_ref().map(|s| s.is_empty()).unwrap_or(true);
                Ok(Some(json!({
                    "status": "completed",
                    "task_id": task_id,
                    "result": result.as_ref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
                    "result_text": result,
                    "result_missing": missing,
                })))
            }
            TaskStatus::Failed
            | TaskStatus::Cancelled
            | TaskStatus::Expired
            | TaskStatus::Timeout => {
                let _ = self.store.touch_a2a_last_wait(task_id);
                let result = self.store.a2a_task_result(task_id)?;
                Ok(Some(json!({
                    "status": status.as_str().to_ascii_lowercase(),
                    "task_id": task_id,
                    "result": result.as_ref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
                    "result_text": result,
                })))
            }
            TaskStatus::NeedsHuman => {
                // Soft: wait döngüsünü bitir ama completed sayma — istemci onay beklesin.
                Ok(Some(json!({
                    "status": "still_running",
                    "needs_human": true,
                    "task_id": task_id,
                    "task_status": "NEEDS_HUMAN",
                    "hint": "Kullanıcı onayı gerekli — tekrar bekleme; UI/karar sonrası yeniden dene.",
                    "message": "Görev insan onayı bekliyor (NEEDS_HUMAN).",
                })))
            }
            _ => Ok(None),
        }
    }

    async fn handle_cancel(
        &self,
        task_id: &str,
        session_id: &str,
        kind: CancelKind,
    ) -> Result<Value> {
        match kind {
            CancelKind::UserStop => self.cancel_task_user_stop(task_id, session_id).await,
            CancelKind::DeadlineExceeded => {
                self.background_on_cancel(task_id, session_id, "deadline exceeded")
                    .await
            }
            CancelKind::SessionDisconnect => {
                let (must_deliver, _) = self.store.a2a_flags(task_id).unwrap_or((false, false));
                if must_deliver {
                    // must_deliver: in-flight bile arka plana; iptal yok.
                    self.background_on_cancel(task_id, session_id, "session disconnect")
                        .await
                } else {
                    self.cancel_task_user_stop(task_id, session_id).await
                }
            }
            CancelKind::Other => Ok(json!({
                "status": "cancelled",
                "task_id": task_id,
                "reason": "other",
            })),
        }
    }

    async fn cancel_task_user_stop(&self, task_id: &str, session_id: &str) -> Result<Value> {
        self.store.cancel_a2a_task(task_id, session_id)?;
        let mut control_stop = false;
        let mut control_stop_error: Option<String> = None;
        if !self.skip_nats || self.control_publisher_forced {
            let publisher = Arc::clone(&self.control_publisher);
            let tid = task_id.to_string();
            let sid = session_id.to_string();
            match tokio::task::spawn_blocking(move || publisher.publish_control_stop(&tid, &sid))
                .await
                .context("control.stop join")?
            {
                Ok(()) => control_stop = true,
                Err(err) => {
                    control_stop_error = Some(err.to_string());
                    log::warn!("control.stop yayınlanamadı: {err}");
                }
            }
        }
        let mut body = json!({
            "status": "cancelled",
            "task_id": task_id,
            "reason": "context canceled",
            "control_stop": control_stop,
            "subject": CONTROL_STOP,
        });
        if let Some(err) = control_stop_error {
            body["control_stop_error"] = json!(err);
        }
        Ok(body)
    }

    async fn background_on_cancel(
        &self,
        task_id: &str,
        session_id: &str,
        reason: &str,
    ) -> Result<Value> {
        let won = self.store.try_mark_a2a_wait_timeout(task_id)?;
        if !won {
            if let Some(payload) = self.try_read_result(task_id, session_id)? {
                return Ok(payload);
            }
        }
        let poll_after = crate::db::next_poll_after_secs(0);
        Ok(json!({
            "status": "backgrounded",
            "task_id": task_id,
            "reason": reason,
            "poll_after_secs": poll_after,
            "next_action": crate::db::NEXT_ACTION_WAIT_TASK,
            "message": "Görev arka planda sürüyor. lounge_wait_task ile sonucu alın.",
            "hint": "Call lounge_wait_task in this session (or with task_token)."
        }))
    }

    /// Bounded wait — eşik `timeout_limit` (istemci tablosu); istenen `timeout_ms` ile min.
    pub async fn wait_task(
        &self,
        session_id: &str,
        client_name: &str,
        task_id: &str,
        timeout_ms: Option<u64>,
        cancel_rx: Option<watch::Receiver<Option<CancelKind>>>,
    ) -> Result<Value> {
        self.wait_task_with_opts(
            session_id,
            client_name,
            task_id,
            timeout_ms,
            cancel_rx,
            WaitOpts::default(),
            None,
        )
        .await
    }

    pub async fn wait_task_with_token(
        &self,
        session_id: &str,
        client_name: &str,
        task_id: &str,
        timeout_ms: Option<u64>,
        cancel_rx: Option<watch::Receiver<Option<CancelKind>>>,
        task_token: Option<&str>,
    ) -> Result<Value> {
        self.wait_task_with_opts(
            session_id,
            client_name,
            task_id,
            timeout_ms,
            cancel_rx,
            WaitOpts::default(),
            task_token,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn wait_task_with_opts(
        &self,
        session_id: &str,
        client_name: &str,
        task_id: &str,
        timeout_ms: Option<u64>,
        mut cancel_rx: Option<watch::Receiver<Option<CancelKind>>>,
        opts: WaitOpts,
        task_token: Option<&str>,
    ) -> Result<Value> {
        let has_progress = opts.progress_token.is_some();
        let timeout_limit = self
            .timeouts
            .timeout_limit_with_progress(client_name, has_progress);
        let diag = profile_diag(client_name, timeout_limit);
        let requested = timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(timeout_limit);
        let budget = requested.min(timeout_limit);

        if !self
            .store
            .session_or_token_can_read_a2a_task(task_id, session_id, task_token)?
        {
            return Err(anyhow!("yetkisiz oturum: görev okunamaz"));
        }
        let _ = self.store.touch_a2a_last_wait(task_id);

        let deadline_ms = self
            .clock
            .now_ms()
            .saturating_add(budget.as_millis() as u64);
        let poll = Duration::from_millis(50);
        let mut nats_rx = self.subscribe_terminal(task_id);
        let mut nats_alive = !self.skip_nats;
        let mut cancel_alive = cancel_rx.is_some();

        loop {
            let cancel_kind = if cancel_alive {
                cancel_rx.as_ref().and_then(|rx| *rx.borrow())
            } else {
                None
            };
            if let Some(kind) = cancel_kind {
                match kind {
                    CancelKind::UserStop => {
                        let mut out = self.handle_cancel(task_id, session_id, kind).await?;
                        merge_diag(&mut out, &diag);
                        return Ok(out);
                    }
                    // Oturum koptu: yalnız long-poll bitsin — görevi iptal etme (backgrounded korunur).
                    CancelKind::SessionDisconnect => {
                        let status = self
                            .store
                            .a2a_task_status(task_id)?
                            .unwrap_or(TaskStatus::WaitTimeoutReached);
                        let poll_after = crate::db::next_poll_after_secs(0);
                        let mut body = json!({
                            "status": "still_running",
                            "task_id": task_id,
                            "task_status": status.as_str(),
                            "reason": "session disconnect",
                            "poll_after_secs": poll_after,
                            "retry_after_ms": poll_after.saturating_mul(1000),
                            "next_action": crate::db::NEXT_ACTION_WAIT_TASK,
                            "timeout_limit_secs": timeout_limit.as_secs(),
                            "message": "Oturum koptu — görev arka planda sürüyor. lounge_wait_task (veya task_token) ile yeniden bağlanın; iptal edilmedi.",
                            "hint": "Task was not cancelled. Reconnect and call lounge_wait_task."
                        });
                        merge_diag(&mut body, &diag);
                        return Ok(body);
                    }
                    CancelKind::DeadlineExceeded => {
                        // İstemci wait deadline — görev arka planda kalsın (iptal yok).
                        let mut out = self
                            .background_on_cancel(task_id, session_id, "deadline exceeded")
                            .await?;
                        merge_diag(&mut out, &diag);
                        return Ok(out);
                    }
                    CancelKind::Other => {}
                }
            }

            if let Some(mut payload) =
                self.try_read_result_with_token(task_id, session_id, task_token)?
            {
                merge_diag(&mut payload, &diag);
                return Ok(payload);
            }

            if self.clock.now_ms() >= deadline_ms {
                let status = self
                    .store
                    .a2a_task_status(task_id)?
                    .unwrap_or(TaskStatus::WaitTimeoutReached);
                let (must_deliver, long_running) =
                    self.store.a2a_flags(task_id).unwrap_or((false, false));
                let use_backoff = long_running
                    || matches!(status, TaskStatus::WaitTimeoutReached)
                    || must_deliver;
                let poll_after = if use_backoff {
                    self.store.bump_a2a_empty_wait(task_id).unwrap_or(15)
                } else {
                    1
                };
                let mut body = json!({
                    "status": "still_running",
                    "task_id": task_id,
                    "task_status": status.as_str(),
                    "retry_after_ms": poll_after.saturating_mul(1000),
                    "poll_after_secs": poll_after,
                    "next_action": crate::db::NEXT_ACTION_WAIT_TASK,
                    "timeout_limit_secs": timeout_limit.as_secs(),
                    "long_running": long_running,
                    "must_deliver": must_deliver,
                    "message": "Görev hâlâ çalışıyor — lounge_wait_task tekrar çağrılabilir."
                });
                if matches!(status, TaskStatus::NeedsHuman) {
                    body["needs_human"] = json!(true);
                    body["hint"] = json!(
                        "Kullanıcı onayı gerekli — tekrar bekleme; UI/karar sonrası yeniden dene."
                    );
                }
                merge_diag(&mut body, &diag);
                return Ok(body);
            }

            let slice = poll.min(Duration::from_millis(
                deadline_ms.saturating_sub(self.clock.now_ms()).max(1),
            ));
            let nats_fut = poll_terminal_match(&mut nats_rx, task_id, nats_alive);
            if cancel_alive {
                if let Some(rx) = cancel_rx.as_mut() {
                    tokio::select! {
                        _ = self.clock.sleep(slice) => {}
                        closed = nats_fut => { if closed { nats_alive = false; } }
                        changed = rx.changed() => {
                            match changed {
                                Ok(()) => {}
                                Err(_) => {
                                    cancel_alive = false;
                                    cancel_rx = None;
                                }
                            }
                        }
                    }
                    continue;
                }
            }
            tokio::select! {
                _ = self.clock.sleep(slice) => {}
                closed = nats_fut => { if closed { nats_alive = false; } }
            }
        }
    }

    /// Oturum kopması: in-flight cancel kanallarına SessionDisconnect.
    /// must_deliver → arka plan; diğerleri → iptal. Backgrounded map'te yok → korunur.
    pub async fn cancel_in_flight_on_disconnect(
        &self,
        session_id: &str,
        cancel_txs: Vec<watch::Sender<Option<CancelKind>>>,
    ) -> usize {
        let n = cancel_txs.len();
        for tx in cancel_txs {
            let _ = tx.send(Some(CancelKind::SessionDisconnect));
        }
        if n > 0 {
            log::info!("session disconnect: {n} in-flight SessionDisconnect session={session_id}");
        }
        let _ = self.store.mark_mcp_session_disconnected(session_id);
        n
    }

    pub async fn yield_result(
        &self,
        session_id: &str,
        task_id: &str,
        status_raw: &str,
        output: &Value,
        artifacts: Option<&Value>,
        metrics: Option<&Value>,
    ) -> Result<Value> {
        let status = match status_raw.trim().to_ascii_lowercase().as_str() {
            "completed" | "ok" | "success" => TaskStatus::Completed,
            "failed" | "fail" | "error" => TaskStatus::Failed,
            other => return Err(anyhow!("status completed|failed olmalı, gelen={other}")),
        };
        let status_label = status.as_str().to_ascii_lowercase();
        let envelope = json!({
            "output": output,
            "artifacts": artifacts.cloned().unwrap_or(json!([])),
            "metrics": metrics.cloned().unwrap_or(json!({})),
            "yielded_at": now_rfc3339(),
            "yielded_by_session": session_id,
        });
        let raw = serde_json::to_string(&envelope)?;
        const MAX_YIELD_JSON: usize = 262_144; // 256 KiB
        if raw.len() > MAX_YIELD_JSON {
            return Err(anyhow!(
                "yield output çok büyük ({} bayt > {MAX_YIELD_JSON})",
                raw.len()
            ));
        }
        self.store
            .yield_a2a_result(task_id, session_id, status.clone(), &raw)?;

        // Workflow engine / UI: yield Completed|Failed → bus terminal yayını.
        let mut published = false;
        let mut publish_error: Option<String> = None;
        if !self.skip_nats {
            match self.publish_task_terminal(task_id, status, &envelope).await {
                Ok(()) => published = true,
                Err(err) => {
                    publish_error = Some(err.to_string());
                    log::warn!("yield terminal publish: {err}");
                }
            }
        }

        let mut body = json!({
            "ok": true,
            "task_id": task_id,
            "status": status_label,
            "session_id": session_id,
            "published": published,
        });
        if let Some(err) = publish_error {
            body["publish_error"] = json!(err);
        }
        Ok(body)
    }

    async fn publish_task_requested(&self, task: &LoungeTask) -> Result<()> {
        let url = self.nats_url.clone();
        let subject = TASK_REQUESTED.to_string();
        let bytes = serde_json::to_vec(task)?;
        tokio::task::spawn_blocking(move || {
            #[allow(deprecated)]
            let nc = crate::services::nats_connect(&url).map_err(|e| anyhow!("NATS connect: {e}"))?;
            nc.publish(&subject, bytes)
                .map_err(|e| anyhow!("NATS publish: {e}"))?;
            nc.flush().map_err(|e| anyhow!("NATS flush: {e}"))?;
            Ok::<(), anyhow::Error>(())
        })
        .await
        .context("NATS publish join")??;
        Ok(())
    }

    async fn publish_task_terminal(
        &self,
        task_id: &str,
        status: TaskStatus,
        envelope: &Value,
    ) -> Result<()> {
        let subject = if matches!(status, TaskStatus::Failed) {
            TASK_FAILED
        } else {
            TASK_COMPLETED
        };
        let url = self.nats_url.clone();
        let subject = subject.to_string();
        let payload = json!({
            "id": task_id,
            "task_id": task_id,
            "status": status.as_str(),
            "result": envelope,
            "at": now_rfc3339(),
        });
        let bytes = serde_json::to_vec(&payload)?;
        tokio::task::spawn_blocking(move || {
            #[allow(deprecated)]
            let nc = crate::services::nats_connect(&url).map_err(|e| anyhow!("NATS connect: {e}"))?;
            nc.publish(&subject, bytes)
                .map_err(|e| anyhow!("NATS publish: {e}"))?;
            nc.flush().map_err(|e| anyhow!("NATS flush: {e}"))?;
            Ok::<(), anyhow::Error>(())
        })
        .await
        .context("terminal publish join")??;
        Ok(())
    }
}

/// `true` ⇒ hub kapandı (kolu bırak). `alive=false` ⇒ sonsuza pending.
async fn poll_terminal_match(
    rx: &mut broadcast::Receiver<String>,
    want: &str,
    alive: bool,
) -> bool {
    if !alive {
        std::future::pending::<()>().await;
        return true;
    }
    loop {
        match rx.recv().await {
            Ok(id) if id == want => return false,
            Ok(_) => continue,
            Err(broadcast::error::RecvError::Closed) => return true,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
        }
    }
}

fn merge_diag(body: &mut Value, diag: &Value) {
    if let (Some(obj), Some(d)) = (body.as_object_mut(), diag.as_object()) {
        for (k, v) in d {
            obj.insert(k.clone(), v.clone());
        }
    }
}

#[cfg(test)]
fn terminal_payload_matches_task(data: &[u8], task_id: &str) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(data) else {
        return false;
    };
    value
        .get("id")
        .or_else(|| value.get("task_id"))
        .and_then(|v| v.as_str())
        == Some(task_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::wait_clock::ManualWaitClock;
    use crate::models::AgentSession;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct RecordingPublisher {
        calls: AtomicUsize,
        fail: bool,
    }

    impl ControlStopPublisher for RecordingPublisher {
        fn publish_control_stop(&self, _task_id: &str, _session_id: &str) -> Result<()> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                Err(anyhow!("simulated publish fail"))
            } else {
                Ok(())
            }
        }
    }

    fn bind_worker(store: &ExperienceStore, session_id: &str, agent: &str) {
        let mut sess = AgentSession::new("p", agent, "worker", "/tmp", "test");
        sess.id = session_id.into();
        store.upsert_session(&sess).unwrap();
    }

    fn orch_manual() -> (Orchestrator, Arc<ManualWaitClock>, String) {
        let store = ExperienceStore::memory().unwrap();
        let clock = Arc::new(ManualWaitClock::new());
        let timeouts = Arc::new(TimeoutManager::with_defaults());
        timeouts.set_global_override_secs(Some(2));
        let orch = Orchestrator::new(
            store,
            "nats://127.0.0.1:9",
            timeouts,
            clock.clone() as Arc<dyn WaitClock>,
        )
        .with_skip_nats(true);
        let session = Uuid::new_v4().to_string();
        (orch, clock, session)
    }

    #[tokio::test]
    async fn sync_return_under_threshold() {
        let (orch, clock, session) = orch_manual();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "echo".into(),
            project_id: "p".into(),
            idempotency_key: Some("idem-1".into()),
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: false,
            must_deliver: false,
        };
        let store = orch.store().clone();
        bind_worker(&store, "worker-sess", "worker");
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let call = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            async move {
                orch.call_agent(&session, "cursor", args, Some(cancel_rx))
                    .await
            }
        });
        tokio::task::yield_now().await;
        clock.advance(Duration::from_millis(20));
        let mut found = None;
        for _ in 0..40 {
            let id: Option<String> = {
                let conn = store.conn.lock().unwrap();
                conn.query_row("SELECT id FROM a2a_tasks LIMIT 1", [], |r| r.get(0))
                    .ok()
            };
            if let Some(id) = id {
                store
                    .yield_a2a_result(&id, "worker-sess", TaskStatus::Completed, r#"{"v":1}"#)
                    .unwrap();
                found = Some(id);
                break;
            }
            clock.advance(Duration::from_millis(5));
            tokio::task::yield_now().await;
        }
        assert!(found.is_some(), "task admit edilmeli");
        clock.advance(Duration::from_millis(50));
        let result = call.await.unwrap().unwrap();
        assert_eq!(result["status"], "completed");
        let _ = cancel_tx;
    }

    #[tokio::test]
    async fn backgrounded_then_wait_task() {
        let (orch, clock, session) = orch_manual();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "slow".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: false,
            must_deliver: false,
        };
        bind_worker(orch.store(), "w", "worker");
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let call = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            async move {
                orch.call_agent(&session, "cursor", args, Some(cancel_rx))
                    .await
            }
        });
        tokio::task::yield_now().await;
        clock.advance(Duration::from_secs(3));
        let bg = call.await.unwrap().unwrap();
        assert_eq!(bg["status"], "backgrounded");
        let task_id = bg["task_id"].as_str().unwrap().to_string();

        orch.store()
            .yield_a2a_result(&task_id, "w", TaskStatus::Completed, r#"{"done":true}"#)
            .unwrap();

        let waited = orch
            .wait_task(&session, "cursor", &task_id, Some(500), None)
            .await
            .unwrap();
        assert_eq!(waited["status"], "completed");
        let _ = cancel_tx;
    }

    #[tokio::test]
    async fn context_canceled_reports_control_stop_honestly() {
        let store = ExperienceStore::memory().unwrap();
        let clock = Arc::new(ManualWaitClock::new());
        let timeouts = Arc::new(TimeoutManager::with_defaults());
        timeouts.set_global_override_secs(Some(2));
        let publisher = Arc::new(RecordingPublisher {
            calls: AtomicUsize::new(0),
            fail: false,
        });
        let orch = Orchestrator::new(
            store,
            "nats://127.0.0.1:9",
            timeouts,
            clock.clone() as Arc<dyn WaitClock>,
        )
        .with_skip_nats(true)
        .with_control_publisher(publisher.clone() as Arc<dyn ControlStopPublisher>);
        let session = Uuid::new_v4().to_string();

        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "x".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: false,
            must_deliver: false,
        };
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let call = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            async move {
                orch.call_agent(&session, "cursor", args, Some(cancel_rx))
                    .await
            }
        });
        tokio::task::yield_now().await;
        clock.advance(Duration::from_millis(10));
        let _ = cancel_tx.send(Some(CancelKind::UserStop));
        clock.advance(Duration::from_millis(50));
        let out = call.await.unwrap().unwrap();
        assert_eq!(out["status"], "cancelled");
        assert_eq!(out["control_stop"], true);
        assert_eq!(publisher.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn context_canceled_skip_nats_reports_control_stop_false() {
        let (orch, clock, session) = orch_manual();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "x".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: false,
            must_deliver: false,
        };
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let call = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            async move {
                orch.call_agent(&session, "cursor", args, Some(cancel_rx))
                    .await
            }
        });
        tokio::task::yield_now().await;
        clock.advance(Duration::from_millis(10));
        let _ = cancel_tx.send(Some(CancelKind::UserStop));
        clock.advance(Duration::from_millis(50));
        let out = call.await.unwrap().unwrap();
        assert_eq!(out["status"], "cancelled");
        assert_eq!(
            out["control_stop"], false,
            "skip_nats iken control_stop:false olmalı"
        );
    }

    #[tokio::test]
    async fn deadline_exceeded_keeps_task_running() {
        let (orch, clock, session) = orch_manual();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "x".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: false,
            must_deliver: false,
        };
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let call = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            async move {
                orch.call_agent(&session, "cursor", args, Some(cancel_rx))
                    .await
            }
        });
        tokio::task::yield_now().await;
        let _ = cancel_tx.send(Some(CancelKind::DeadlineExceeded));
        clock.advance(Duration::from_millis(50));
        let out = call.await.unwrap().unwrap();
        assert_eq!(out["status"], "backgrounded");
        let task_id = out["task_id"].as_str().unwrap();
        let st = orch.store().a2a_task_status(task_id).unwrap().unwrap();
        assert_eq!(st, TaskStatus::WaitTimeoutReached);
    }

    /// Antigravity: notifications/cancelled (context deadline exceeded) → backgrounded kalır.
    #[tokio::test]
    async fn antigravity_deadline_cancel_keeps_task_backgrounded() {
        let (orch, clock, session) = orch_manual();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "antigravity-long".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: false,
            must_deliver: false,
        };
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let call = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            async move {
                orch.call_agent(&session, "antigravity-client", args, Some(cancel_rx))
                    .await
            }
        });
        tokio::task::yield_now().await;
        let kind = CancelKind::from_reason("context deadline exceeded");
        assert_eq!(kind, CancelKind::DeadlineExceeded);
        let _ = cancel_tx.send(Some(kind));
        clock.advance(Duration::from_millis(50));
        let out = call.await.unwrap().unwrap();
        assert_eq!(out["status"], "backgrounded");
        assert!(out["poll_after_secs"].as_u64().unwrap() >= 15);
        let task_id = out["task_id"].as_str().unwrap();
        let st = orch.store().a2a_task_status(task_id).unwrap().unwrap();
        assert_eq!(st, TaskStatus::WaitTimeoutReached);
        // İptal edilmemiş — CANCELLED olmamalı.
        assert_ne!(st, TaskStatus::Cancelled);
    }

    #[tokio::test]
    async fn unauthorized_session_cannot_wait() {
        let (orch, _clock, session) = orch_manual();
        let mut task = LoungeTask::new("mcp:cursor", "p", "t");
        task.session_id = Some(session.clone());
        orch.store().admit_a2a_task(&mut task, 10).unwrap();
        let err = orch
            .wait_task("other-session", "cursor", &task.id, Some(10), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("yetkisiz"));
    }

    #[tokio::test]
    async fn needs_human_surfaces_flag_without_treating_as_completed() {
        // NeedsHuman soft: still_running + needs_human:true (completed değil).
        // Geç gelen worker Completed yazabilir; istemci onay sonrası yeniden dener.
        let (orch, _clock, session) = orch_manual();
        let mut task = LoungeTask::new("mcp:cursor", "p", "t");
        task.session_id = Some(session.clone());
        orch.store().admit_a2a_task(&mut task, 10).unwrap();
        orch.store()
            .set_a2a_task_status(&task.id, TaskStatus::NeedsHuman)
            .unwrap();
        let out = orch
            .wait_task(&session, "cursor", &task.id, Some(500), None)
            .await
            .unwrap();
        assert_eq!(out["status"], "still_running");
        assert_eq!(out["task_status"], "NEEDS_HUMAN");
        assert_eq!(out["needs_human"], true);
        assert!(
            out["hint"].as_str().unwrap_or("").contains("onay"),
            "hint must discourage blind re-wait: {out}"
        );
    }

    #[tokio::test]
    async fn completed_without_result_finishes_with_result_missing() {
        // P1-D: Completed + boş result_json → completed (hang yok); result_missing:true.
        let (orch, _clock, session) = orch_manual();
        let mut task = LoungeTask::new("mcp:cursor", "p", "race");
        task.session_id = Some(session.clone());
        orch.store().admit_a2a_task(&mut task, 10).unwrap();
        orch.store()
            .set_a2a_task_status(&task.id, TaskStatus::Completed)
            .unwrap();
        let got = orch.try_read_result(&task.id, &session).unwrap().unwrap();
        assert_eq!(got["status"], "completed");
        assert_eq!(got["result_missing"], true);
        assert!(got["result"].is_null());

        orch.store()
            .complete_a2a_with_result(
                &task.id,
                TaskStatus::Completed,
                Some(r#"{"ok":true}"#),
                false,
            )
            .unwrap();
        let got = orch.try_read_result(&task.id, &session).unwrap().unwrap();
        assert_eq!(got["status"], "completed");
        assert_eq!(got["result_missing"], false);
        assert!(got["result"].is_object() || got["result_text"].is_string());
    }

    #[tokio::test]
    async fn mcp_tasks_are_unverified() {
        let (orch, _clock, session) = orch_manual();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "x".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: false,
            long_running: false,
            must_deliver: false,
        };
        let out = orch
            .call_agent(&session, "cursor", args, None)
            .await
            .unwrap();
        assert_eq!(out["source_verified"], false);
        let task = orch
            .store()
            .load_a2a_task(out["task_id"].as_str().unwrap())
            .unwrap()
            .unwrap();
        assert!(!task.source_verified);
    }

    #[test]
    fn cancel_kind_parses_reasons() {
        assert_eq!(
            CancelKind::from_reason("context canceled"),
            CancelKind::UserStop
        );
        assert_eq!(
            CancelKind::from_reason("context deadline exceeded"),
            CancelKind::DeadlineExceeded
        );
    }

    #[test]
    fn nats_terminal_payload_matches_task_id() {
        assert!(terminal_payload_matches_task(
            br#"{"id":"abc","type":"task"}"#,
            "abc"
        ));
        assert!(terminal_payload_matches_task(
            br#"{"task_id":"xyz"}"#,
            "xyz"
        ));
        assert!(!terminal_payload_matches_task(br#"{"id":"abc"}"#, "other"));
    }

    #[tokio::test]
    async fn cursor_progress_heartbeat_extends_wait() {
        let store = ExperienceStore::memory().unwrap();
        let clock = Arc::new(ManualWaitClock::new());
        let timeouts = Arc::new(TimeoutManager::with_defaults());
        // Override yok — Cursor progress ile 280 sn.
        let sink = Arc::new(CountingProgressSink::default());
        let orch = Orchestrator::new(
            store,
            "nats://127.0.0.1:9",
            timeouts,
            clock.clone() as Arc<dyn WaitClock>,
        )
        .with_skip_nats(true);
        let session = Uuid::new_v4().to_string();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "long".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: false,
            must_deliver: false,
        };
        let opts = WaitOpts {
            progress_token: Some(json!(42)),
            progress_sink: sink.clone() as Arc<dyn ProgressSink>,
        };
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let call = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            async move {
                orch.call_agent_with_opts(&session, "cursor-vscode", args, Some(cancel_rx), opts)
                    .await
            }
        });
        tokio::task::yield_now().await;
        // 100 sn eşiğini aş, 280'e kadar devam etmeli (henüz backgrounded değil).
        clock.advance(Duration::from_secs(110));
        tokio::task::yield_now().await;
        assert!(!call.is_finished(), "progress ile 110sn'de hâlâ beklemeli");
        assert!(
            sink.count.load(Ordering::SeqCst) >= 10,
            "≈10 sn heartbeat: {}",
            sink.count.load(Ordering::SeqCst)
        );
        // 280 sn dolsun → backgrounded
        clock.advance(Duration::from_secs(180));
        let out = call.await.unwrap().unwrap();
        assert_eq!(out["status"], "backgrounded");
        assert_eq!(out["client_profile"], "cursor-vscode");
        assert_eq!(out["timeout_limit_secs"], 280);
        let _ = cancel_tx;
    }

    #[tokio::test]
    async fn disconnect_cancels_inflight_keeps_backgrounded() {
        let (orch, clock, session) = orch_manual();
        // 1) Backgrounded görev
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "bg".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: false,
            must_deliver: false,
        };
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let call = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            async move {
                orch.call_agent(&session, "cursor", args, Some(cancel_rx))
                    .await
            }
        });
        tokio::task::yield_now().await;
        clock.advance(Duration::from_secs(3));
        let bg = call.await.unwrap().unwrap();
        assert_eq!(bg["status"], "backgrounded");
        let bg_id = bg["task_id"].as_str().unwrap().to_string();
        let _ = cancel_tx;

        // 2) Hâlâ in-flight senkron bekleme
        let args2 = CallAgentArgs {
            target_agent: "worker".into(),
            task: "inflight".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: false,
            must_deliver: false,
        };
        let (tx2, rx2) = watch::channel(None);
        let call2 = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            async move { orch.call_agent(&session, "cursor", args2, Some(rx2)).await }
        });
        tokio::task::yield_now().await;
        clock.advance(Duration::from_millis(20));

        // Disconnect: yalnız in-flight cancel
        let n = orch
            .cancel_in_flight_on_disconnect(&session, vec![tx2])
            .await;
        assert_eq!(n, 1);
        clock.advance(Duration::from_millis(50));
        let cancelled = call2.await.unwrap().unwrap();
        assert_eq!(cancelled["status"], "cancelled");

        // Backgrounded hâlâ WaitTimeoutReached (CANCELLED değil)
        let st = orch.store().a2a_task_status(&bg_id).unwrap().unwrap();
        assert_eq!(st, TaskStatus::WaitTimeoutReached);
    }

    #[tokio::test]
    async fn responses_include_client_profile() {
        let (orch, _clock, session) = orch_manual();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "x".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: false,
            long_running: false,
            must_deliver: false,
        };
        let out = orch
            .call_agent(&session, "claude-ai", args, None)
            .await
            .unwrap();
        assert_eq!(out["client_profile"], "claude-ai");
        assert_eq!(out["timeout_limit_secs"], 2);
        assert!(out.get("task_token").and_then(|v| v.as_str()).is_some());
    }

    #[tokio::test]
    async fn threshold_race_single_winner_completed_not_backgrounded() {
        // Eşik anında sonuç yazılırsa yalnız completed (çift yanıt yok).
        let (orch, clock, session) = orch_manual();
        bind_worker(orch.store(), "w", "worker");
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "race".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: false,
            must_deliver: false,
        };
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let call = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            async move {
                orch.call_agent(&session, "cursor", args, Some(cancel_rx))
                    .await
            }
        });
        tokio::task::yield_now().await;
        // Deadline'a yaklaş: override 2s.
        clock.advance(Duration::from_millis(1900));
        tokio::task::yield_now().await;
        let task_id = {
            let conn = orch.store().conn.lock().unwrap();
            conn.query_row("SELECT id FROM a2a_tasks LIMIT 1", [], |r| {
                r.get::<_, String>(0)
            })
            .unwrap()
        };
        orch.store()
            .yield_a2a_result(&task_id, "w", TaskStatus::Completed, r#"{"race":true}"#)
            .unwrap();
        clock.advance(Duration::from_millis(200));
        let out = call.await.unwrap().unwrap();
        assert_eq!(out["status"], "completed", "tek kazanan completed: {out}");
        assert_ne!(out["status"], "backgrounded");
        let _ = cancel_tx;
    }

    #[tokio::test]
    async fn must_deliver_idempotency_first_wins() {
        let (orch, _clock, session) = orch_manual();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "md".into(),
            project_id: "p".into(),
            idempotency_key: Some("md-key-1".into()),
            repo_path: None,
            parent_task_id: None,
            wait: false,
            long_running: false,
            must_deliver: true,
        };
        let first = orch
            .call_agent(&session, "cursor", args.clone(), None)
            .await
            .unwrap();
        let id1 = first["task_id"].as_str().unwrap().to_string();
        let second = orch
            .call_agent(&session, "cursor", args, None)
            .await
            .unwrap();
        assert_eq!(second["task_id"], id1);
        assert!(
            second.get("task_token").is_none() || second["replay_status"].is_string(),
            "ikinci çağrı yeni token üretmez / replay: {second}"
        );
        let conn = orch.store().conn.lock().unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM a2a_tasks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn must_deliver_quota_rejects_with_rpc_marker() {
        let (orch, _clock, session) = orch_manual();
        for i in 0..5 {
            let args = CallAgentArgs {
                target_agent: "worker".into(),
                task: format!("md-{i}"),
                project_id: "p".into(),
                idempotency_key: Some(format!("q-{i}")),
                repo_path: None,
                parent_task_id: None,
                wait: false,
                long_running: false,
                must_deliver: true,
            };
            orch.call_agent(&session, "cursor", args, None)
                .await
                .unwrap();
        }
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "overflow".into(),
            project_id: "p".into(),
            idempotency_key: Some("q-overflow".into()),
            repo_path: None,
            parent_task_id: None,
            wait: false,
            long_running: false,
            must_deliver: true,
        };
        let err = orch
            .call_agent(&session, "cursor", args, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("MCP_RPC:-32029"),
            "kota JSON-RPC marker: {err}"
        );
    }

    #[tokio::test]
    async fn cursor_heartbeat_does_not_delay_cancel() {
        let store = ExperienceStore::memory().unwrap();
        let clock = Arc::new(ManualWaitClock::new());
        let timeouts = Arc::new(TimeoutManager::with_defaults());
        let sink = Arc::new(CountingProgressSink::default());
        let orch = Orchestrator::new(
            store,
            "nats://127.0.0.1:9",
            timeouts,
            clock.clone() as Arc<dyn WaitClock>,
        )
        .with_skip_nats(true);
        let session = Uuid::new_v4().to_string();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "hb-cancel".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: false,
            must_deliver: false,
        };
        let opts = WaitOpts {
            progress_token: Some(json!(7)),
            progress_sink: sink.clone() as Arc<dyn ProgressSink>,
        };
        let (cancel_tx, cancel_rx) = watch::channel(None);
        let call = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            async move {
                orch.call_agent_with_opts(&session, "cursor-vscode", args, Some(cancel_rx), opts)
                    .await
            }
        });
        tokio::task::yield_now().await;
        clock.advance(Duration::from_secs(5));
        tokio::task::yield_now().await;
        let _ = cancel_tx.send(Some(CancelKind::UserStop));
        // Heartbeat 10sn — cancel hemen işlenmeli (50ms poll).
        clock.advance(Duration::from_millis(100));
        let out = tokio::time::timeout(Duration::from_secs(2), call)
            .await
            .expect("cancel heartbeat tarafından gecikmemeli")
            .unwrap()
            .unwrap();
        assert_eq!(out["status"], "cancelled");
    }

    #[tokio::test]
    async fn task_token_allows_reconnect_wait() {
        let (orch, _clock, session) = orch_manual();
        bind_worker(orch.store(), "w", "worker");
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "tok".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: true,
            must_deliver: false,
        };
        let bg = orch
            .call_agent(&session, "cursor", args, None)
            .await
            .unwrap();
        assert_eq!(bg["status"], "backgrounded");
        assert_eq!(bg["reason"], "long_running");
        assert!(bg["poll_after_secs"].as_u64().unwrap() >= 15);
        assert_eq!(
            bg["next_action"].as_str().unwrap(),
            crate::db::NEXT_ACTION_WAIT_TASK
        );
        let task_id = bg["task_id"].as_str().unwrap().to_string();
        let token = bg["task_token"].as_str().unwrap().to_string();
        orch.store()
            .yield_a2a_result(&task_id, "w", TaskStatus::Completed, r#"{"ok":1}"#)
            .unwrap();
        let other = Uuid::new_v4().to_string();
        let waited = orch
            .wait_task_with_token(&other, "cursor", &task_id, Some(500), None, Some(&token))
            .await
            .unwrap();
        assert_eq!(waited["status"], "completed");
        let denied = orch
            .wait_task_with_token(
                &other,
                "cursor",
                &task_id,
                Some(10),
                None,
                Some("bad-token-xxxxxxxxxxxx"),
            )
            .await;
        assert!(denied.is_err());
    }

    #[tokio::test]
    async fn long_running_immediate_backgrounded() {
        let (orch, clock, session) = orch_manual();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "lr".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: true,
            must_deliver: false,
        };
        let out = orch
            .call_agent(&session, "cursor", args, None)
            .await
            .unwrap();
        assert_eq!(out["status"], "backgrounded");
        assert_eq!(out["long_running"], true);
        assert_eq!(out["must_deliver"], false);
        assert!(out["poll_after_secs"].as_u64().is_some());
        assert_eq!(
            out["next_action"].as_str().unwrap(),
            crate::db::NEXT_ACTION_WAIT_TASK
        );
        // Saat ilerlemeden döndü — eşik beklenmedi (senkron dönüş).
        let _ = clock;
    }

    #[tokio::test]
    async fn must_deliver_disconnect_backgrounds_inflight() {
        let (orch, clock, session) = orch_manual();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "md-inflight".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: false,
            must_deliver: true,
        };
        let (tx, rx) = watch::channel(None);
        let call = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            async move { orch.call_agent(&session, "cursor", args, Some(rx)).await }
        });
        tokio::task::yield_now().await;
        clock.advance(Duration::from_millis(20));
        let n = orch
            .cancel_in_flight_on_disconnect(&session, vec![tx])
            .await;
        assert_eq!(n, 1);
        clock.advance(Duration::from_millis(50));
        let out = call.await.unwrap().unwrap();
        assert_eq!(out["status"], "backgrounded");
        assert_eq!(out["reason"], "session disconnect");
        let tid = out["task_id"].as_str().unwrap();
        let st = orch.store().a2a_task_status(tid).unwrap().unwrap();
        assert_eq!(st, TaskStatus::WaitTimeoutReached);
    }

    #[tokio::test]
    async fn must_deliver_echoed_on_dispatch() {
        let (orch, _clock, session) = orch_manual();
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "echo-flags".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: false,
            long_running: true,
            must_deliver: true,
        };
        let out = orch
            .call_agent(&session, "cursor", args, None)
            .await
            .unwrap();
        assert_eq!(out["long_running"], true);
        assert_eq!(out["must_deliver"], true);
        assert_eq!(
            out["next_action"].as_str().unwrap(),
            crate::db::NEXT_ACTION_WAIT_TASK
        );
        let tid = out["task_id"].as_str().unwrap();
        let (md, lr) = orch.store().a2a_flags(tid).unwrap();
        assert!(md && lr);
    }

    #[tokio::test]
    async fn wait_task_session_disconnect_does_not_cancel_backgrounded() {
        // B1: backgrounded göreve wait_task açılıp oturum kapanınca CANCELLED olmamalı;
        // task_token ile sonraki oturum sonuç alabilmeli.
        let (orch, clock, session) = orch_manual();
        bind_worker(orch.store(), "w", "worker");
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "bg-wait-disc".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: true,
            must_deliver: false,
        };
        let bg = orch
            .call_agent(&session, "cursor", args, None)
            .await
            .unwrap();
        assert_eq!(bg["status"], "backgrounded");
        let task_id = bg["task_id"].as_str().unwrap().to_string();
        let token = bg["task_token"].as_str().unwrap().to_string();

        let (tx, rx) = watch::channel(None);
        let wait = tokio::spawn({
            let orch = orch.clone();
            let session = session.clone();
            let task_id = task_id.clone();
            async move {
                orch.wait_task(&session, "cursor", &task_id, Some(5_000), Some(rx))
                    .await
            }
        });
        tokio::task::yield_now().await;
        clock.advance(Duration::from_millis(20));
        let _ = tx.send(Some(CancelKind::SessionDisconnect));
        clock.advance(Duration::from_millis(50));
        let out = wait.await.unwrap().unwrap();
        assert_eq!(out["status"], "still_running");
        assert_eq!(out["reason"], "session disconnect");
        let st = orch.store().a2a_task_status(&task_id).unwrap().unwrap();
        assert_eq!(
            st,
            TaskStatus::WaitTimeoutReached,
            "disconnect wait iptal etmemeli"
        );

        orch.store()
            .yield_a2a_result(&task_id, "w", TaskStatus::Completed, r#"{"ok":1}"#)
            .unwrap();
        let other = Uuid::new_v4().to_string();
        let claimed = orch
            .wait_task_with_token(&other, "cursor", &task_id, Some(500), None, Some(&token))
            .await
            .unwrap();
        assert_eq!(claimed["status"], "completed");
    }

    #[tokio::test]
    async fn try_read_result_marks_delivered_against_orphan_ttl() {
        // B3: terminal okuma last_wait yazar → sweeper COMPLETED silmez.
        let (orch, _clock, session) = orch_manual();
        bind_worker(orch.store(), "w", "worker");
        let args = CallAgentArgs {
            target_agent: "worker".into(),
            task: "deliv-read".into(),
            project_id: "p".into(),
            idempotency_key: None,
            repo_path: None,
            parent_task_id: None,
            wait: true,
            long_running: true,
            must_deliver: false,
        };
        let bg = orch
            .call_agent(&session, "cursor", args, None)
            .await
            .unwrap();
        let tid = bg["task_id"].as_str().unwrap().to_string();
        // Wait başı (result'tan önce).
        let _ = orch.store().touch_a2a_last_wait(&tid);
        tokio::time::sleep(Duration::from_millis(5)).await;
        orch.store()
            .yield_a2a_result(&tid, "w", TaskStatus::Completed, r#"{"v":1}"#)
            .unwrap();
        let got = orch.try_read_result(&tid, &session).unwrap().unwrap();
        assert_eq!(got["status"], "completed");
        let (expired, _) = orch
            .store()
            .expire_a2a_orphans(
                "2099-01-01T00:00:00.000Z",
                Duration::from_secs(1),
                Duration::from_secs(1800),
            )
            .unwrap();
        assert!(!expired.contains(&tid), "expired={expired:?}");
        assert_eq!(
            orch.store().a2a_task_status(&tid).unwrap(),
            Some(TaskStatus::Completed)
        );
    }
}
