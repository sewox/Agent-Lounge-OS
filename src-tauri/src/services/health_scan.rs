//! Health-1a: full paged dead-symbol scan (concurrency 1).
//!
//! Streams Function+Method / CALLS pages (~2000/page) into SQLite staging rows,
//! verifies counts against Cypher `count()`, computes broken/unused via JOIN,
//! applies H2 filters, then atomically promotes staging → live. Cancel / count
//! mismatch → `partial=true` + `reason`, **without** destroying the prior good
//! snapshot.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, Notify, Semaphore};
use uuid::Uuid;

use super::health_filters::{mask_home_path, split_findings, HealthFilterContext};
use super::memory_bridge::{MemoryBridge, HEALTH_SCAN_PAGE_SIZE};
use crate::db::{canonicalize_repo_path, prefer_canonical, register_path_project, ExperienceStore};
use crate::models::{
    now_rfc3339, AstNode, CodeReference, DeadSymbol, HealthScanProgress, HealthScanResult,
};

pub const HEALTH_SCAN_EVENT: &str = "lounge://health-scan";
pub const HEALTH_SCAN_PROGRESS_EVENT: &str = "lounge://health-scan-progress";
pub const HEALTH_SCAN_CONCURRENCY: usize = 1;

const KIND_HEALTH_NODE: &str = "health_node";
const KIND_HEALTH_REF: &str = "health_reference";
const META_PARTIAL: &str = "__health_scan_partial__";
const META_HEALTH: &str = "__health_scan__";

pub const REASON_CANCELLED: &str = "cancelled";
pub const REASON_COUNT_MISMATCH: &str = "count_mismatch";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthScanPhase {
    Queued,
    Scanning,
    Done,
    Failed,
    Cancelled,
}

impl HealthScanPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Scanning => "scanning",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Scanning)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthScanJob {
    pub id: String,
    pub project_id: String,
    pub repo_path: String,
    pub status: HealthScanPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<HealthScanResult>,
    pub enqueued_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthScanEvent {
    pub job: HealthScanJob,
}

struct QueueInner {
    jobs: Vec<HealthScanJob>,
    cancels: std::collections::HashMap<String, Arc<AtomicBool>>,
}

pub struct HealthScanQueue {
    bridge: MemoryBridge,
    store: ExperienceStore,
    inner: Mutex<QueueInner>,
    wake: Notify,
    gate: Semaphore,
    started: AtomicBool,
}

fn global_queue() -> &'static Mutex<Option<Arc<HealthScanQueue>>> {
    static Q: OnceLock<Mutex<Option<Arc<HealthScanQueue>>>> = OnceLock::new();
    Q.get_or_init(|| Mutex::new(None))
}

pub async fn install_health_scan_queue(queue: Arc<HealthScanQueue>) {
    let mut guard = global_queue().lock().await;
    *guard = Some(queue);
}

async fn global_health_queue() -> Option<Arc<HealthScanQueue>> {
    global_queue().lock().await.clone()
}

pub async fn enqueue_after_index(repo_path: &str, project_hint: &str) {
    if let Some(q) = global_health_queue().await {
        if let Err(err) = q.enqueue(repo_path, project_hint).await {
            log::warn!("health scan enqueue after index failed: {err}");
        }
    }
}

impl HealthScanQueue {
    pub fn new(bridge: MemoryBridge, store: ExperienceStore) -> Arc<Self> {
        Arc::new(Self {
            bridge,
            store,
            inner: Mutex::new(QueueInner {
                jobs: Vec::new(),
                cancels: std::collections::HashMap::new(),
            }),
            wake: Notify::new(),
            gate: Semaphore::new(HEALTH_SCAN_CONCURRENCY),
            started: AtomicBool::new(false),
        })
    }

    pub fn ensure_worker(self: &Arc<Self>, app: AppHandle) {
        if self
            .started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        let queue = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let started = Arc::clone(&queue);
            queue.worker_loop(app).await;
            started.started.store(false, Ordering::SeqCst);
        });
    }

    async fn worker_loop(self: Arc<Self>, app: AppHandle) {
        loop {
            let next_id = {
                let guard = self.inner.lock().await;
                guard
                    .jobs
                    .iter()
                    .find(|j| j.status == HealthScanPhase::Queued)
                    .map(|j| j.id.clone())
            };
            let Some(job_id) = next_id else {
                self.wake.notified().await;
                continue;
            };

            let Ok(permit) = self.gate.acquire().await else {
                break;
            };

            {
                let mut guard = self.inner.lock().await;
                if let Some(job) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
                    if job.status != HealthScanPhase::Queued {
                        drop(permit);
                        continue;
                    }
                    job.status = HealthScanPhase::Scanning;
                    job.updated_at = now_rfc3339();
                    emit_job(&app, job.clone());
                }
            }

            self.run_one(&app, &job_id).await;
            drop(permit);
        }
    }

    pub async fn enqueue(&self, repo_path: &str, project_hint: &str) -> Result<HealthScanJob> {
        let repo_path = repo_path.trim();
        if repo_path.is_empty() {
            bail!("repo_path boş");
        }
        let canonical_path = canonicalize_repo_path(repo_path);

        {
            let guard = self.inner.lock().await;
            if let Some(existing) = guard
                .jobs
                .iter()
                .find(|j| j.status.is_active() && j.repo_path == canonical_path)
            {
                return Ok(existing.clone());
            }
        }

        let project_id = resolve_project_id(&self.store, &canonical_path, project_hint).await?;
        let id = Uuid::new_v4().to_string();
        let now = now_rfc3339();
        let job = HealthScanJob {
            id: id.clone(),
            project_id,
            repo_path: canonical_path,
            status: HealthScanPhase::Queued,
            error: None,
            result: None,
            enqueued_at: now.clone(),
            updated_at: now,
        };
        {
            let mut guard = self.inner.lock().await;
            guard
                .cancels
                .insert(id.clone(), Arc::new(AtomicBool::new(false)));
            guard.jobs.push(job.clone());
            if guard.jobs.len() > 100 {
                let drop_n = guard.jobs.len() - 100;
                guard.jobs.drain(0..drop_n);
            }
        }
        self.wake.notify_one();
        Ok(job)
    }

    pub async fn cancel_job(&self, app: &AppHandle, job_id: &str) -> bool {
        let mut guard = self.inner.lock().await;
        if let Some(flag) = guard.cancels.get(job_id) {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(job) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
            if job.status.is_terminal() {
                return false;
            }
            if job.status == HealthScanPhase::Queued {
                job.status = HealthScanPhase::Cancelled;
                job.updated_at = now_rfc3339();
                job.result = Some(partial_result(
                    &job.project_id,
                    &job.repo_path,
                    "",
                    0,
                    0,
                    0,
                    0,
                    0,
                    REASON_CANCELLED,
                ));
                emit_job(app, job.clone());
            }
            return true;
        }
        false
    }

    pub async fn list_jobs(&self) -> Vec<HealthScanJob> {
        self.inner.lock().await.jobs.clone()
    }

    pub async fn latest_for_project(&self, project_id: &str) -> Option<HealthScanJob> {
        let guard = self.inner.lock().await;
        guard
            .jobs
            .iter()
            .rev()
            .find(|j| j.project_id == project_id || j.repo_path == project_id)
            .cloned()
    }

    async fn run_one(&self, app: &AppHandle, job_id: &str) {
        let (repo_path, project_id, cancel) = {
            let guard = self.inner.lock().await;
            let job = match guard.jobs.iter().find(|j| j.id == job_id) {
                Some(j) => j.clone(),
                None => return,
            };
            let cancel = guard
                .cancels
                .get(job_id)
                .cloned()
                .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
            (job.repo_path, job.project_id, cancel)
        };

        let progress_cb = {
            let app = app.clone();
            let job_id = job_id.to_string();
            let project_id = project_id.clone();
            Arc::new(move |progress: HealthScanProgress| {
                let mut p = progress;
                p.job_id = job_id.clone();
                p.project_id = project_id.clone();
                let _ = app.emit(HEALTH_SCAN_PROGRESS_EVENT, &p);
            }) as Arc<dyn Fn(HealthScanProgress) + Send + Sync>
        };

        let scan = run_health_scan(
            &self.bridge,
            &self.store,
            &repo_path,
            &project_id,
            cancel.clone(),
            Some(progress_cb),
        )
        .await;

        match scan {
            Ok(result) if result.partial => {
                self.finish_partial(app, job_id, result).await;
            }
            Ok(result) => self.finish_done(app, job_id, result).await,
            Err(err) => {
                self.finish_failed(app, job_id, mask_home_path(&err.to_string()))
                    .await;
            }
        }
    }

    async fn finish_done(&self, app: &AppHandle, job_id: &str, result: HealthScanResult) {
        let mut guard = self.inner.lock().await;
        if let Some(job) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
            if job.status == HealthScanPhase::Cancelled {
                return;
            }
            job.status = HealthScanPhase::Done;
            job.result = Some(result);
            job.error = None;
            job.updated_at = now_rfc3339();
            emit_job(app, job.clone());
        }
    }

    async fn finish_partial(&self, app: &AppHandle, job_id: &str, result: HealthScanResult) {
        let mut guard = self.inner.lock().await;
        if let Some(job) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
            job.status = if result.reason.as_deref() == Some(REASON_CANCELLED) {
                HealthScanPhase::Cancelled
            } else {
                HealthScanPhase::Done
            };
            job.result = Some(result);
            job.error = None;
            job.updated_at = now_rfc3339();
            emit_job(app, job.clone());
        }
    }

    async fn finish_failed(&self, app: &AppHandle, job_id: &str, error: String) {
        let mut guard = self.inner.lock().await;
        if let Some(job) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
            if job.status == HealthScanPhase::Cancelled {
                return;
            }
            job.status = HealthScanPhase::Failed;
            job.error = Some(error);
            job.updated_at = now_rfc3339();
            emit_job(app, job.clone());
        }
    }
}

fn emit_job(app: &AppHandle, job: HealthScanJob) {
    let _ = app.emit(HEALTH_SCAN_EVENT, HealthScanEvent { job });
}

#[allow(clippy::too_many_arguments)]
fn partial_result(
    project_id: &str,
    repo_path: &str,
    indexed_at: &str,
    symbols: u64,
    edges: u64,
    expected_symbols: u64,
    expected_edges: u64,
    pages: u64,
    reason: &str,
) -> HealthScanResult {
    HealthScanResult {
        project_id: project_id.to_string(),
        repo_path: mask_home_path(repo_path),
        indexed_at: indexed_at.to_string(),
        symbols_scanned: symbols,
        edges_scanned: edges,
        expected_symbols,
        expected_edges,
        pages,
        page_size: HEALTH_SCAN_PAGE_SIZE as u64,
        partial: true,
        reason: Some(reason.to_string()),
        kritik: Vec::new(),
        potansiyel: Vec::new(),
        error: None,
    }
}

async fn resolve_project_id(
    store: &ExperienceStore,
    repo_path: &str,
    project_hint: &str,
) -> Result<String> {
    let path = repo_path.to_string();
    let hint_owned = project_hint.trim().to_string();
    let conn = store.conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        crate::db::migrate_project_identity(&conn)?;
        if !path.trim().is_empty() {
            return register_path_project(
                &conn,
                if hint_owned.is_empty() {
                    "unknown"
                } else {
                    hint_owned.as_str()
                },
                &path,
                &[hint_owned.as_str()],
            );
        }
        if !hint_owned.is_empty() {
            return prefer_canonical(&conn, &hint_owned);
        }
        Ok(canonicalize_repo_path(&path))
    })
    .await
    .context("resolve project_id join")?
}

/// Core scan used by the queue and by unit tests (no Tauri required).
pub async fn run_health_scan(
    bridge: &MemoryBridge,
    store: &ExperienceStore,
    repo_path: &str,
    project_id: &str,
    cancel: Arc<AtomicBool>,
    progress: Option<Arc<dyn Fn(HealthScanProgress) + Send + Sync>>,
) -> Result<HealthScanResult> {
    let repo_path = canonicalize_repo_path(repo_path);
    let cbm_project = bridge
        .resolve_cbm_project_name(&repo_path)
        .await
        .unwrap_or_else(|| {
            std::path::Path::new(&repo_path)
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| project_id.to_string())
        });

    let indexed_at = require_indexed_at(store, project_id).await?;
    let page_size = HEALTH_SCAN_PAGE_SIZE;

    let expected_fn = bridge.fetch_label_count(&cbm_project, "Function").await?;
    let expected_method = bridge.fetch_label_count(&cbm_project, "Method").await?;
    let expected_symbols = expected_fn.saturating_add(expected_method);
    let expected_edges = bridge.fetch_calls_count(&cbm_project).await?;

    // Clear any leftover staging from a prior interrupted scan.
    clear_staging(store, project_id).await?;

    let mut symbols_scanned = 0u64;
    let mut edges_scanned = 0u64;
    let mut pages = 0u64;

    // --- Page Function + Method into staging (streamed, transactional) ---
    for (label, kind) in [("Function", "function"), ("Method", "method")] {
        let mut skip = 0u64;
        let mut page_index = 0u64;
        loop {
            if cancel.load(Ordering::SeqCst) {
                clear_staging(store, project_id).await?;
                let result = partial_result(
                    project_id,
                    &repo_path,
                    &indexed_at,
                    symbols_scanned,
                    edges_scanned,
                    expected_symbols,
                    expected_edges,
                    pages,
                    REASON_CANCELLED,
                );
                store_partial_meta(store, project_id, &repo_path, &indexed_at, &result).await?;
                return Ok(result);
            }
            let batch = bridge
                .fetch_ast_nodes_page(&cbm_project, label, kind, skip, page_size)
                .await?;
            let n = batch.len() as u64;
            pages = pages.saturating_add(1);
            page_index = page_index.saturating_add(1);
            if n == 0 {
                break;
            }
            insert_nodes_page(store, project_id, &repo_path, &indexed_at, &batch).await?;
            symbols_scanned = symbols_scanned.saturating_add(n);
            if let Some(cb) = &progress {
                cb(HealthScanProgress {
                    label: label.to_string(),
                    page_index,
                    page_rows: n,
                    symbols_so_far: symbols_scanned,
                    edges_so_far: edges_scanned,
                    ..HealthScanProgress::default()
                });
            }
            // Advance by returned rows — CBM may token-truncate below page_size
            // while still having more data at the next offset.
            skip = skip.saturating_add(n);
            if n < page_size as u64 {
                // Short page with no further rows at this offset → done for label.
                // If truncation left has_more, the next fetch at `skip` continues.
                let more = bridge
                    .fetch_ast_nodes_page(&cbm_project, label, kind, skip, page_size)
                    .await?;
                if more.is_empty() {
                    break;
                }
                // Continue loop with the already-fetched next page by inserting now.
                pages = pages.saturating_add(1);
                page_index = page_index.saturating_add(1);
                let n2 = more.len() as u64;
                insert_nodes_page(store, project_id, &repo_path, &indexed_at, &more).await?;
                symbols_scanned = symbols_scanned.saturating_add(n2);
                if let Some(cb) = &progress {
                    cb(HealthScanProgress {
                        label: label.to_string(),
                        page_index,
                        page_rows: n2,
                        symbols_so_far: symbols_scanned,
                        edges_so_far: edges_scanned,
                        ..HealthScanProgress::default()
                    });
                }
                skip = skip.saturating_add(n2);
                if n2 < page_size as u64 {
                    break;
                }
            }
        }
    }

    // --- Page CALLS ---
    {
        let mut skip = 0u64;
        let mut page_index = 0u64;
        loop {
            if cancel.load(Ordering::SeqCst) {
                clear_staging(store, project_id).await?;
                let result = partial_result(
                    project_id,
                    &repo_path,
                    &indexed_at,
                    symbols_scanned,
                    edges_scanned,
                    expected_symbols,
                    expected_edges,
                    pages,
                    REASON_CANCELLED,
                );
                store_partial_meta(store, project_id, &repo_path, &indexed_at, &result).await?;
                return Ok(result);
            }
            let batch = bridge
                .fetch_call_edges_page(&cbm_project, skip, page_size)
                .await?;
            let n = batch.len() as u64;
            pages = pages.saturating_add(1);
            page_index = page_index.saturating_add(1);
            if n == 0 {
                break;
            }
            insert_edges_page(store, project_id, &repo_path, &indexed_at, &batch).await?;
            edges_scanned = edges_scanned.saturating_add(n);
            if let Some(cb) = &progress {
                cb(HealthScanProgress {
                    label: "CALLS".into(),
                    page_index,
                    page_rows: n,
                    symbols_so_far: symbols_scanned,
                    edges_so_far: edges_scanned,
                    ..HealthScanProgress::default()
                });
            }
            // Advance by returned rows — same token-truncation guard as symbols.
            skip = skip.saturating_add(n);
            if n < page_size as u64 {
                let more = bridge
                    .fetch_call_edges_page(&cbm_project, skip, page_size)
                    .await?;
                if more.is_empty() {
                    break;
                }
                pages = pages.saturating_add(1);
                page_index = page_index.saturating_add(1);
                let n2 = more.len() as u64;
                insert_edges_page(store, project_id, &repo_path, &indexed_at, &more).await?;
                edges_scanned = edges_scanned.saturating_add(n2);
                if let Some(cb) = &progress {
                    cb(HealthScanProgress {
                        label: "CALLS".into(),
                        page_index,
                        page_rows: n2,
                        symbols_so_far: symbols_scanned,
                        edges_so_far: edges_scanned,
                        ..HealthScanProgress::default()
                    });
                }
                skip = skip.saturating_add(n2);
                if n2 < page_size as u64 {
                    break;
                }
            }
        }
    }

    // Count guard — silent CBM truncation / SKIP bugs.
    if symbols_scanned != expected_symbols || edges_scanned != expected_edges {
        clear_staging(store, project_id).await?;
        let result = partial_result(
            project_id,
            &repo_path,
            &indexed_at,
            symbols_scanned,
            edges_scanned,
            expected_symbols,
            expected_edges,
            pages,
            REASON_COUNT_MISMATCH,
        );
        store_partial_meta(store, project_id, &repo_path, &indexed_at, &result).await?;
        return Ok(result);
    }

    // Promote staging → live atomically, then JOIN + filter + write dead.
    let project_key = project_id.to_string();
    let repo_for_db = repo_path.clone();
    let indexed_at_db = indexed_at.clone();
    let conn = store.conn.clone();
    let (kritik, potansiyel) = tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        promote_staging_atomic(&conn, &project_key, &repo_for_db, &indexed_at_db)?;
        let candidates = compute_dead_via_join(&conn, &project_key)?;
        let filter_ctx = HealthFilterContext::for_repo(&repo_for_db);
        let (kritik, potansiyel) = split_findings(&filter_ctx, candidates);
        let all: Vec<DeadSymbol> = kritik.iter().chain(potansiyel.iter()).cloned().collect();
        replace_dead_rows_atomic(&conn, &project_key, &repo_for_db, &indexed_at_db, &all)?;
        Ok::<_, anyhow::Error>((kritik, potansiyel))
    })
    .await
    .context("health scan promote join")??;

    Ok(HealthScanResult {
        project_id: project_id.to_string(),
        repo_path: mask_home_path(&repo_path),
        indexed_at,
        symbols_scanned,
        edges_scanned,
        expected_symbols,
        expected_edges,
        pages,
        page_size: page_size as u64,
        partial: false,
        reason: None,
        kritik: mask_findings(kritik),
        potansiyel: mask_findings(potansiyel),
        error: None,
    })
}

fn mask_findings(mut rows: Vec<DeadSymbol>) -> Vec<DeadSymbol> {
    for row in &mut rows {
        if let Some(file) = row.file.as_mut() {
            *file = mask_home_path(file);
        }
        if let Some(detail) = row.detail.as_mut() {
            *detail = mask_home_path(detail);
        }
        if let Some(last_ref) = row.last_ref.as_mut() {
            *last_ref = mask_home_path(last_ref);
        }
    }
    rows
}

async fn require_indexed_at(store: &ExperienceStore, project_id: &str) -> Result<String> {
    let project_id = project_id.to_string();
    let conn = store.conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        let existing: Option<String> = conn
            .query_row(
                r#"
                SELECT indexed_at FROM project_index
                WHERE project_id = ?1
                  AND kind NOT LIKE 'health_%'
                ORDER BY indexed_at DESC
                LIMIT 1
                "#,
                params![project_id],
                |row| row.get(0),
            )
            .optional()?;
        existing.ok_or_else(|| {
            anyhow::anyhow!(
                "indexed_at yok — önce index_workspace çalışmalı (project_id={project_id})"
            )
        })
    })
    .await
    .context("indexed_at join")?
}

async fn clear_staging(store: &ExperienceStore, project_id: &str) -> Result<()> {
    let project_id = project_id.to_string();
    let conn = store.conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        conn.execute(
            "DELETE FROM project_index WHERE project_id = ?1 AND kind IN (?2, ?3)",
            params![project_id, KIND_HEALTH_NODE, KIND_HEALTH_REF],
        )?;
        Ok(())
    })
    .await
    .context("clear staging join")?
}

async fn insert_nodes_page(
    store: &ExperienceStore,
    project_id: &str,
    repo_path: &str,
    indexed_at: &str,
    nodes: &[AstNode],
) -> Result<()> {
    let project_id = project_id.to_string();
    let repo_path = repo_path.to_string();
    let indexed_at = indexed_at.to_string();
    let nodes = nodes.to_vec();
    let conn = store.conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<()> {
            for node in &nodes {
                let name = if node.name.is_empty() {
                    node.id.as_str()
                } else {
                    node.name.as_str()
                };
                // Store absolute / as-provided paths (no masking).
                conn.execute(
                    r#"
                    INSERT INTO project_index (
                        id, project_id, repo_path, kind, name, file_path, line, target,
                        ref_count, detail, payload_json, indexed_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, '{}', ?11)
                    "#,
                    params![
                        Uuid::new_v4().to_string(),
                        project_id,
                        repo_path,
                        KIND_HEALTH_NODE,
                        name,
                        node.file.as_deref(),
                        node.line,
                        Some(node.id.as_str()).filter(|id| !id.is_empty()),
                        node.ref_count as i64,
                        Some(node.kind.as_str()).filter(|k| !k.is_empty()),
                        indexed_at,
                    ],
                )?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(err) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    })
    .await
    .context("insert nodes page join")?
}

async fn insert_edges_page(
    store: &ExperienceStore,
    project_id: &str,
    repo_path: &str,
    indexed_at: &str,
    edges: &[CodeReference],
) -> Result<()> {
    let project_id = project_id.to_string();
    let repo_path = repo_path.to_string();
    let indexed_at = indexed_at.to_string();
    let edges = edges.to_vec();
    let conn = store.conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<()> {
            for edge in &edges {
                conn.execute(
                    r#"
                    INSERT INTO project_index (
                        id, project_id, repo_path, kind, name, file_path, line, target,
                        ref_count, detail, payload_json, indexed_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, NULL, '{}', ?9)
                    "#,
                    params![
                        Uuid::new_v4().to_string(),
                        project_id,
                        repo_path,
                        KIND_HEALTH_REF,
                        edge.from_id,
                        edge.file.as_deref(),
                        edge.line,
                        edge.to_id,
                        indexed_at,
                    ],
                )?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(err) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    })
    .await
    .context("insert edges page join")?
}

async fn store_partial_meta(
    store: &ExperienceStore,
    project_id: &str,
    repo_path: &str,
    indexed_at: &str,
    result: &HealthScanResult,
) -> Result<()> {
    let project_id = project_id.to_string();
    let repo_path = repo_path.to_string();
    let indexed_at = indexed_at.to_string();
    let payload = serde_json::to_string(result)?;
    let conn = store.conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let op = (|| -> Result<()> {
            conn.execute(
                "DELETE FROM project_index WHERE project_id = ?1 AND kind = 'meta' AND name = ?2",
                params![project_id, META_PARTIAL],
            )?;
            conn.execute(
                r#"
                INSERT INTO project_index (
                    id, project_id, repo_path, kind, name, file_path, line, target,
                    ref_count, detail, payload_json, indexed_at
                ) VALUES (?1, ?2, ?3, 'meta', ?4, NULL, NULL, NULL, 0, NULL, ?5, ?6)
                "#,
                params![
                    Uuid::new_v4().to_string(),
                    project_id,
                    repo_path,
                    META_PARTIAL,
                    payload,
                    indexed_at,
                ],
            )?;
            Ok(())
        })();
        match op {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(err) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(err)
            }
        }
    })
    .await
    .context("store partial meta join")?
}

fn promote_staging_atomic(
    conn: &Connection,
    project_id: &str,
    repo_path: &str,
    indexed_at: &str,
) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let op = (|| -> Result<()> {
        // Replace live graph rows only — leave unrelated meta/flags alone except totals.
        conn.execute(
            "DELETE FROM project_index WHERE project_id = ?1 AND kind IN ('node', 'reference', 'dead', 'broken')",
            params![project_id],
        )?;
        conn.execute(
            "UPDATE project_index SET kind = 'node' WHERE project_id = ?1 AND kind = ?2",
            params![project_id, KIND_HEALTH_NODE],
        )?;
        conn.execute(
            "UPDATE project_index SET kind = 'reference' WHERE project_id = ?1 AND kind = ?2",
            params![project_id, KIND_HEALTH_REF],
        )?;

        let node_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM project_index WHERE project_id = ?1 AND kind = 'node'",
            params![project_id],
            |row| row.get(0),
        )?;
        let edge_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM project_index WHERE project_id = ?1 AND kind = 'reference'",
            params![project_id],
            |row| row.get(0),
        )?;
        let totals = serde_json::json!({
            "declared_node_count": node_count,
            "declared_edge_count": edge_count,
            "health_scan": true,
        })
        .to_string();
        conn.execute(
            "DELETE FROM project_index WHERE project_id = ?1 AND kind = 'meta' AND name = '__graph_totals__'",
            params![project_id],
        )?;
        conn.execute(
            r#"
            INSERT INTO project_index (
                id, project_id, repo_path, kind, name, file_path, line, target,
                ref_count, detail, payload_json, indexed_at
            ) VALUES (?1, ?2, ?3, 'meta', '__graph_totals__', NULL, NULL, NULL, ?4, ?5, ?6, ?7)
            "#,
            params![
                Uuid::new_v4().to_string(),
                project_id,
                repo_path,
                node_count,
                (edge_count as u64).to_string(),
                totals,
                indexed_at,
            ],
        )?;
        // Clear any prior partial marker on successful promote.
        conn.execute(
            "DELETE FROM project_index WHERE project_id = ?1 AND kind = 'meta' AND name = ?2",
            params![project_id, META_PARTIAL],
        )?;
        Ok(())
    })();
    match op {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

/// Prefer qualified `target` ids; fall back to name only when target is empty.
fn compute_dead_via_join(conn: &Connection, project_id: &str) -> Result<Vec<DeadSymbol>> {
    let mut dead = Vec::new();

    {
        let sql = r#"
            SELECT DISTINCT r.target, r.file_path, r.line, r.name
            FROM project_index r
            WHERE r.project_id = ?1
              AND r.kind = 'reference'
              AND r.target IS NOT NULL
              AND TRIM(r.target) != ''
              AND NOT EXISTS (
                SELECT 1 FROM project_index n
                WHERE n.project_id = r.project_id
                  AND n.kind = 'node'
                  AND (
                    (n.target IS NOT NULL AND TRIM(n.target) != '' AND n.target = r.target)
                    OR ((n.target IS NULL OR TRIM(n.target) = '') AND n.name = r.target)
                  )
              )
            ORDER BY r.target, r.file_path
        "#;
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![project_id], |row| {
            let target: String = row.get(0)?;
            let file: Option<String> = row.get(1)?;
            let line: Option<i64> = row.get(2)?;
            let from_id: String = row.get(3)?;
            Ok(DeadSymbol {
                name: target.clone(),
                kind: "broken".into(),
                file,
                line,
                detail: Some(format!("{from_id} → {target} hedefi yok")),
                project_id: Some(project_id.to_string()),
                ..DeadSymbol::default()
            })
        })?;
        for row in rows {
            dead.push(row?);
        }
    }

    {
        let sql = r#"
            SELECT n.name, n.file_path, n.line, COALESCE(n.target, n.name)
            FROM project_index n
            WHERE n.project_id = ?1
              AND n.kind = 'node'
              AND NOT EXISTS (
                SELECT 1 FROM project_index r
                WHERE r.project_id = n.project_id
                  AND r.kind = 'reference'
                  AND (
                    (n.target IS NOT NULL AND TRIM(n.target) != '' AND r.target = n.target)
                    OR r.target = n.name
                  )
              )
            ORDER BY n.name, n.file_path
        "#;
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(DeadSymbol {
                name: row.get(0)?,
                kind: "unused".into(),
                file: row.get(1)?,
                line: row.get(2)?,
                detail: Some("gelen referans yok".into()),
                project_id: Some(project_id.to_string()),
                ..DeadSymbol::default()
            })
        })?;
        for row in rows {
            dead.push(row?);
        }
    }

    Ok(dead)
}

fn replace_dead_rows_atomic(
    conn: &Connection,
    project_id: &str,
    repo_path: &str,
    indexed_at: &str,
    findings: &[DeadSymbol],
) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let op = (|| -> Result<()> {
        conn.execute(
            "DELETE FROM project_index WHERE project_id = ?1 AND kind IN ('dead', 'broken')",
            params![project_id],
        )?;
        for symbol in findings {
            let kind = if symbol.kind == "broken" {
                "broken"
            } else {
                "dead"
            };
            let payload = serde_json::json!({
                "severity": symbol.severity,
                "rule": symbol.rule,
            })
            .to_string();
            // Absolute paths stored; masking happens at output only.
            conn.execute(
                r#"
                INSERT INTO project_index (
                    id, project_id, repo_path, kind, name, file_path, line, target,
                    ref_count, detail, payload_json, indexed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 0, ?8, ?9, ?10)
                "#,
                params![
                    Uuid::new_v4().to_string(),
                    project_id,
                    repo_path,
                    kind,
                    symbol.name,
                    symbol.file.as_deref(),
                    symbol.line,
                    symbol.detail.as_deref(),
                    payload,
                    indexed_at,
                ],
            )?;
        }
        conn.execute(
            "DELETE FROM project_index WHERE project_id = ?1 AND kind = 'meta' AND name = ?2",
            params![project_id, META_HEALTH],
        )?;
        let meta = serde_json::json!({
            "partial": false,
            "finding_count": findings.len(),
        })
        .to_string();
        conn.execute(
            r#"
            INSERT INTO project_index (
                id, project_id, repo_path, kind, name, file_path, line, target,
                ref_count, detail, payload_json, indexed_at
            ) VALUES (?1, ?2, ?3, 'meta', ?4, NULL, NULL, NULL, ?5, NULL, ?6, ?7)
            "#,
            params![
                Uuid::new_v4().to_string(),
                project_id,
                repo_path,
                META_HEALTH,
                findings.len() as i64,
                meta,
                indexed_at,
            ],
        )?;
        Ok(())
    })();
    match op {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(err) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::GuardedCommand;
    use crate::services::memory_bridge::{health_page_timeout, MemoryBridge, MemoryBridgeConfig};
    use std::fs;
    use std::path::PathBuf;
    use std::time::Duration;

    /// Fast cross-platform stub: pre-rendered JSON pages, no per-call Python.
    /// Function: sym_00000..sym_N-1; Method: meth_00000..meth_M-1;
    /// CALLS: each sym_i → meth_i (matched) plus every 10th → ghost_XXXXX (broken).
    fn write_fast_paged_stub(
        dir: &std::path::Path,
        fn_count: usize,
        method_count: usize,
        page_size: usize,
    ) -> PathBuf {
        let pages = dir.join("pages");
        fs::create_dir_all(&pages).unwrap();

        // Pre-render pages keyed by exact skip offset so peek-after-short-page
        // (skip=end) returns empty instead of replaying the last page index.
        let mut skip = 0usize;
        while skip < fn_count {
            let end = (skip + page_size).min(fn_count);
            let rows: Vec<String> = (skip..end)
                .map(|i| {
                    format!(
                        "[\"sym_{i:05}\",\"sym_{i:05}\",\"{}/src/fn/sym_{i:05}.rs\",{line}]",
                        dir.display(),
                        line = i + 1
                    )
                })
                .collect();
            let body = format!(
                "{{\"columns\":[\"name\",\"id\",\"file\",\"line\"],\"rows\":[{}]}}",
                rows.join(",")
            );
            fs::write(pages.join(format!("fn_skip_{skip}.json")), body).unwrap();
            skip = end;
        }

        skip = 0;
        while skip < method_count {
            let end = (skip + page_size).min(method_count);
            let rows: Vec<String> = (skip..end)
                .map(|i| {
                    format!(
                        "[\"meth_{i:05}\",\"meth_{i:05}\",\"{}/src/meth/meth_{i:05}.rs\",{line}]",
                        dir.display(),
                        line = i + 1
                    )
                })
                .collect();
            let body = format!(
                "{{\"columns\":[\"name\",\"id\",\"file\",\"line\"],\"rows\":[{}]}}",
                rows.join(",")
            );
            fs::write(pages.join(format!("meth_skip_{skip}.json")), body).unwrap();
            skip = end;
        }

        // CALLS pages: matched edges + broken ghosts. Total edges = method_count + broken_count.
        // We emit one edge per method (sym→meth) and one ghost per 10 functions.
        let mut edges: Vec<(String, String, String, i64)> = Vec::new();
        for i in 0..method_count {
            edges.push((
                format!("sym_{i:05}"),
                format!("meth_{i:05}"),
                format!("{}/src/fn/sym_{i:05}.rs", dir.display()),
                (i + 1) as i64,
            ));
        }
        for i in (0..fn_count).step_by(10) {
            edges.push((
                format!("sym_{i:05}"),
                format!("ghost_{i:05}"),
                format!("{}/src/fn/sym_{i:05}.rs", dir.display()),
                (i + 1) as i64,
            ));
        }
        let edge_total = edges.len();
        skip = 0;
        while skip < edge_total {
            let end = (skip + page_size).min(edge_total);
            let rows: Vec<String> = edges[skip..end]
                .iter()
                .map(|(fr, to, file, line)| {
                    format!("[\"{fr}\",\"{to}\",\"{fr}\",\"{to}\",\"{file}\",{line}]")
                })
                .collect();
            let body = format!(
                "{{\"columns\":[\"caller\",\"callee\",\"from_id\",\"to_id\",\"file\",\"line\"],\"rows\":[{}]}}",
                rows.join(",")
            );
            fs::write(pages.join(format!("calls_skip_{skip}.json")), body).unwrap();
            skip = end;
        }
        fs::write(
            dir.join("counts.json"),
            format!("{{\"fn\":{fn_count},\"method\":{method_count},\"calls\":{edge_total}}}"),
        )
        .unwrap();

        // Fast dispatcher: cats pre-rendered pages by exact skip (no row generation).
        let dispatcher = dir.join("stub_dispatch.py");
        fs::write(
            &dispatcher,
            r#"import json, os, re, sys, time
args = " ".join(sys.argv[1:])
root = os.path.dirname(os.path.abspath(__file__))
pages = os.path.join(root, "pages")
with open(os.path.join(root, "counts.json"), encoding="utf-8") as f:
    counts = json.load(f)
if "count(f)" in args or "count(f)" in args.replace(" ", ""):
    n = counts["method"] if ":Method" in args else counts["fn"]
    print(json.dumps({"columns": ["c"], "rows": [[n]]})); raise SystemExit(0)
if "count(r)" in args:
    print(json.dumps({"columns": ["c"], "rows": [[counts["calls"]]]})); raise SystemExit(0)
if "index_repository" in args:
    print(json.dumps({"project": "health-t1", "nodes": 1, "edges": 0, "status": "indexed"})); raise SystemExit(0)
if "list_projects" in args:
    print(json.dumps({"projects": []})); raise SystemExit(0)
m = re.search(r"SKIP\s+(\d+)", args)
skip = int(m.group(1)) if m else 0
# Prefer CBM --offset when present.
m = re.search(r"--offset\s+(\d+)", args)
if m:
    skip = int(m.group(1))
delay = os.path.join(root, "page_delay_ms")
if skip > 0 and os.path.isfile(delay):
    try:
        ms = float(open(delay, encoding="utf-8").read().strip() or "0")
    except Exception:
        ms = 0
    if ms > 0:
        time.sleep(ms / 1000.0)
def emit(prefix, empty):
    path = os.path.join(pages, f"{prefix}_skip_{skip}.json")
    if os.path.isfile(path):
        sys.stdout.write(open(path, encoding="utf-8").read()); return
    print(empty)
if "CALLS" in args:
    emit("calls", json.dumps({"columns":["caller","callee","from_id","to_id","file","line"],"rows":[]})); raise SystemExit(0)
if ":Method" in args:
    emit("meth", json.dumps({"columns":["name","id","file","line"],"rows":[]})); raise SystemExit(0)
if ":Function" in args:
    emit("fn", json.dumps({"columns":["name","id","file","line"],"rows":[]})); raise SystemExit(0)
print({})
"#,
        )
        .unwrap();

        #[cfg(windows)]
        let script = dir.join("codebase-memory-mcp.cmd");
        #[cfg(not(windows))]
        let script = dir.join("codebase-memory-mcp");

        #[cfg(windows)]
        {
            fs::write(
                &script,
                format!(
                    "@echo off\r\npython \"{disp}\" %*\r\n",
                    disp = dispatcher.display()
                ),
            )
            .unwrap();
            return script;
        }
        #[cfg(not(windows))]
        {
            fs::write(
                &script,
                format!(
                    "#!/usr/bin/env bash\nexec python3 \"{disp}\" \"$@\"\n",
                    disp = dispatcher.display()
                ),
            )
            .unwrap();
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
            script
        }
    }

    fn seed_indexed_at(store: &ExperienceStore, project_id: &str, repo: &str, at: &str) {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            r#"
            INSERT INTO project_index (
                id, project_id, repo_path, kind, name, file_path, line, target,
                ref_count, detail, payload_json, indexed_at
            ) VALUES (?1, ?2, ?3, 'meta', '__seed__', NULL, NULL, NULL, 0, NULL, '{}', ?4)
            "#,
            params![Uuid::new_v4().to_string(), project_id, repo, at],
        )
        .unwrap();
    }

    #[test]
    fn health_page_timeout_scales_with_page_size() {
        let cfg = MemoryBridgeConfig::force_cli();
        let t2000 = health_page_timeout(&cfg, 2000);
        let t4000 = health_page_timeout(&cfg, 4000);
        assert!(t2000 > cfg.read_timeout);
        assert!(t4000 > t2000);
        // 2000 → +30s (2 chunks × 15)
        assert_eq!(t2000, cfg.read_timeout + Duration::from_secs(30));
    }

    #[tokio::test]
    async fn t1_paged_scan_methods_broken_indexed_at_and_boundaries() {
        let dir = std::env::temp_dir().join(format!("al-health-t1-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let fn_count = 5000usize;
        let method_count = 2500usize;
        let script =
            write_fast_paged_stub(&dir, fn_count, method_count, HEALTH_SCAN_PAGE_SIZE as usize);
        let bridge = MemoryBridge::from_binary(&script);
        let store = ExperienceStore::memory().unwrap();
        let repo = dir.to_string_lossy().into_owned();
        let project_id = {
            let conn = store.conn.lock().unwrap();
            register_path_project(&conn, "health-t1", &repo, &["health-t1"]).unwrap()
        };
        let expected_at = "2026-10-15T12:00:00+00:00";
        seed_indexed_at(&store, &project_id, &repo, expected_at);

        let cancel = Arc::new(AtomicBool::new(false));
        let result = run_health_scan(&bridge, &store, &repo, &project_id, cancel, None)
            .await
            .expect("scan");

        assert!(!result.partial, "reason={:?}", result.reason);
        assert_eq!(result.indexed_at, expected_at);
        assert_eq!(result.symbols_scanned, (fn_count + method_count) as u64);
        assert_eq!(result.expected_symbols, result.symbols_scanned);
        assert_eq!(result.edges_scanned, result.expected_edges);
        assert!(result.pages >= 4, "pages={}", result.pages);

        // Unique Function+Method rows, no boundary loss/dup.
        let (fn_names, meth_names): (Vec<_>, Vec<_>) = {
            let conn = store.conn.lock().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT name, detail FROM project_index WHERE project_id = ?1 AND kind = 'node' ORDER BY name",
                )
                .unwrap();
            let rows: Vec<(String, Option<String>)> = stmt
                .query_map(params![project_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .unwrap()
                .map(|r| r.unwrap())
                .collect();
            let fns: Vec<_> = rows
                .iter()
                .filter(|(_, k)| k.as_deref() == Some("function"))
                .map(|(n, _)| n.clone())
                .collect();
            let meths: Vec<_> = rows
                .iter()
                .filter(|(_, k)| k.as_deref() == Some("method"))
                .map(|(n, _)| n.clone())
                .collect();
            (fns, meths)
        };
        assert_eq!(fn_names.len(), fn_count);
        assert_eq!(meth_names.len(), method_count);
        let unique_fn: std::collections::BTreeSet<_> = fn_names.iter().cloned().collect();
        let unique_meth: std::collections::BTreeSet<_> = meth_names.iter().cloned().collect();
        assert_eq!(unique_fn.len(), fn_count);
        assert_eq!(unique_meth.len(), method_count);
        for i in [1999usize, 2000, 2001, 3999, 4000, 4001] {
            assert!(unique_fn.contains(&format!("sym_{i:05}")));
        }
        for i in [1999usize, 2000, 2001] {
            if i < method_count {
                assert!(unique_meth.contains(&format!("meth_{i:05}")));
            }
        }

        // Broken across full set (ghost_XXXXX every 10).
        let broken: Vec<String> = {
            let conn = store.conn.lock().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM project_index WHERE project_id = ?1 AND kind = 'broken' ORDER BY name",
                )
                .unwrap();
            stmt.query_map(params![project_id], |row| row.get(0))
                .unwrap()
                .map(|r| r.unwrap())
                .collect()
        };
        let expected_broken = (0..fn_count).step_by(10).count();
        assert_eq!(broken.len(), expected_broken);
        assert!(broken.contains(&"ghost_00000".to_string()));
        assert!(broken.contains(&"ghost_04990".to_string()));
        assert!(!result.kritik.is_empty());
        assert!(result
            .kritik
            .iter()
            .all(|s| s.severity.as_deref() == Some("Kritik")));
        assert!(result
            .kritik
            .iter()
            .all(|s| s.rule.as_deref() == Some("broken-missing-target")));

        // Absolute paths stored internally (not masked).
        let sample_file: String = {
            let conn = store.conn.lock().unwrap();
            conn.query_row(
                "SELECT file_path FROM project_index WHERE project_id = ?1 AND kind = 'node' LIMIT 1",
                params![project_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert!(
            PathBuf::from(&sample_file).is_absolute(),
            "expected absolute stored path, got {sample_file}"
        );
        assert!(!sample_file.starts_with("~/"));
        // Output is masked.
        assert!(
            result.repo_path.starts_with("~/")
                || !result
                    .repo_path
                    .contains(&std::env::var("HOME").unwrap_or_default())
        );
    }

    #[tokio::test]
    async fn t1_cancel_keeps_prior_snapshot_and_sets_partial_reason() {
        let dir = std::env::temp_dir().join(format!("al-health-t1c-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let script = write_fast_paged_stub(&dir, 5000, 100, HEALTH_SCAN_PAGE_SIZE as usize);
        fs::write(dir.join("page_delay_ms"), "1500").unwrap();
        let bridge = MemoryBridge::from_binary(&script);
        let store = ExperienceStore::memory().unwrap();
        let repo = dir.to_string_lossy().into_owned();
        let project_id = {
            let conn = store.conn.lock().unwrap();
            register_path_project(&conn, "health-t1c", &repo, &["health-t1c"]).unwrap()
        };
        let expected_at = "2026-10-15T12:34:56+00:00";
        seed_indexed_at(&store, &project_id, &repo, expected_at);

        // Seed a "good" live node that must survive cancel.
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                r#"
                INSERT INTO project_index (
                    id, project_id, repo_path, kind, name, file_path, line, target,
                    ref_count, detail, payload_json, indexed_at
                ) VALUES (?1, ?2, ?3, 'node', 'keep_me', 'src/keep.rs', 1, 'keep_me', 0, 'function', '{}', ?4)
                "#,
                params![Uuid::new_v4().to_string(), project_id, repo, expected_at],
            )
            .unwrap();
        }

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_flag = cancel.clone();
        let bridge2 = bridge.clone();
        let store2 = store.clone();
        let repo2 = repo.clone();
        let pid2 = project_id.clone();
        let handle = tokio::spawn(async move {
            run_health_scan(&bridge2, &store2, &repo2, &pid2, cancel_flag, None).await
        });
        tokio::time::sleep(Duration::from_millis(400)).await;
        cancel.store(true, Ordering::SeqCst);
        let result = handle.await.unwrap().expect("scan");
        assert!(result.partial);
        assert_eq!(result.reason.as_deref(), Some(REASON_CANCELLED));
        assert_eq!(result.indexed_at, expected_at);

        // Prior good snapshot intact.
        let keep: i64 = {
            let conn = store.conn.lock().unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM project_index WHERE project_id = ?1 AND kind = 'node' AND name = 'keep_me'",
                params![project_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(keep, 1);
        // No staging leftovers.
        let staging: i64 = {
            let conn = store.conn.lock().unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM project_index WHERE project_id = ?1 AND kind LIKE 'health_%'",
                params![project_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(staging, 0);
        // Partial flagged separately.
        let partial_meta: i64 = {
            let conn = store.conn.lock().unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM project_index WHERE project_id = ?1 AND kind = 'meta' AND name = ?2",
                params![project_id, META_PARTIAL],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(partial_meta, 1);
    }

    #[tokio::test]
    async fn progress_emitted_per_page() {
        let dir = std::env::temp_dir().join(format!("al-health-prog-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let script = write_fast_paged_stub(&dir, 2500, 100, HEALTH_SCAN_PAGE_SIZE as usize);
        let bridge = MemoryBridge::from_binary(&script);
        let store = ExperienceStore::memory().unwrap();
        let repo = dir.to_string_lossy().into_owned();
        let project_id = {
            let conn = store.conn.lock().unwrap();
            register_path_project(&conn, "health-prog", &repo, &["health-prog"]).unwrap()
        };
        seed_indexed_at(&store, &project_id, &repo, "2026-10-15T00:00:00+00:00");

        let pages_seen = Arc::new(std::sync::Mutex::new(Vec::<HealthScanProgress>::new()));
        let pages_cb = pages_seen.clone();
        let result = run_health_scan(
            &bridge,
            &store,
            &repo,
            &project_id,
            Arc::new(AtomicBool::new(false)),
            Some(Arc::new(move |p| {
                pages_cb.lock().unwrap().push(p);
            })),
        )
        .await
        .expect("scan");
        assert!(!result.partial);
        let events = pages_seen.lock().unwrap().clone();
        assert!(
            events.len() >= 3,
            "expected ≥1 Function + Method + CALLS page, got {}",
            events.len()
        );
        assert!(events
            .iter()
            .any(|e| e.label == "Function" && e.page_rows > 0));
        assert!(events.iter().any(|e| e.label == "CALLS"));
        assert!(events.iter().any(|e| e.page_index >= 1));
    }

    #[tokio::test]
    async fn count_mismatch_marks_partial_without_promote() {
        let dir = std::env::temp_dir().join(format!("al-health-mismatch-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        // Stub that lies about counts (says 5000) but only serves 100 symbols.
        let script = write_fast_paged_stub(&dir, 100, 0, HEALTH_SCAN_PAGE_SIZE as usize);
        // Overwrite counts to claim more than we serve.
        fs::write(
            dir.join("counts.json"),
            r#"{"fn":5000,"method":0,"calls":0}"#,
        )
        .unwrap();
        let bridge = MemoryBridge::from_binary(&script);
        let store = ExperienceStore::memory().unwrap();
        let repo = dir.to_string_lossy().into_owned();
        let project_id = {
            let conn = store.conn.lock().unwrap();
            register_path_project(&conn, "mismatch", &repo, &["mismatch"]).unwrap()
        };
        seed_indexed_at(&store, &project_id, &repo, "2026-01-01T00:00:00+00:00");

        let result = run_health_scan(
            &bridge,
            &store,
            &repo,
            &project_id,
            Arc::new(AtomicBool::new(false)),
            None,
        )
        .await
        .expect("scan");
        assert!(result.partial);
        assert_eq!(result.reason.as_deref(), Some(REASON_COUNT_MISMATCH));
        assert_eq!(result.expected_symbols, 5000);
        assert_eq!(result.symbols_scanned, 100);
        let live_nodes: i64 = {
            let conn = store.conn.lock().unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM project_index WHERE project_id = ?1 AND kind = 'node'",
                params![project_id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(live_nodes, 0, "mismatch must not promote staging");
    }

    /// Real CBM v0.11.0 SKIP/LIMIT evidence (fetches pinned release when absent).
    #[tokio::test]
    async fn real_cbm_v011_skip_limit_multi_page() {
        let cbm = ensure_real_cbm_binary();
        // Index a tiny synthetic crate with > page-size symbols is expensive;
        // instead verify query_graph honors SKIP/LIMIT by indexing a generated tree
        // with enough functions for 2 pages at a reduced page size... But HEALTH_SCAN_PAGE_SIZE
        // is fixed at 2000. Generating 2000+ real symbols is OK for CI if bounded.
        let dir = std::env::temp_dir().join(format!("al-cbm-real-{}", Uuid::new_v4()));
        fs::create_dir_all(dir.join("src")).unwrap();
        // 2100 trivial functions → at least 2 pages of 2000.
        let mut lib = String::from("#![allow(dead_code)]\n");
        for i in 0..2100 {
            lib.push_str(&format!("pub fn cbm_sym_{i:04}() {{}}\n"));
        }
        fs::write(dir.join("src/lib.rs"), lib).unwrap();
        fs::write(
            dir.join("Cargo.toml"),
            r#"[package]
name = "cbm_health_probe"
version = "0.1.0"
edition = "2021"
"#,
        )
        .unwrap();

        let bridge = MemoryBridge::from_binary(&cbm);
        // Index (mutating — long timeout).
        let graph = bridge
            .index_workspace(dir.to_string_lossy().into_owned())
            .await
            .expect("real CBM index");
        // CBM may prefix the project name; resolve via list_projects root_path.
        let project = bridge
            .resolve_cbm_project_name(&dir.to_string_lossy())
            .await
            .or_else(|| (!graph.project.is_empty()).then_some(graph.project.clone()))
            .unwrap_or_else(|| dir.file_name().unwrap().to_string_lossy().into_owned());
        let total = bridge
            .fetch_label_count(&project, "Function")
            .await
            .expect("count");
        assert!(
            total > HEALTH_SCAN_PAGE_SIZE as u64,
            "need >1 page of symbols from real CBM, got {total} (project={project})"
        );

        let page0 = bridge
            .fetch_ast_nodes_page(&project, "Function", "function", 0, HEALTH_SCAN_PAGE_SIZE)
            .await
            .expect("page0");
        let page1 = bridge
            .fetch_ast_nodes_page(
                &project,
                "Function",
                "function",
                HEALTH_SCAN_PAGE_SIZE as u64,
                HEALTH_SCAN_PAGE_SIZE,
            )
            .await
            .expect("page1");
        assert_eq!(page0.len(), HEALTH_SCAN_PAGE_SIZE as usize);
        assert!(!page1.is_empty());
        // No overlap at boundary.
        let ids0: std::collections::HashSet<_> = page0.iter().map(|n| n.id.clone()).collect();
        let ids1: std::collections::HashSet<_> = page1.iter().map(|n| n.id.clone()).collect();
        assert!(ids0.is_disjoint(&ids1), "SKIP/LIMIT pages overlap");
        assert_eq!(
            ids0.len() + ids1.len(),
            page0.len() + page1.len(),
            "duplicate ids inside a page"
        );
        // Advance by returned rows (token truncation can yield short pages mid-stream).
        let mut skip = 0u64;
        let mut scanned = 0u64;
        loop {
            let batch = bridge
                .fetch_ast_nodes_page(
                    &project,
                    "Function",
                    "function",
                    skip,
                    HEALTH_SCAN_PAGE_SIZE,
                )
                .await
                .unwrap();
            let n = batch.len() as u64;
            if n == 0 {
                break;
            }
            scanned += n;
            skip += n;
            if n < HEALTH_SCAN_PAGE_SIZE as u64 {
                let more = bridge
                    .fetch_ast_nodes_page(
                        &project,
                        "Function",
                        "function",
                        skip,
                        HEALTH_SCAN_PAGE_SIZE,
                    )
                    .await
                    .unwrap();
                if more.is_empty() {
                    break;
                }
                let n2 = more.len() as u64;
                scanned += n2;
                skip += n2;
                if n2 < HEALTH_SCAN_PAGE_SIZE as u64 {
                    break;
                }
            }
        }
        assert_eq!(scanned, total, "paged scan must equal count(f)");
    }

    fn ensure_real_cbm_binary() -> PathBuf {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let bridge_unix = manifest.join("../bridge/codebase-memory-mcp");
        let bridge_win = manifest.join("../bridge/codebase-memory-mcp.exe");
        let candidates = [
            bridge_unix.clone(),
            bridge_win.clone(),
            PathBuf::from("/workspace/bridge/codebase-memory-mcp"),
        ];
        for c in &candidates {
            if c.is_file() {
                let meta = fs::metadata(c).unwrap();
                // Compile stubs are tiny (<4KB); real v0.11.0 is multi-MB.
                if meta.len() > 4096 {
                    return c.clone();
                }
            }
        }
        // Fetch pinned v0.11.0 via project script (PolicyGate via GuardedCommand).
        let script = manifest.join("../scripts/ci/fetch-codebase-memory-mcp.sh");
        assert!(
            script.is_file(),
            "fetch script missing at {}",
            script.display()
        );
        let status = GuardedCommand::new("bash")
            .arg(&script)
            .status()
            .expect("spawn fetch-codebase-memory-mcp.sh via GuardedCommand");
        assert!(status.success(), "fetch real CBM v0.11.0 failed");
        for c in &[bridge_unix, bridge_win] {
            if c.is_file() && fs::metadata(c).unwrap().len() > 4096 {
                return c.clone();
            }
        }
        panic!("real CBM binary missing after fetch");
    }

    #[test]
    fn join_broken_and_unused_prefers_qualified_ids() {
        let store = ExperienceStore::memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let pid = "proj-join";
        let now = now_rfc3339();
        // Manually insert live nodes/refs (post-promote shape).
        for (id, name, file) in [
            ("foo::foo", "foo", "src/a.rs"),
            ("bar::bar", "bar", "src/b.rs"),
        ] {
            conn.execute(
                r#"INSERT INTO project_index (
                    id, project_id, repo_path, kind, name, file_path, line, target,
                    ref_count, detail, payload_json, indexed_at
                ) VALUES (?1, ?2, '/tmp/proj', 'node', ?3, ?4, 1, ?5, 0, 'function', '{}', ?6)"#,
                params![Uuid::new_v4().to_string(), pid, name, file, id, now],
            )
            .unwrap();
        }
        conn.execute(
            r#"INSERT INTO project_index (
                id, project_id, repo_path, kind, name, file_path, line, target,
                ref_count, detail, payload_json, indexed_at
            ) VALUES (?1, ?2, '/tmp/proj', 'reference', 'foo::foo', 'src/a.rs', 3, 'ghost::x', 1, NULL, '{}', ?3)"#,
            params![Uuid::new_v4().to_string(), pid, now],
        )
        .unwrap();
        let dead = compute_dead_via_join(&conn, pid).unwrap();
        assert!(dead
            .iter()
            .any(|d| d.kind == "broken" && d.name == "ghost::x"));
        assert!(dead.iter().any(|d| d.kind == "unused" && d.name == "foo"));
        assert!(dead.iter().any(|d| d.kind == "unused" && d.name == "bar"));
    }
}
