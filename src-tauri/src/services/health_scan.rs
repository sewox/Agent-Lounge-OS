//! Health-1a: full paged dead-symbol scan (concurrency 1).
//!
//! After each indexing job (and via manual trigger) pages Function+Method symbols
//! (~2000/page), stores nodes/edges, computes broken via SQLite JOIN, applies H2
//! filters, and writes Kritik/Potansiyel findings. Cancel → `partial=true`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, Notify, Semaphore};
use uuid::Uuid;

use super::health_filters::{mask_home_path, split_findings, HealthFilterContext};
use super::memory_bridge::{MemoryBridge, HEALTH_SCAN_PAGE_SIZE};
use crate::db::{canonicalize_repo_path, prefer_canonical, register_path_project, ExperienceStore};
use crate::models::{now_rfc3339, AstNode, CodeReference, DeadSymbol, HealthScanResult};

pub const HEALTH_SCAN_EVENT: &str = "lounge://health-scan";
pub const HEALTH_SCAN_CONCURRENCY: usize = 1;

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

/// Install the process-wide queue (called once from app setup).
pub async fn install_health_scan_queue(queue: Arc<HealthScanQueue>) {
    let mut guard = global_queue().lock().await;
    *guard = Some(queue);
}

async fn global_health_queue() -> Option<Arc<HealthScanQueue>> {
    global_queue().lock().await.clone()
}

/// Enqueue after indexing when a global queue is installed (no-op in unit tests).
pub async fn enqueue_after_index(repo_path: &str, project_hint: &str) {
    if let Some(q) = global_health_queue().await {
        let _ = q.enqueue(repo_path, project_hint).await;
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

            let queue = Arc::clone(&self);
            let app_handle = app.clone();
            // Concurrency 1: run inline under the permit (no nested spawn pile-up).
            queue.run_one(&app_handle, &job_id).await;
            drop(permit);
        }
    }

    pub async fn enqueue(&self, repo_path: &str, project_hint: &str) -> Result<HealthScanJob> {
        let repo_path = repo_path.trim();
        if repo_path.is_empty() {
            bail!("repo_path boş");
        }
        let canonical_path = canonicalize_repo_path(repo_path);
        let masked_path = mask_home_path(&canonical_path);

        // Dedup active jobs for the same repo.
        {
            let guard = self.inner.lock().await;
            if let Some(existing) = guard.jobs.iter().find(|j| {
                j.status.is_active()
                    && (j.repo_path == canonical_path || j.repo_path == masked_path)
            }) {
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
            // Retain last 100 jobs.
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
                job.result = Some(HealthScanResult {
                    project_id: job.project_id.clone(),
                    repo_path: mask_home_path(&job.repo_path),
                    indexed_at: now_rfc3339(),
                    partial: true,
                    page_size: HEALTH_SCAN_PAGE_SIZE as u64,
                    ..HealthScanResult::default()
                });
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

        if cancel.load(Ordering::SeqCst) {
            self.finish_cancelled(app, job_id, &project_id, &repo_path, None)
                .await;
            return;
        }

        let scan = run_health_scan(
            &self.bridge,
            &self.store,
            &repo_path,
            &project_id,
            cancel.clone(),
        )
        .await;

        if cancel.load(Ordering::SeqCst) {
            let partial = scan.ok();
            self.finish_cancelled(app, job_id, &project_id, &repo_path, partial)
                .await;
            return;
        }

        match scan {
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
            job.status = if result.partial {
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

    async fn finish_cancelled(
        &self,
        app: &AppHandle,
        job_id: &str,
        project_id: &str,
        repo_path: &str,
        partial: Option<HealthScanResult>,
    ) {
        let mut guard = self.inner.lock().await;
        if let Some(job) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
            job.status = HealthScanPhase::Cancelled;
            job.updated_at = now_rfc3339();
            job.result = Some(partial.unwrap_or(HealthScanResult {
                project_id: project_id.to_string(),
                repo_path: mask_home_path(repo_path),
                indexed_at: now_rfc3339(),
                partial: true,
                page_size: HEALTH_SCAN_PAGE_SIZE as u64,
                ..HealthScanResult::default()
            }));
            if let Some(r) = job.result.as_mut() {
                r.partial = true;
            }
            emit_job(app, job.clone());
        }
    }
}

fn emit_job(app: &AppHandle, job: HealthScanJob) {
    let _ = app.emit(HEALTH_SCAN_EVENT, HealthScanEvent { job });
}

async fn resolve_project_id(
    store: &ExperienceStore,
    repo_path: &str,
    project_hint: &str,
) -> Result<String> {
    let hint = project_hint.trim();
    let path = repo_path.to_string();
    let hint_owned = hint.to_string();
    let conn = store.conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        let _ = crate::db::migrate_project_identity(&conn);
        if !path.trim().is_empty() {
            if let Ok(id) = register_path_project(
                &conn,
                if hint_owned.is_empty() {
                    "unknown"
                } else {
                    hint_owned.as_str()
                },
                &path,
                &[hint_owned.as_str()],
            ) {
                return Ok(id);
            }
        }
        if !hint_owned.is_empty() {
            return prefer_canonical(&conn, &hint_owned);
        }
        Ok(canonicalize_repo_path(&path))
    })
    .await
    .context("resolve project_id join")?
}

/// Core scan used by the queue and by unit tests (no Tauri).
pub async fn run_health_scan(
    bridge: &MemoryBridge,
    store: &ExperienceStore,
    repo_path: &str,
    project_id: &str,
    cancel: Arc<AtomicBool>,
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

    let indexed_at = read_or_fresh_indexed_at(store, project_id).await?;
    let page_size = HEALTH_SCAN_PAGE_SIZE;

    let mut nodes: Vec<AstNode> = Vec::new();
    let mut pages = 0u64;
    let mut partial = false;

    // Page Function then Method — stable ORDER BY for boundary correctness.
    for label in ["Function", "Method"] {
        let kind = if label == "Function" {
            "function"
        } else {
            "method"
        };
        let mut skip = 0u64;
        loop {
            if cancel.load(Ordering::SeqCst) {
                partial = true;
                break;
            }
            let batch = bridge
                .fetch_ast_nodes_page(&cbm_project, label, kind, skip, page_size)
                .await?;
            let n = batch.len() as u64;
            pages = pages.saturating_add(1);
            nodes.extend(batch);
            if n < page_size as u64 {
                break;
            }
            skip = skip.saturating_add(page_size as u64);
        }
        if partial {
            break;
        }
    }

    let mut edges: Vec<CodeReference> = Vec::new();
    if !partial {
        let mut skip = 0u64;
        loop {
            if cancel.load(Ordering::SeqCst) {
                partial = true;
                break;
            }
            let batch = bridge
                .fetch_call_edges_page(&cbm_project, skip, page_size)
                .await?;
            let n = batch.len() as u64;
            pages = pages.saturating_add(1);
            edges.extend(batch);
            if n < page_size as u64 {
                break;
            }
            skip = skip.saturating_add(page_size as u64);
        }
    }

    // Persist nodes/edges then JOIN for broken/unused.
    let project_key = project_id.to_string();
    let repo_for_db = repo_path.clone();
    let indexed_at_db = indexed_at.clone();
    let nodes_db = nodes.clone();
    let edges_db = edges.clone();
    let conn = store.conn.clone();
    let candidates = tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        replace_graph_rows(
            &conn,
            &project_key,
            &repo_for_db,
            &indexed_at_db,
            &nodes_db,
            &edges_db,
        )?;
        compute_dead_via_join(&conn, &project_key)
    })
    .await
    .context("health scan sqlite join")??;

    let filter_ctx = HealthFilterContext::for_repo(&repo_path);
    let (kritik, potansiyel) = split_findings(&filter_ctx, candidates);

    // Write filtered findings (replace prior dead/broken).
    let project_key = project_id.to_string();
    let repo_for_db = repo_path.clone();
    let indexed_at_db = indexed_at.clone();
    let all_findings: Vec<DeadSymbol> = kritik.iter().chain(potansiyel.iter()).cloned().collect();
    let findings_db = all_findings.clone();
    let conn = store.conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        replace_dead_rows(
            &conn,
            &project_key,
            &repo_for_db,
            &indexed_at_db,
            &findings_db,
            partial,
        )
    })
    .await
    .context("health scan save dead")??;

    Ok(HealthScanResult {
        project_id: project_id.to_string(),
        repo_path: mask_home_path(&repo_path),
        indexed_at,
        symbols_scanned: nodes.len() as u64,
        edges_scanned: edges.len() as u64,
        pages,
        page_size: page_size as u64,
        partial,
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

async fn read_or_fresh_indexed_at(store: &ExperienceStore, project_id: &str) -> Result<String> {
    let project_id = project_id.to_string();
    let conn = store.conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        let existing: Option<String> = conn
            .query_row(
                r#"
                SELECT indexed_at FROM project_index
                WHERE project_id = ?1
                ORDER BY indexed_at DESC
                LIMIT 1
                "#,
                params![project_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(existing.unwrap_or_else(now_rfc3339))
    })
    .await
    .context("indexed_at join")?
}

fn replace_graph_rows(
    conn: &Connection,
    project_id: &str,
    repo_path: &str,
    indexed_at: &str,
    nodes: &[AstNode],
    edges: &[CodeReference],
) -> Result<()> {
    conn.execute(
        "DELETE FROM project_index WHERE project_id = ?1 AND kind IN ('node', 'reference')",
        params![project_id],
    )?;
    for node in nodes {
        let name = if node.name.is_empty() {
            node.id.as_str()
        } else {
            node.name.as_str()
        };
        conn.execute(
            r#"
            INSERT INTO project_index (
                id, project_id, repo_path, kind, name, file_path, line, target,
                ref_count, detail, payload_json, indexed_at
            ) VALUES (?1, ?2, ?3, 'node', ?4, ?5, ?6, ?7, ?8, ?9, '{}', ?10)
            "#,
            params![
                Uuid::new_v4().to_string(),
                project_id,
                repo_path,
                name,
                node.file.as_deref().map(mask_home_path),
                node.line,
                Some(node.id.as_str()).filter(|id| !id.is_empty()),
                node.ref_count as i64,
                Some(node.kind.as_str()).filter(|k| !k.is_empty()),
                indexed_at,
            ],
        )?;
    }
    for edge in edges {
        conn.execute(
            r#"
            INSERT INTO project_index (
                id, project_id, repo_path, kind, name, file_path, line, target,
                ref_count, detail, payload_json, indexed_at
            ) VALUES (?1, ?2, ?3, 'reference', ?4, ?5, ?6, ?7, 1, NULL, '{}', ?8)
            "#,
            params![
                Uuid::new_v4().to_string(),
                project_id,
                repo_path,
                edge.from_id,
                edge.file.as_deref().map(mask_home_path),
                edge.line,
                edge.to_id,
                indexed_at,
            ],
        )?;
    }
    // Refresh declared totals to full scanned counts.
    let totals = serde_json::json!({
        "declared_node_count": nodes.len() as u64,
        "declared_edge_count": edges.len() as u64,
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
            nodes.len() as i64,
            (edges.len() as u64).to_string(),
            totals,
            indexed_at,
        ],
    )?;
    Ok(())
}

/// Full-set broken + unused via SQLite JOIN (not LIMIT-shaped in-memory derive).
fn compute_dead_via_join(conn: &Connection, project_id: &str) -> Result<Vec<DeadSymbol>> {
    let mut dead = Vec::new();

    // Broken: reference targets with no matching node id/name.
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
                    (n.target IS NOT NULL AND n.target = r.target)
                    OR n.name = r.target
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
                last_ref: None,
                severity: None,
                rule: None,
            })
        })?;
        for row in rows {
            dead.push(row?);
        }
    }

    // Unused: nodes with zero inbound references.
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
                    r.target = n.name
                    OR (n.target IS NOT NULL AND r.target = n.target)
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
                last_ref: None,
                severity: None,
                rule: None,
            })
        })?;
        for row in rows {
            dead.push(row?);
        }
    }

    Ok(dead)
}

fn replace_dead_rows(
    conn: &Connection,
    project_id: &str,
    repo_path: &str,
    indexed_at: &str,
    findings: &[DeadSymbol],
    partial: bool,
) -> Result<()> {
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
            "partial_scan": partial,
        })
        .to_string();
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
                symbol.file.as_deref().map(mask_home_path),
                symbol.line,
                symbol.detail.as_deref().map(mask_home_path),
                payload,
                indexed_at,
            ],
        )?;
    }
    // Meta flag for partial scans.
    conn.execute(
        "DELETE FROM project_index WHERE project_id = ?1 AND kind = 'meta' AND name = '__health_scan__'",
        params![project_id],
    )?;
    let meta = serde_json::json!({
        "partial": partial,
        "finding_count": findings.len(),
    })
    .to_string();
    conn.execute(
        r#"
        INSERT INTO project_index (
            id, project_id, repo_path, kind, name, file_path, line, target,
            ref_count, detail, payload_json, indexed_at
        ) VALUES (?1, ?2, ?3, 'meta', '__health_scan__', NULL, NULL, NULL, ?4, NULL, ?5, ?6)
        "#,
        params![
            Uuid::new_v4().to_string(),
            project_id,
            repo_path,
            findings.len() as i64,
            meta,
            indexed_at,
        ],
    )?;
    Ok(())
}

/// Poll cancel flag without busy-spinning.
#[allow(dead_code)]
async fn wait_cancel(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::memory_bridge::MemoryBridge;
    use std::fs;
    use std::path::PathBuf;

    fn write_paged_cbm_stub(dir: &std::path::Path, total_symbols: usize) -> PathBuf {
        // Shared Python pager: Function pages only; Method empty; optional sibling delay file.
        let pager = dir.join("page_symbols.py");
        fs::write(
            &pager,
            format!(
                r#"import json, os, re, sys, time
args = " ".join(sys.argv[1:])
total = {total}
here = os.path.dirname(os.path.abspath(__file__))
if "CALLS" in args:
    print(json.dumps({{"columns":["caller","callee","from_id","to_id","file","line"],"rows":[]}}))
    raise SystemExit(0)
if ":Method" in args or "(f:Method)" in args:
    print(json.dumps({{"columns":["name","id","file","line"],"rows":[]}}))
    raise SystemExit(0)
m = re.search(r"SKIP\s+(\d+)", args); skip = int(m.group(1)) if m else 0
m = re.search(r"LIMIT\s+(\d+)", args); limit = int(m.group(1)) if m else 2000
delay_path = os.path.join(here, "page_delay_ms")
if skip > 0 and os.path.isfile(delay_path):
    try:
        ms = float(open(delay_path, encoding="utf-8").read().strip() or "0")
    except Exception:
        ms = 0
    if ms > 0:
        time.sleep(ms / 1000.0)
rows = []
end = min(total, skip + limit)
for i in range(skip, end):
    name = f"sym_{{i:05d}}"
    rows.append([name, name, f"src/gen/{{name}}.rs", i + 1])
print(json.dumps({{"columns":["name","id","file","line"],"rows":rows}}))
"#,
                total = total_symbols
            ),
        )
        .unwrap();

        #[cfg(windows)]
        let script = dir.join("codebase-memory-mcp.cmd");
        #[cfg(not(windows))]
        let script = dir.join("codebase-memory-mcp");

        #[cfg(windows)]
        {
            let cmd = format!(
                r#"@echo off
setlocal
set "ARGS=%*"
echo %ARGS%| findstr /C:"count(f)" >nul && (echo {{"columns":["c"],"rows":[[{total}]]}} & exit /b 0)
echo %ARGS%| findstr /C:"count(r)" >nul && (echo {{"columns":["c"],"rows":[[0]]}} & exit /b 0)
echo %ARGS%| findstr /I "index_repository" >nul && (echo {{"project":"health-t1","nodes":{total},"edges":0,"files":1,"status":"indexed"}} & exit /b 0)
echo %ARGS%| findstr /I "list_projects" >nul && (echo {{"projects":[]}} & exit /b 0)
python "{pager}" %*
"#,
                total = total_symbols,
                pager = pager.display()
            );
            fs::write(&script, cmd).unwrap();
            return script;
        }
        #[cfg(not(windows))]
        {
            let body = format!(
                r#"#!/usr/bin/env bash
set -euo pipefail
args="$*"
if printf '%s' "$args" | grep -q 'count(f)'; then
  echo '{{"columns":["c"],"rows":[[{total}]]}}'; exit 0
fi
if printf '%s' "$args" | grep -q 'count(r)'; then
  echo '{{"columns":["c"],"rows":[[0]]}}'; exit 0
fi
if printf '%s' "$args" | grep -q index_repository; then
  echo '{{"project":"health-t1","nodes":{total},"edges":0,"files":1,"status":"indexed"}}'; exit 0
fi
if printf '%s' "$args" | grep -q list_projects; then
  echo '{{"projects":[]}}'; exit 0
fi
exec python3 "{pager}" "$@"
"#,
                total = total_symbols,
                pager = pager.display()
            );
            fs::write(&script, body).unwrap();
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
            script
        }
    }

    #[tokio::test]
    async fn t1_paged_scan_no_lost_or_duplicate_at_boundaries() {
        let dir = std::env::temp_dir().join(format!("al-health-t1-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let total = 5000usize;
        let script = write_paged_cbm_stub(&dir, total);
        let bridge = MemoryBridge::from_binary(&script);
        let store = ExperienceStore::memory().unwrap();

        // Register canonical project id.
        let repo = dir.to_string_lossy().into_owned();
        let project_id = {
            let conn = store.conn.lock().unwrap();
            register_path_project(&conn, "health-t1", &repo, &["health-t1"]).unwrap()
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let result = run_health_scan(&bridge, &store, &repo, &project_id, cancel)
            .await
            .expect("scan");

        assert!(!result.partial, "full scan must not be partial");
        assert_eq!(result.symbols_scanned, total as u64);
        assert!(
            result.pages >= 3,
            "expected multiple pages, got {}",
            result.pages
        );
        assert_eq!(result.project_id, project_id);
        assert_eq!(result.page_size, HEALTH_SCAN_PAGE_SIZE as u64);
        assert!(!result.indexed_at.is_empty());

        // SQLite must hold exactly `total` unique node names — no boundary dup/loss.
        let names: Vec<String> = {
            let conn = store.conn.lock().unwrap();
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM project_index WHERE project_id = ?1 AND kind = 'node' ORDER BY name",
                )
                .unwrap();
            let rows = stmt
                .query_map(params![project_id], |row| row.get::<_, String>(0))
                .unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert_eq!(names.len(), total, "lost/extra rows across page boundaries");
        let unique: std::collections::BTreeSet<_> = names.iter().cloned().collect();
        assert_eq!(unique.len(), total, "duplicate rows across page boundaries");
        assert_eq!(names.first().map(String::as_str), Some("sym_00000"));
        assert_eq!(names.last().map(String::as_str), Some("sym_04999"));

        // Page boundary symbols present (1999, 2000, 2001, 3999, 4000, 4001).
        for i in [1999usize, 2000, 2001, 3999, 4000, 4001] {
            let expect = format!("sym_{i:05}");
            assert!(unique.contains(&expect), "missing boundary symbol {expect}");
        }
    }

    #[tokio::test]
    async fn t1_cancel_yields_partial_true() {
        let dir = std::env::temp_dir().join(format!("al-health-t1c-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let script = write_paged_cbm_stub(&dir, 5000);
        // Slow subsequent pages so cancel lands between page 1 and 2 (cross-platform).
        fs::write(dir.join("page_delay_ms"), "2000").unwrap();

        let bridge = MemoryBridge::from_binary(&script);
        let store = ExperienceStore::memory().unwrap();
        let repo = dir.to_string_lossy().into_owned();
        let project_id = {
            let conn = store.conn.lock().unwrap();
            register_path_project(&conn, "health-t1c", &repo, &["health-t1c"]).unwrap()
        };

        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_flag = cancel.clone();
        let bridge2 = bridge.clone();
        let store2 = store.clone();
        let repo2 = repo.clone();
        let pid2 = project_id.clone();

        let handle = tokio::spawn(async move {
            run_health_scan(&bridge2, &store2, &repo2, &pid2, cancel_flag).await
        });

        // Cancel after the first page has likely completed; page 2 sleeps via env delay.
        tokio::time::sleep(Duration::from_millis(400)).await;
        cancel.store(true, Ordering::SeqCst);

        let result = handle.await.unwrap().expect("scan result");
        assert!(result.partial, "cancel must mark partial=true");
        assert!(
            result.symbols_scanned < 5000,
            "cancelled scan should not finish all symbols (got {})",
            result.symbols_scanned
        );
    }

    #[test]
    fn join_broken_and_unused() {
        let store = ExperienceStore::memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let pid = "proj-join";
        let now = now_rfc3339();
        replace_graph_rows(
            &conn,
            pid,
            "/tmp/proj",
            &now,
            &[
                AstNode {
                    id: "foo".into(),
                    name: "foo".into(),
                    kind: "function".into(),
                    file: Some("src/a.rs".into()),
                    line: Some(1),
                    ref_count: 0,
                },
                AstNode {
                    id: "bar".into(),
                    name: "bar".into(),
                    kind: "function".into(),
                    file: Some("src/b.rs".into()),
                    line: Some(2),
                    ref_count: 0,
                },
            ],
            &[CodeReference {
                from_id: "foo".into(),
                to_id: "ghost".into(),
                file: Some("src/a.rs".into()),
                line: Some(3),
            }],
        )
        .unwrap();
        let dead = compute_dead_via_join(&conn, pid).unwrap();
        let broken: Vec<_> = dead.iter().filter(|d| d.kind == "broken").collect();
        let unused: Vec<_> = dead.iter().filter(|d| d.kind == "unused").collect();
        assert_eq!(broken.len(), 1);
        assert_eq!(broken[0].name, "ghost");
        let unused_names: std::collections::BTreeSet<_> =
            unused.iter().map(|d| d.name.as_str()).collect();
        assert!(unused_names.contains("foo"));
        assert!(unused_names.contains("bar"));
    }
}
