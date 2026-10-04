//! MCP Bridge v2 — Mod A→B async orchestration.
//!
//! `lounge_call_agent` / `delegate_and_wait`: `select!` ile `timeout_limit` ve
//! sonuç dinleyicisini bekler. Eşik dolarsa `{status: backgrounded, task_id}` —
//! istemci sert zaman aşımından **önce** dönmek esas.
//!
//! Sonuç teslimi (deadline exceeded sonrası): bağlantı açık kalır; ajan
//! `lounge_wait_task` ile aynı oturumda sonucu çeker (piggyback). Progress
//! bildirimleri yalnız UI nabzı içindir; süreyi uzatmaz.
//!
//! İptal: `notifications/cancelled` reason `context canceled` → görev iptal +
//! `lounge.control.stop`. `deadline exceeded` → görev arka planda sürer.
//!
//! NATS: paylaşılan [`NatsTerminalHub`] (çağrı başına connect yok). Sender
//! kapanınca select kolu devre dışı kalır (busy-loop yok).

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use tokio::sync::{broadcast, watch};
use uuid::Uuid;

use super::nats_terminal::NatsTerminalHub;
use super::session_id::is_valid_session_id;
use super::timeout_manager::TimeoutManager;
use super::wait_clock::WaitClock;
use crate::db::{configured_max_hops, AdmitOutcome, ExperienceStore};
use crate::models::{
    now_rfc3339, LoungeTask, TaskStatus, CONTROL_STOP, TASK_COMPLETED, TASK_FAILED, TASK_REQUESTED,
};

/// İstemci iptal nedeni ayrımı.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelKind {
    /// Kullanıcı Stop — görev öldürülür.
    UserStop,
    /// İstemci deadline — görev arka planda devam.
    DeadlineExceeded,
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
        let nc = nats::connect(&url).map_err(|e| anyhow!("NATS connect: {e}"))?;
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
        let control_publisher: Arc<dyn ControlStopPublisher> =
            Arc::new(NatsControlStopPublisher {
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
        mut cancel_rx: Option<watch::Receiver<Option<CancelKind>>>,
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
        let timeout_limit = self.timeouts.timeout_limit(client_name);
        let source = format!(
            "mcp:{}",
            super::mcp_server::normalize_client_host(client_name)
        );

        let mut task = LoungeTask::new(source, args.project_id.clone(), args.task.clone());
        task.target_agent = Some(args.target_agent.clone());
        task.session_id = Some(session_id.to_string());
        // P1-1: token/parent-PID doğrulaması gelene kadar MCP görevleri unverified.
        task.source_verified = false;
        task.idempotency_key = args.idempotency_key.clone();
        task.repo_path = args.repo_path.clone();
        // parent_task_id istemci değeri yok sayılır — yalnız sunucu enjekte eder.
        let _ = args.parent_task_id;
        task.parent_task_id = None;

        let max_hops = configured_max_hops();
        let outcome = self
            .store
            .admit_a2a_task(&mut task, max_hops)
            .map_err(|e| anyhow!(e.to_string()))?;

        let (task_id, replay_status) = match outcome {
            AdmitOutcome::Accepted(t) => (t.id, None),
            AdmitOutcome::Replay {
                existing_task_id,
                status,
            } => {
                if status.is_terminal() {
                    let result = self.store.a2a_task_result(&existing_task_id)?;
                    return Ok(json!({
                        "status": status.as_str().to_ascii_lowercase(),
                        "task_id": existing_task_id,
                        "replay": true,
                        "result": result.and_then(|s| serde_json::from_str::<Value>(&s).ok()),
                        "timeout_limit_secs": timeout_limit.as_secs(),
                    }));
                }
                (existing_task_id, Some(status))
            }
        };

        if !self.skip_nats {
            self.publish_task_requested(&task).await?;
        }

        if !args.wait {
            return Ok(json!({
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
                "replay_status": replay_status.map(|s| s.as_str()),
                "note": "Görev NATS'a yazıldı. Sonuç için lounge_wait_task kullanın (Mod B). source_verified=false — onay kapısı uygulanabilir."
            }));
        }

        self.wait_until_done_or_background(&task_id, session_id, timeout_limit, cancel_rx.as_mut())
            .await
    }

    async fn wait_until_done_or_background(
        &self,
        task_id: &str,
        session_id: &str,
        timeout_limit: Duration,
        mut cancel_rx: Option<&mut watch::Receiver<Option<CancelKind>>>,
    ) -> Result<Value> {
        let poll = Duration::from_millis(50);
        let deadline_ms = self
            .clock
            .now_ms()
            .saturating_add(timeout_limit.as_millis() as u64);

        let mut nats_rx = self.subscribe_terminal(task_id);
        // Sender kapandıysa kolu tamamen bırak (pending future → CPU yok).
        let mut nats_alive = !self.skip_nats;

        loop {
            let pending_cancel = cancel_rx.as_ref().and_then(|rx| *rx.borrow());
            if let Some(kind) = pending_cancel {
                return self.handle_cancel(task_id, session_id, kind).await;
            }

            if let Some(payload) = self.try_read_result(task_id, session_id)? {
                return Ok(payload);
            }

            if self.clock.now_ms() >= deadline_ms {
                let _ = self.store.mark_a2a_wait_timeout(task_id);
                return Ok(json!({
                    "status": "backgrounded",
                    "task_id": task_id,
                    "message": format!(
                        "İşlem {}sn eşiğini aştı, arka planda devam ediyor. Sonucu lounge_wait_task ile alın; sonuç gelmeden kullanıcıya bitti demeyin.",
                        timeout_limit.as_secs()
                    ),
                    "timeout_limit_secs": timeout_limit.as_secs(),
                    "hint": "Call lounge_wait_task(task_id) — do not tell the user the work finished until wait returns completed/failed."
                }));
            }

            let slice = poll.min(Duration::from_millis(
                deadline_ms.saturating_sub(self.clock.now_ms()).max(1),
            ));
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

    fn try_read_result(&self, task_id: &str, session_id: &str) -> Result<Option<Value>> {
        if !self.store.session_can_read_a2a_task(task_id, session_id)? {
            return Err(anyhow!("yetkisiz oturum: görev okunamaz"));
        }
        let status = match self.store.a2a_task_status(task_id)? {
            Some(s) => s,
            None => return Ok(None),
        };
        // Completed/Failed/Cancelled + NeedsHuman/Expired/Timeout → wait biter (sonsuz still_running yok).
        if matches!(
            status,
            TaskStatus::Completed
                | TaskStatus::Failed
                | TaskStatus::Cancelled
                | TaskStatus::NeedsHuman
                | TaskStatus::Expired
                | TaskStatus::Timeout
        ) {
            let result = self.store.a2a_task_result(task_id)?;
            return Ok(Some(json!({
                "status": status.as_str().to_ascii_lowercase(),
                "task_id": task_id,
                "result": result.as_ref().and_then(|s| serde_json::from_str::<Value>(s).ok()),
                "result_text": result,
            })));
        }
        Ok(None)
    }

    async fn handle_cancel(
        &self,
        task_id: &str,
        session_id: &str,
        kind: CancelKind,
    ) -> Result<Value> {
        match kind {
            CancelKind::UserStop => {
                self.store.cancel_a2a_task(task_id, session_id)?;
                let mut control_stop = false;
                let mut control_stop_error: Option<String> = None;
                if !self.skip_nats || self.control_publisher_forced {
                    let publisher = Arc::clone(&self.control_publisher);
                    let tid = task_id.to_string();
                    let sid = session_id.to_string();
                    match tokio::task::spawn_blocking(move || {
                        publisher.publish_control_stop(&tid, &sid)
                    })
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
            CancelKind::DeadlineExceeded => {
                let _ = self.store.mark_a2a_wait_timeout(task_id);
                Ok(json!({
                    "status": "backgrounded",
                    "task_id": task_id,
                    "reason": "deadline exceeded",
                    "message": "İstemci zaman aşımı — görev arka planda sürüyor. lounge_wait_task ile sonucu alın.",
                    "hint": "Connection stays open; call lounge_wait_task in this session."
                }))
            }
            CancelKind::Other => Ok(json!({
                "status": "cancelled",
                "task_id": task_id,
                "reason": "other",
            })),
        }
    }

    /// Bounded wait — eşik `timeout_limit` (istemci tablosu); istenen `timeout_ms` ile min.
    pub async fn wait_task(
        &self,
        session_id: &str,
        client_name: &str,
        task_id: &str,
        timeout_ms: Option<u64>,
        mut cancel_rx: Option<watch::Receiver<Option<CancelKind>>>,
    ) -> Result<Value> {
        let timeout_limit = self.timeouts.timeout_limit(client_name);
        let requested = timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(timeout_limit);
        let budget = requested.min(timeout_limit);

        if !self.store.session_can_read_a2a_task(task_id, session_id)? {
            return Err(anyhow!("yetkisiz oturum: görev okunamaz"));
        }

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
                if kind == CancelKind::UserStop {
                    return self.handle_cancel(task_id, session_id, kind).await;
                }
            }

            if let Some(payload) = self.try_read_result(task_id, session_id)? {
                return Ok(payload);
            }

            if self.clock.now_ms() >= deadline_ms {
                let status = self
                    .store
                    .a2a_task_status(task_id)?
                    .unwrap_or(TaskStatus::WaitTimeoutReached);
                return Ok(json!({
                    "status": "still_running",
                    "task_id": task_id,
                    "task_status": status.as_str(),
                    "retry_after_ms": 1000,
                    "timeout_limit_secs": timeout_limit.as_secs(),
                    "message": "Görev hâlâ çalışıyor — lounge_wait_task tekrar çağrılabilir."
                }));
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
            let nc = nats::connect(&url).map_err(|e| anyhow!("NATS connect: {e}"))?;
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
            let nc = nats::connect(&url).map_err(|e| anyhow!("NATS connect: {e}"))?;
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
    async fn needs_human_ends_wait_task() {
        let (orch, _clock, session) = orch_manual();
        let mut task = LoungeTask::new("mcp:cursor", "p", "t");
        task.session_id = Some(session.clone());
        orch.store().admit_a2a_task(&mut task, 10).unwrap();
        orch.store()
            .set_a2a_task_status(&task.id, TaskStatus::NeedsHuman)
            .unwrap();
        let out = orch
            .wait_task(&session, "cursor", &task.id, Some(50), None)
            .await
            .unwrap();
        assert_eq!(out["status"], "needs_human");
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
}
