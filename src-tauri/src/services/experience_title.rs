//! Async experience title generation via Lounge LMR (gemma-2-2b-it).
//!
//! Pipeline: **hygiene → LLM → hygiene** (LLM only sees masked content; regex
//! re-applied to output). Concurrency 1. Agent/user titles are never overwritten.
//!
//! # Constants (OPEN: bounded values, documented)
//! - `TITLE_MAX_CHARS` = 80
//! - `TITLE_LMR_TIMEOUT` = 12s
//! - `TITLE_MAX_RETRIES` = 3
//! - `TITLE_BACKOFF_SECS` = [2, 5, 15]
//! - Model: `gemma-2-2b-it` (override `LOUNGE_TITLE_MODEL`)
//! - Endpoint: Lounge LMR `127.0.0.1:18790`

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use tokio::sync::{Mutex, Notify};

use crate::bridge::hygiene::{clean_title_output, fallback_title, scrub};
use crate::db::ExperienceStore;
use crate::models::{TitleSource, TITLE_MAX_CHARS};
use crate::services::lounge_ollama_endpoint;

/// Max title length (chars).
pub const TITLE_MAX_LEN: usize = TITLE_MAX_CHARS;
/// Strict LMR HTTP timeout for title generation.
pub const TITLE_LMR_TIMEOUT: Duration = Duration::from_secs(12);
/// Bounded retries when LMR is down / returns garbage / times out.
pub const TITLE_MAX_RETRIES: u32 = 3;
/// Backoff intervals (seconds) between retries.
pub const TITLE_BACKOFF_SECS: [u64; 3] = [2, 5, 15];
/// Default Gemma-2 instruct model on LMR.
pub const TITLE_MODEL_DEFAULT: &str = "gemma-2-2b-it";
/// Single in-flight title job.
pub const TITLE_CONCURRENCY: usize = 1;

const TITLE_SYSTEM_PROMPT: &str = "You write short experience titles. Reply with ONLY the title text — no quotes, no markdown, no prefix. Same language as the content. Max 80 characters.";

static TITLE_BACKLOG: AtomicU64 = AtomicU64::new(0);

/// Expose backlog count for Health-1 (out of scope UI; counter only).
pub fn title_backlog_count() -> u64 {
    TITLE_BACKLOG.load(Ordering::Relaxed)
}

fn title_model() -> String {
    std::env::var("LOUNGE_TITLE_MODEL").unwrap_or_else(|_| TITLE_MODEL_DEFAULT.into())
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
    pending: Mutex<VecDeque<TitleJob>>,
    notify: Notify,
    started: Mutex<bool>,
}

impl TitleQueue {
    fn new() -> Self {
        Self {
            store: Mutex::new(None),
            pending: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
            started: Mutex::new(false),
        }
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

/// Enqueue title generation for a record that still has a fallback/empty title.
/// No-op if title_source is agent|user (never overwrite).
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
        TITLE_BACKLOG.store(pending.len() as u64, Ordering::Relaxed);
    }
    q.notify.notify_one();
}

/// On first start after this version: enqueue title generation for rows lacking a title.
pub async fn backfill_missing_titles(store: &ExperienceStore) {
    ensure_title_worker(store.clone()).await;
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
}

async fn worker_loop(q: &'static TitleQueue) {
    loop {
        let job = {
            let mut pending = q.pending.lock().await;
            let job = pending.pop_front();
            TITLE_BACKLOG.store(pending.len() as u64, Ordering::Relaxed);
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
            continue;
        };
        if let Err(err) = generate_and_apply(&store, &job).await {
            log::warn!("title gen {}: {err}", job.id);
        }
    }
}

async fn generate_and_apply(store: &ExperienceStore, job: &TitleJob) -> Result<()> {
    // Never overwrite agent/user titles (re-check at apply time).
    if let Some(rec) = store.get_record(job.id.clone()).await? {
        if rec.title_source == TitleSource::Agent || rec.title_source == TitleSource::User {
            return Ok(());
        }
        if rec.title_source == TitleSource::Llm && !rec.title.trim().is_empty() {
            return Ok(());
        }
    }

    let masked = scrub(&job.content);
    let mut last_err = None;
    for attempt in 0..TITLE_MAX_RETRIES {
        match generate_title_once(&masked.text).await {
            Ok(title) => {
                let cleaned = scrub(&title);
                let final_title = clean_title_output(&cleaned.text, TITLE_MAX_LEN)
                    .unwrap_or_else(|| fallback_title(&masked.text, TITLE_MAX_LEN));
                store
                    .set_experience_title(
                        job.id.clone(),
                        final_title,
                        TitleSource::Llm,
                        false, // do not force over agent/user
                    )
                    .await?;
                return Ok(());
            }
            Err(err) => {
                last_err = Some(err);
                let wait = TITLE_BACKOFF_SECS
                    .get(attempt as usize)
                    .copied()
                    .unwrap_or(15);
                tokio::time::sleep(Duration::from_secs(wait)).await;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("title generation failed")))
}

/// Single LMR call (hygiene already applied to `masked_content`).
pub async fn generate_title_once(masked_content: &str) -> Result<String> {
    generate_title_at(&lounge_ollama_endpoint(), masked_content).await
}

/// Same as `generate_title_once` but against an explicit endpoint (tests / overrides).
pub async fn generate_title_at(endpoint: &str, masked_content: &str) -> Result<String> {
    let model = title_model();
    chat_title(endpoint, &model, masked_content).await
}

/// Testable entry: hygiene → (optional) LLM stub → hygiene → clean.
/// Returns `(title, source)` where source is Llm on success else Fallback.
pub async fn title_pipeline_with_generator<F, Fut>(
    content: &str,
    generate: F,
) -> (String, TitleSource)
where
    F: FnOnce(String) -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    let masked = scrub(content);
    match generate(masked.text.clone()).await {
        Ok(raw) => {
            let after = scrub(&raw);
            match clean_title_output(&after.text, TITLE_MAX_LEN) {
                Some(t) => (t, TitleSource::Llm),
                None => (
                    fallback_title(&masked.text, TITLE_MAX_LEN),
                    TitleSource::Fallback,
                ),
            }
        }
        Err(_) => (
            fallback_title(&masked.text, TITLE_MAX_LEN),
            TitleSource::Fallback,
        ),
    }
}

async fn chat_title(endpoint: &str, model: &str, user: &str) -> Result<String> {
    let url = format!("{endpoint}/api/chat");
    let client = reqwest::Client::builder()
        .timeout(TITLE_LMR_TIMEOUT)
        .build()
        .context("title chat client")?;
    let body = serde_json::json!({
        "model": model,
        "stream": false,
        "messages": [
            {"role": "system", "content": TITLE_SYSTEM_PROMPT},
            {"role": "user", "content": user}
        ]
    });
    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("title chat failed: {url}"))?;
    if !response.status().is_success() {
        anyhow::bail!("title chat HTTP {}", response.status());
    }
    let payload: serde_json::Value = response.json().await.context("title chat JSON")?;
    let content = payload
        .pointer("/message/content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("title response missing message.content"))?;
    Ok(content.to_string())
}

/// Whether agent/user titles may be overwritten (always false).
pub fn may_overwrite(source: TitleSource) -> bool {
    !matches!(source, TitleSource::Agent | TitleSource::User)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        .await;
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
            .await;
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
    async fn stub_lmr_http_500() {
        let port = stub_chat(500, "{\"error\":\"boom\"}", 0).await;
        let endpoint = format!("http://127.0.0.1:{port}");
        let err = generate_title_at(&endpoint, "content").await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("500") || msg.contains("HTTP"),
            "unexpected err: {msg}"
        );
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
    async fn stub_lmr_timeout() {
        let body = chat_ok_body("late title");
        let port = stub_chat(200, &body, 20_000).await;
        let endpoint = format!("http://127.0.0.1:{port}");
        let err = generate_title_at(&endpoint, "content").await.unwrap_err();
        let msg = err.to_string().to_ascii_lowercase();
        assert!(
            msg.contains("timeout") || msg.contains("timed out") || msg.contains("failed"),
            "unexpected err: {msg}"
        );
    }
}
