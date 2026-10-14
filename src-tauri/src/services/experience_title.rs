//! Async experience title generation via Lounge LMR (gemma-2-2b-it).
//!
//! Pipeline: **hygiene → LLM → hygiene** (LLM only sees masked content; regex
//! re-applied to output). Concurrency 1. Agent/user titles are never overwritten.
//! Garbage LLM output is stored as `title_source=fallback` (never `llm`).
//!
//! # Constants (OPEN: bounded values, documented)
//! - `TITLE_MAX_CHARS` = 80
//! - `TITLE_LMR_TIMEOUT` = 12s
//! - `TITLE_MAX_RETRIES` = 3
//! - `TITLE_BACKOFF_SECS` = [2, 5, 15]
//! - Model: `gemma-2-2b-it` (override `LOUNGE_TITLE_MODEL`)
//! - Endpoint: Lounge LMR `127.0.0.1:18790`
//!
//! Retry state is persisted on the experience row (`title_attempts`,
//! `title_next_retry_at`, `title_give_up`) so the backlog shrinks when LMR is down.
//! Upgrade backfill enqueue runs once (`migrations.title_backfill_enqueue_v1`).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Result};
use tokio::sync::{Mutex, Notify, Semaphore};

use crate::bridge::hygiene::{clean_title_output, fallback_title, scrub};
use crate::db::{ExperienceStore, MIGRATION_TITLE_BACKFILL_ENQUEUE_V1};
use crate::models::{TitleSource, TITLE_MAX_CHARS};
use crate::services::lounge_ollama_endpoint;

/// Max title length (chars).
pub const TITLE_MAX_LEN: usize = TITLE_MAX_CHARS;
/// Strict LMR HTTP timeout for title generation.
pub const TITLE_LMR_TIMEOUT: Duration = Duration::from_secs(12);
/// Bounded retries when LMR is down / times out / HTTP errors.
pub const TITLE_MAX_RETRIES: u32 = 3;
/// Backoff intervals (seconds) between retries.
pub const TITLE_BACKOFF_SECS: [u64; 3] = [2, 5, 15];
/// Default Gemma-2 instruct model on LMR.
pub const TITLE_MODEL_DEFAULT: &str = "gemma-2-2b-it";
/// Single in-flight title job.
pub const TITLE_CONCURRENCY: usize = 1;

const TITLE_SYSTEM_PROMPT: &str = "You write short experience titles. Reply with ONLY the title text — no quotes, no markdown, no prefix. Same language as the content. Max 80 characters.";

static TITLE_BACKLOG: AtomicU64 = AtomicU64::new(0);

/// Expose in-memory queue backlog for Health-1.
pub fn title_backlog_count() -> u64 {
    TITLE_BACKLOG.load(Ordering::Relaxed)
}

fn title_model() -> String {
    std::env::var("LOUNGE_TITLE_MODEL").unwrap_or_else(|_| TITLE_MODEL_DEFAULT.into())
}

thread_local! {
    static TEST_BACKOFF_MS: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
}

pub fn next_retry_delay(attempt_index: u32) -> Duration {
    if let Some(ms) = TEST_BACKOFF_MS.with(|c| c.get()) {
        return Duration::from_millis(ms);
    }
    Duration::from_secs(
        TITLE_BACKOFF_SECS
            .get(attempt_index as usize)
            .copied()
            .unwrap_or(*TITLE_BACKOFF_SECS.last().unwrap_or(&15)),
    )
}

/// Test-only: force all backoff sleeps to `ms` (None restores production constants).
pub fn set_test_backoff_ms(ms: Option<u64>) {
    TEST_BACKOFF_MS.with(|c| c.set(ms));
}

fn global_queue() -> &'static TitleQueue {
    static Q: OnceLock<TitleQueue> = OnceLock::new();
    Q.get_or_init(TitleQueue::new)
}

#[derive(Clone)]
struct TitleJob {
    id: String,
    content: String,
}

struct TitleQueue {
    store: Mutex<Option<ExperienceStore>>,
    endpoint: Mutex<Option<String>>,
    pending: Mutex<VecDeque<TitleJob>>,
    notify: Notify,
    started: Mutex<bool>,
    /// Ensures concurrency == 1 across the worker.
    gate: Semaphore,
}

impl TitleQueue {
    fn new() -> Self {
        Self {
            store: Mutex::new(None),
            endpoint: Mutex::new(None),
            pending: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
            started: Mutex::new(false),
            gate: Semaphore::new(TITLE_CONCURRENCY),
        }
    }

    fn set_backlog(pending_len: usize) {
        TITLE_BACKLOG.store(pending_len as u64, Ordering::Relaxed);
    }
}

/// Attach the experience store and start the concurrency-1 worker (idempotent).
pub async fn ensure_title_worker(store: ExperienceStore) {
    let q = global_queue();
    {
        let mut slot = q.store.lock().await;
        *slot = Some(store.clone());
    }
    let mut started = q.started.lock().await;
    if *started {
        return;
    }
    *started = true;
    drop(started);
    let q_worker = global_queue();
    tokio::spawn(async move {
        worker_loop(q_worker).await;
    });
}

/// Test helper: override LMR endpoint used by the global worker.
pub async fn set_title_endpoint_override_for_tests(endpoint: Option<String>) {
    let q = global_queue();
    *q.endpoint.lock().await = endpoint;
}

/// Enqueue title generation. No-op if title_source is agent|user (never overwrite).
pub async fn enqueue_title(store: &ExperienceStore, id: &str, content: &str, source: TitleSource) {
    if source == TitleSource::Agent || source == TitleSource::User {
        return;
    }
    ensure_title_worker(store.clone()).await;
    let q = global_queue();
    {
        let mut pending = q.pending.lock().await;
        if pending.iter().any(|j| j.id == id) {
            return;
        }
        pending.push_back(TitleJob {
            id: id.to_string(),
            content: content.to_string(),
        });
        TitleQueue::set_backlog(pending.len());
    }
    q.notify.notify_one();
}

/// On first start after this version only: enqueue title generation for rows lacking a title.
pub async fn backfill_missing_titles(store: &ExperienceStore) {
    ensure_title_worker(store.clone()).await;
    match store
        .settings_flag_done(MIGRATION_TITLE_BACKFILL_ENQUEUE_V1.into())
        .await
    {
        Ok(true) => return,
        Ok(false) => {}
        Err(err) => {
            log::warn!("title backfill flag read failed: {err}");
            return;
        }
    }
    let ids = match store.list_ids_needing_title().await {
        Ok(v) => v,
        Err(err) => {
            log::warn!("title backfill list failed: {err}");
            return;
        }
    };
    for (id, content) in ids {
        enqueue_title(store, &id, &content, TitleSource::Fallback).await;
    }
    if let Err(err) = store
        .set_settings_flag(MIGRATION_TITLE_BACKFILL_ENQUEUE_V1.into())
        .await
    {
        log::warn!("title backfill flag write failed: {err}");
    }
}

async fn worker_loop(q: &'static TitleQueue) {
    loop {
        let job = {
            let mut pending = q.pending.lock().await;
            let job = pending.pop_front();
            TitleQueue::set_backlog(pending.len());
            job
        };
        let Some(job) = job else {
            q.notify.notified().await;
            continue;
        };
        let store = {
            let guard = q.store.lock().await;
            guard.clone()
        };
        let Some(store) = store else {
            // Dropped: no store — backlog already decremented on pop.
            continue;
        };
        let endpoint = {
            let guard = q.endpoint.lock().await;
            guard.clone().unwrap_or_else(lounge_ollama_endpoint)
        };
        let _permit = q.gate.acquire().await.expect("title concurrency semaphore");
        if let Err(err) = generate_and_apply(&store, &endpoint, &job).await {
            log::warn!("title gen {}: {err}", job.id);
        }
    }
}

/// Process one job (also used directly by tests without the global loop).
pub async fn generate_and_apply_for_tests(
    store: &ExperienceStore,
    endpoint: &str,
    id: &str,
    content: &str,
) -> Result<()> {
    generate_and_apply(
        store,
        endpoint,
        &TitleJob {
            id: id.into(),
            content: content.into(),
        },
    )
    .await
}

async fn generate_and_apply(store: &ExperienceStore, endpoint: &str, job: &TitleJob) -> Result<()> {
    // Never overwrite agent/user titles (re-check at apply time).
    if let Some(rec) = store.get_record(job.id.clone()).await? {
        if rec.title_source == TitleSource::Agent || rec.title_source == TitleSource::User {
            // Dropped job — backlog already decremented when popped from queue.
            return Ok(());
        }
        if rec.title_source == TitleSource::Llm && !rec.title.trim().is_empty() {
            return Ok(());
        }
    }

    let mut last_err: Option<TitleError> = None;
    for attempt in 0..TITLE_MAX_RETRIES {
        match title_pipeline_at(endpoint, &job.content).await {
            Ok((title, source)) => {
                store
                    .set_experience_title(job.id.clone(), title, source, false)
                    .await?;
                store.clear_title_retry(job.id.clone()).await?;
                return Ok(());
            }
            Err(err) => {
                let next = chrono::Utc::now()
                    + chrono::Duration::from_std(next_retry_delay(attempt))
                        .unwrap_or_else(|_| chrono::Duration::seconds(15));
                let next_s = next.to_rfc3339();
                let attempts = attempt + 1;
                let _ = store
                    .record_title_retry(job.id.clone(), attempts, next_s)
                    .await;
                last_err = Some(err);
                tokio::time::sleep(next_retry_delay(attempt)).await;
            }
        }
    }

    // Give up: persist fallback so backlog shrinks.
    let masked = scrub(&job.content);
    let title = fallback_title(&masked.text, TITLE_MAX_LEN);
    store
        .set_experience_title(job.id.clone(), title, TitleSource::Fallback, false)
        .await?;
    store.mark_title_give_up(job.id.clone()).await?;
    Err(last_err
        .map(|e| anyhow!(e))
        .unwrap_or_else(|| anyhow!("title generation failed")))
}

/// Typed LMR title errors — timeout vs HTTP status vs connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TitleError {
    Timeout,
    Http(u16),
    Connect(String),
    Other(String),
}

impl std::fmt::Display for TitleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "title chat timed out"),
            Self::Http(code) => write!(f, "title chat HTTP {code}"),
            Self::Connect(msg) => write!(f, "title chat connection failed: {msg}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for TitleError {}

/// Finalize LLM raw output: hygiene → clean. Garbage → fallback source (never llm).
pub fn finalize_title_from_llm_output(content: &str, raw_llm: &str) -> (String, TitleSource) {
    let masked_content = scrub(content);
    let after = scrub(raw_llm);
    match clean_title_output(&after.text, TITLE_MAX_LEN) {
        Some(t) => (t, TitleSource::Llm),
        None => (
            fallback_title(&masked_content.text, TITLE_MAX_LEN),
            TitleSource::Fallback,
        ),
    }
}

/// Testable entry: hygiene → generator → hygiene → clean.
/// Returns `(title, source)` where garbage/errors yield Fallback (never Llm for garbage).
pub async fn title_pipeline_with_generator<F, Fut>(
    content: &str,
    generate: F,
) -> Result<(String, TitleSource), TitleError>
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<String, TitleError>>,
{
    let masked = scrub(content);
    match generate(masked.text.clone()).await {
        Ok(raw) => Ok(finalize_title_from_llm_output(content, &raw)),
        Err(err) => Err(err),
    }
}

async fn title_pipeline_at(
    endpoint: &str,
    content: &str,
) -> Result<(String, TitleSource), TitleError> {
    title_pipeline_with_generator(content, |masked| async move {
        generate_title_at(endpoint, &masked).await
    })
    .await
}

/// Single LMR call (hygiene already applied to `masked_content`).
pub async fn generate_title_once(masked_content: &str) -> Result<String, TitleError> {
    generate_title_at(&lounge_ollama_endpoint(), masked_content).await
}

/// Same as `generate_title_once` but against an explicit endpoint (tests / overrides).
pub async fn generate_title_at(endpoint: &str, masked_content: &str) -> Result<String, TitleError> {
    let model = title_model();
    chat_title(endpoint, &model, masked_content).await
}

async fn chat_title(endpoint: &str, model: &str, user: &str) -> Result<String, TitleError> {
    let url = format!("{endpoint}/api/chat");
    let client = reqwest::Client::builder()
        .timeout(TITLE_LMR_TIMEOUT)
        .build()
        .map_err(|e| TitleError::Other(format!("title chat client: {e}")))?;
    let body = serde_json::json!({
        "model": model,
        "stream": false,
        "messages": [
            {"role": "system", "content": TITLE_SYSTEM_PROMPT},
            {"role": "user", "content": user}
        ]
    });

    let response = match client.post(&url).json(&body).send().await {
        Ok(r) => r,
        Err(err) => {
            if err.is_timeout() {
                return Err(TitleError::Timeout);
            }
            if err.is_connect() {
                return Err(TitleError::Connect(err.to_string()));
            }
            // reqwest may report timeout wrapped; check source chain text carefully.
            let msg = err.to_string().to_ascii_lowercase();
            if msg.contains("timed out") || msg.contains("timeout") {
                return Err(TitleError::Timeout);
            }
            return Err(TitleError::Other(err.to_string()));
        }
    };
    let status = response.status();
    if !status.is_success() {
        return Err(TitleError::Http(status.as_u16()));
    }
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| TitleError::Other(format!("title chat JSON: {e}")))?;
    let content = payload
        .pointer("/message/content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| TitleError::Other("title response missing message.content".into()))?;
    Ok(content.to_string())
}

/// Whether agent/user titles may be overwritten (always false).
pub fn may_overwrite(source: TitleSource) -> bool {
    !matches!(source, TitleSource::Agent | TitleSource::User)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn stub_chat(status: u16, body: &str, delay_ms: u64) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = body.to_string();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = stream.read(&mut buf).await;
                if delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
                let response = format!(
                    "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    if status == 200 { "OK" } else { "ERR" },
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        port
    }

    fn chat_ok_body(title: &str) -> String {
        serde_json::json!({
            "message": {"role": "assistant", "content": title}
        })
        .to_string()
    }

    fn sample_record(
        id: impl Into<String>,
        body: &str,
        title: &str,
        source: TitleSource,
    ) -> crate::models::ExperienceRecord {
        crate::models::ExperienceRecord {
            id: id.into(),
            project_id: "p".into(),
            agent_id: "a".into(),
            topic: body.into(),
            solution_summary: body.into(),
            title: title.into(),
            title_source: source,
            ..Default::default()
        }
    }

    #[test]
    fn never_overwrite_agent_or_user() {
        assert!(!may_overwrite(TitleSource::Agent));
        assert!(!may_overwrite(TitleSource::User));
        assert!(may_overwrite(TitleSource::Llm));
        assert!(may_overwrite(TitleSource::Fallback));
    }

    #[test]
    fn fallback_truncation_unit() {
        let t = fallback_title(&"alpha ".repeat(30), 80);
        assert!(t.chars().count() <= 80);
        assert!(t.ends_with("alpha"));
    }

    #[test]
    fn finalize_garbage_is_fallback_never_llm() {
        let (title, source) = finalize_title_from_llm_output("short note about redis pool", "n/a");
        assert_eq!(source, TitleSource::Fallback);
        assert!(title.to_ascii_lowercase().contains("redis") || title.contains("short"));
    }

    #[tokio::test]
    async fn pipeline_hygiene_before_llm() {
        let seen = Arc::new(Mutex::new(String::new()));
        let seen2 = seen.clone();
        let (title, source) = title_pipeline_with_generator(
            "fix to 192.168.0.5:8080 with sk-abcdefghijklmnopqrstuvwxyz0123",
            |masked| async move {
                *seen2.lock().await = masked.clone();
                assert!(masked.contains("<device-ip>:8080"));
                assert!(masked.contains("<redacted-token>"));
                assert!(!masked.contains("192.168.0.5"));
                Ok("Secure local service fix".into())
            },
        )
        .await
        .unwrap();
        assert_eq!(source, TitleSource::Llm);
        assert_eq!(title, "Secure local service fix");
        let fed = seen.lock().await;
        assert!(fed.contains("<device-ip>"));
    }

    #[tokio::test]
    async fn pipeline_rejects_garbage_uses_fallback() {
        let (title, source) =
            title_pipeline_with_generator("short note about redis pool", |_| async {
                Ok("n/a".into())
            })
            .await
            .unwrap();
        assert_eq!(source, TitleSource::Fallback);
        assert!(title.to_ascii_lowercase().contains("redis") || title.contains("short"));
    }

    #[tokio::test]
    async fn stub_lmr_success() {
        let body = chat_ok_body("Fix ephemeral NATS port");
        let port = stub_chat(200, &body, 0).await;
        let endpoint = format!("http://127.0.0.1:{port}");
        let title = generate_title_at(&endpoint, "NATS bind conflict resolved with -p -1")
            .await
            .expect("success");
        let cleaned = clean_title_output(&title, 80).unwrap();
        assert_eq!(cleaned, "Fix ephemeral NATS port");
    }

    #[tokio::test]
    async fn stub_lmr_http_500_exact_status() {
        let port = stub_chat(500, "{\"error\":\"boom\"}", 0).await;
        let endpoint = format!("http://127.0.0.1:{port}");
        let err = generate_title_at(&endpoint, "content").await.unwrap_err();
        assert_eq!(err, TitleError::Http(500), "exact status required: {err}");
    }

    #[tokio::test]
    async fn stub_lmr_garbage_output() {
        let body = chat_ok_body("As an AI I cannot summarize this");
        let port = stub_chat(200, &body, 0).await;
        let endpoint = format!("http://127.0.0.1:{port}");
        let raw = generate_title_at(&endpoint, "something useful about vault")
            .await
            .unwrap();
        assert!(clean_title_output(&raw, 80).is_none());
    }

    #[tokio::test]
    async fn stub_lmr_timeout_is_timeout_kind() {
        let body = chat_ok_body("late title");
        let port = stub_chat(200, &body, 20_000).await;
        let endpoint = format!("http://127.0.0.1:{port}");
        let err = generate_title_at(&endpoint, "content").await.unwrap_err();
        assert_eq!(
            err,
            TitleError::Timeout,
            "must be Timeout, not connect/other: {err}"
        );
    }

    #[tokio::test]
    async fn connection_refused_is_not_timeout() {
        // Nothing listening — must be Connect/Other, never Timeout.
        let err = generate_title_at("http://127.0.0.1:9", "content")
            .await
            .unwrap_err();
        assert_ne!(
            err,
            TitleError::Timeout,
            "refused must not look like timeout"
        );
        assert!(
            matches!(err, TitleError::Connect(_) | TitleError::Other(_)),
            "unexpected: {err}"
        );
    }

    #[tokio::test]
    async fn worker_garbage_stores_fallback_source() {
        let store = ExperienceStore::memory().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let rec = sample_record(
            id.clone(),
            "redis pool tuning notes",
            &fallback_title("redis pool tuning notes", 80),
            TitleSource::Fallback,
        );
        store.insert_record(rec).await.unwrap();

        let body = chat_ok_body("n/a");
        let port = stub_chat(200, &body, 0).await;
        let endpoint = format!("http://127.0.0.1:{port}");
        generate_and_apply_for_tests(&store, &endpoint, &id, "redis pool tuning notes")
            .await
            .unwrap();
        let loaded = store.get_record(id).await.unwrap().unwrap();
        assert_eq!(
            loaded.title_source,
            TitleSource::Fallback,
            "garbage must never land as llm"
        );
        assert!(!loaded.title.is_empty());
    }

    #[tokio::test]
    async fn worker_never_overwrites_agent_or_user() {
        let store = ExperienceStore::memory().unwrap();
        for source in [TitleSource::Agent, TitleSource::User] {
            let id = uuid::Uuid::new_v4().to_string();
            let rec = sample_record(id.clone(), "keep me", "Agent Title", source);
            store.insert_record(rec).await.unwrap();

            let body = chat_ok_body("LLM should not win");
            let port = stub_chat(200, &body, 0).await;
            let endpoint = format!("http://127.0.0.1:{port}");
            generate_and_apply_for_tests(&store, &endpoint, &id, "keep me")
                .await
                .unwrap();
            let loaded = store.get_record(id).await.unwrap().unwrap();
            assert_eq!(loaded.title, "Agent Title");
            assert_eq!(loaded.title_source, source);
        }
    }

    #[tokio::test]
    async fn retry_backoff_persists_and_gives_up() {
        set_test_backoff_ms(Some(5));
        let store = ExperienceStore::memory().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let rec = sample_record(
            id.clone(),
            "needs title eventually",
            "needs title eventually",
            TitleSource::Fallback,
        );
        store.insert_record(rec).await.unwrap();

        let started = std::time::Instant::now();
        let endpoint = "http://127.0.0.1:9";
        let _ = generate_and_apply_for_tests(&store, endpoint, &id, "needs title eventually").await;
        let elapsed = started.elapsed();
        set_test_backoff_ms(None);

        // 3 retries × 5ms backoff (+ connect fails) — must not use production 2+5+15s.
        assert!(
            elapsed < Duration::from_secs(3),
            "backoff override not applied: {elapsed:?}"
        );
        // Documented production delays still map correctly.
        assert_eq!(TITLE_BACKOFF_SECS, [2, 5, 15]);

        let (attempts, next, give_up) = store.title_retry_state(id.clone()).await.unwrap();
        assert!(give_up, "must give up after max retries");
        assert_eq!(attempts, TITLE_MAX_RETRIES);
        assert!(next.is_none(), "give-up clears next_retry_at");
        let loaded = store.get_record(id).await.unwrap().unwrap();
        assert_eq!(loaded.title_source, TitleSource::Fallback);
        assert_eq!(store.count_title_backlog().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn backfill_runs_once_per_upgrade() {
        let store = ExperienceStore::memory().unwrap();
        let rec = sample_record(
            uuid::Uuid::new_v4().to_string(),
            "untitled row",
            "",
            TitleSource::Fallback,
        );
        store.insert_record(rec).await.unwrap();

        assert!(!store
            .settings_flag_done(MIGRATION_TITLE_BACKFILL_ENQUEUE_V1.into())
            .await
            .unwrap());
        backfill_missing_titles(&store).await;
        assert!(store
            .settings_flag_done(MIGRATION_TITLE_BACKFILL_ENQUEUE_V1.into())
            .await
            .unwrap());
        let backlog_after_first = title_backlog_count();
        // Second boot must not re-enqueue.
        backfill_missing_titles(&store).await;
        assert_eq!(title_backlog_count(), backlog_after_first);
    }

    #[tokio::test]
    async fn concurrency_gate_permits_only_one() {
        assert_eq!(TITLE_CONCURRENCY, 1);
        let q = global_queue();
        let first = q.gate.acquire().await.expect("first permit");
        assert!(
            q.gate.try_acquire().is_err(),
            "second permit must be blocked while first is held"
        );
        drop(first);
        let second = q.gate.try_acquire();
        assert!(second.is_ok(), "permit available after drop");
    }

    #[tokio::test]
    async fn worker_processes_with_single_flight_stub() {
        let current = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let body = chat_ok_body("ok");
        let current_c = current.clone();
        let max_c = max.clone();
        tokio::spawn(async move {
            for _ in 0..3 {
                if let Ok((mut stream, _)) = listener.accept().await {
                    let cur = current_c.clone();
                    let mx = max_c.clone();
                    let body = body.clone();
                    tokio::spawn(async move {
                        let n = cur.fetch_add(1, Ordering::SeqCst) + 1;
                        mx.fetch_max(n, Ordering::SeqCst);
                        let mut buf = vec![0u8; 4096];
                        let _ = stream.read(&mut buf).await;
                        tokio::time::sleep(Duration::from_millis(150)).await;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        cur.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            }
        });

        let store = ExperienceStore::memory().unwrap();
        ensure_title_worker(store.clone()).await;
        set_title_endpoint_override_for_tests(Some(format!("http://127.0.0.1:{port}"))).await;

        for i in 0..3 {
            let body = format!("single flight {i}");
            let rec = sample_record(format!("cf-{i}"), &body, &body, TitleSource::Fallback);
            store.insert_record(rec).await.unwrap();
            enqueue_title(&store, &format!("cf-{i}"), &body, TitleSource::Fallback).await;
        }

        for _ in 0..80 {
            if title_backlog_count() == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        set_title_endpoint_override_for_tests(None).await;
        assert!(
            max.load(Ordering::SeqCst) <= TITLE_CONCURRENCY,
            "max concurrent LMR calls was {}",
            max.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn backoff_constants_match_documented() {
        assert_eq!(next_retry_delay(0), Duration::from_secs(2));
        assert_eq!(next_retry_delay(1), Duration::from_secs(5));
        assert_eq!(next_retry_delay(2), Duration::from_secs(15));
        assert_eq!(next_retry_delay(99), Duration::from_secs(15));
    }
}
