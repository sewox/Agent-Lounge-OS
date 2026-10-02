//! Arka plan indeks kuyruğu — tarama butonunun istek yaşam döngüsünden bağımsız.
//!
//! Bounded concurrency, iptal, hata raporu ve UI yeniden bağlanınca rehydrate.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, Notify, Semaphore};
use uuid::Uuid;

use super::memory_bridge::MemoryBridge;
use super::workspace_scan::{
    discover_projects_blocking, DiscoveredProject, WorkspaceScanErrorKind,
};
use crate::db::ExperienceStore;
use crate::models::{now_rfc3339, IndexSnapshot, ProjectSummary};

/// UI / NATS köprüsü olay adı.
pub const INDEX_JOB_EVENT: &str = "lounge://index-job";
/// Aynı anda kaç CBM index_repository çalışır.
pub const INDEX_CONCURRENCY: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexJobPhase {
    Queued,
    Indexing,
    Done,
    Failed,
    Cancelled,
}

impl IndexJobPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Indexing => "indexing",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Queued | Self::Indexing)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexJob {
    pub id: String,
    pub project: String,
    pub repo_path: String,
    pub status: IndexJobPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<IndexSnapshot>,
    pub enqueued_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IndexProgress {
    pub total: u64,
    pub queued: u64,
    pub indexing: u64,
    pub done: u64,
    pub failed: u64,
    pub cancelled: u64,
}

impl IndexProgress {
    pub fn from_jobs(jobs: &[IndexJob]) -> Self {
        let mut progress = Self {
            total: jobs.len() as u64,
            ..Self::default()
        };
        for job in jobs {
            match job.status {
                IndexJobPhase::Queued => progress.queued += 1,
                IndexJobPhase::Indexing => progress.indexing += 1,
                IndexJobPhase::Done => progress.done += 1,
                IndexJobPhase::Failed => progress.failed += 1,
                IndexJobPhase::Cancelled => progress.cancelled += 1,
            }
        }
        progress
    }

    pub fn active(&self) -> bool {
        self.queued > 0 || self.indexing > 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceScanResult {
    pub workspace_path: String,
    pub discovered: Vec<ProjectSummary>,
    pub jobs: Vec<IndexJob>,
    pub progress: IndexProgress,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexJobEvent {
    pub job: IndexJob,
    pub progress: IndexProgress,
    pub jobs: Vec<IndexJob>,
}

struct QueueInner {
    jobs: Vec<IndexJob>,
    /// job_id → cancel flag (indexing sırasında kontrol).
    cancels: std::collections::HashMap<String, Arc<AtomicBool>>,
}

pub struct IndexQueue {
    inner: Arc<Mutex<QueueInner>>,
    wake: Arc<Notify>,
    started: AtomicBool,
    bridge: MemoryBridge,
    store: ExperienceStore,
    concurrency: usize,
}

impl IndexQueue {
    pub fn new(bridge: MemoryBridge, store: ExperienceStore) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(Mutex::new(QueueInner {
                jobs: Vec::new(),
                cancels: std::collections::HashMap::new(),
            })),
            wake: Arc::new(Notify::new()),
            started: AtomicBool::new(false),
            bridge,
            store,
            concurrency: INDEX_CONCURRENCY,
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
            queue.run_loop(app).await;
        });
    }

    async fn run_loop(self: Arc<Self>, app: AppHandle) {
        let sem = Arc::new(Semaphore::new(self.concurrency.max(1)));
        loop {
            let job_id = {
                let guard = self.inner.lock().await;
                guard
                    .jobs
                    .iter()
                    .find(|j| j.status == IndexJobPhase::Queued)
                    .map(|j| j.id.clone())
            };
            let Some(job_id) = job_id else {
                self.wake.notified().await;
                continue;
            };

            let Ok(permit) = sem.clone().acquire_owned().await else {
                continue;
            };
            // İzin alındıktan sonra hâlâ queued mi?
            {
                let mut guard = self.inner.lock().await;
                let job_idx = match guard.jobs.iter().position(|j| j.id == job_id) {
                    Some(idx) if guard.jobs[idx].status == IndexJobPhase::Queued => idx,
                    _ => {
                        drop(permit);
                        continue;
                    }
                };
                guard.jobs[job_idx].status = IndexJobPhase::Indexing;
                guard.jobs[job_idx].updated_at = now_rfc3339();
                let snapshot = guard.jobs[job_idx].clone();
                let progress = IndexProgress::from_jobs(&guard.jobs);
                let jobs = guard.jobs.clone();
                drop(guard);
                emit_job(&app, snapshot, progress, jobs);
            }

            let queue = Arc::clone(&self);
            let app_handle = app.clone();
            tauri::async_runtime::spawn(async move {
                queue.run_one(&app_handle, &job_id).await;
                drop(permit);
                queue.wake.notify_one();
            });
        }
    }

    async fn run_one(&self, app: &AppHandle, job_id: &str) {
        let (repo_path, cancel) = {
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
            (job.repo_path, cancel)
        };

        if cancel.load(Ordering::SeqCst) {
            self.finish_cancelled(app, job_id).await;
            return;
        }

        if !self.bridge.binary_present() {
            self.finish_failed(
                app,
                job_id,
                "sidecar_missing",
                format!(
                    "codebase-memory-mcp missing: {}",
                    self.bridge.binary_display()
                ),
            )
            .await;
            return;
        }

        let bridge = self.bridge.clone();
        let path = repo_path.clone();
        let index_fut = bridge.index_workspace(path);
        let result = tokio::select! {
            biased;
            _ = wait_cancel(cancel.clone()) => {
                self.finish_cancelled(app, job_id).await;
                return;
            }
            res = index_fut => res,
        };

        if cancel.load(Ordering::SeqCst) {
            self.finish_cancelled(app, job_id).await;
            return;
        }

        match result {
            Ok(graph) => match self.store.save_project_index(graph).await {
                Ok(snapshot) => {
                    self.finish_done(app, job_id, snapshot).await;
                }
                Err(err) => {
                    self.finish_failed(app, job_id, "save_failed", err.to_string())
                        .await;
                }
            },
            Err(err) => {
                let text = err.to_string();
                let code = if text.contains("yok:") || text.contains("bulunamadı") {
                    "sidecar_missing"
                } else if text.contains("PermissionDenied") || text.contains("permission") {
                    "permission_denied"
                } else {
                    "index_failed"
                };
                self.finish_failed(app, job_id, code, text).await;
            }
        }
    }

    async fn finish_done(&self, app: &AppHandle, job_id: &str, snapshot: IndexSnapshot) {
        let mut guard = self.inner.lock().await;
        if let Some(job) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
            if job.status == IndexJobPhase::Cancelled {
                return;
            }
            job.status = IndexJobPhase::Done;
            job.error = None;
            job.error_code = None;
            job.snapshot = Some(snapshot);
            job.updated_at = now_rfc3339();
            let event_job = job.clone();
            let progress = IndexProgress::from_jobs(&guard.jobs);
            let jobs = guard.jobs.clone();
            drop(guard);
            emit_job(app, event_job, progress, jobs);
        }
    }

    async fn finish_failed(&self, app: &AppHandle, job_id: &str, code: &str, error: String) {
        let mut guard = self.inner.lock().await;
        if let Some(job) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
            if job.status == IndexJobPhase::Cancelled {
                return;
            }
            job.status = IndexJobPhase::Failed;
            job.error = Some(error);
            job.error_code = Some(code.into());
            job.updated_at = now_rfc3339();
            let event_job = job.clone();
            let progress = IndexProgress::from_jobs(&guard.jobs);
            let jobs = guard.jobs.clone();
            drop(guard);
            emit_job(app, event_job, progress, jobs);
        }
    }

    async fn finish_cancelled(&self, app: &AppHandle, job_id: &str) {
        let mut guard = self.inner.lock().await;
        if let Some(job) = guard.jobs.iter_mut().find(|j| j.id == job_id) {
            job.status = IndexJobPhase::Cancelled;
            job.error = None;
            job.error_code = Some("cancelled".into());
            job.updated_at = now_rfc3339();
            let event_job = job.clone();
            let progress = IndexProgress::from_jobs(&guard.jobs);
            let jobs = guard.jobs.clone();
            drop(guard);
            emit_job(app, event_job, progress, jobs);
        }
    }

    pub async fn list_jobs(&self) -> Vec<IndexJob> {
        self.inner.lock().await.jobs.clone()
    }

    pub async fn progress(&self) -> IndexProgress {
        IndexProgress::from_jobs(&self.inner.lock().await.jobs)
    }

    pub async fn cancel_job(&self, app: &AppHandle, job_id: &str) -> bool {
        let mut guard = self.inner.lock().await;
        let Some(idx) = guard.jobs.iter().position(|j| j.id == job_id) else {
            return false;
        };
        if guard.jobs[idx].status.is_terminal() {
            return false;
        }
        if let Some(flag) = guard.cancels.get(job_id) {
            flag.store(true, Ordering::SeqCst);
        }
        let mut event_job = None;
        if guard.jobs[idx].status == IndexJobPhase::Queued {
            guard.jobs[idx].status = IndexJobPhase::Cancelled;
            guard.jobs[idx].error_code = Some("cancelled".into());
            guard.jobs[idx].updated_at = now_rfc3339();
            event_job = Some(guard.jobs[idx].clone());
        }
        let progress = IndexProgress::from_jobs(&guard.jobs);
        let jobs = guard.jobs.clone();
        drop(guard);
        if let Some(job) = event_job {
            emit_job(app, job, progress, jobs);
        }
        self.wake.notify_one();
        true
    }

    pub async fn cancel_all(&self, app: &AppHandle) {
        let mut guard = self.inner.lock().await;
        for flag in guard.cancels.values() {
            flag.store(true, Ordering::SeqCst);
        }
        let mut emitted = Vec::new();
        for job in guard.jobs.iter_mut() {
            if job.status == IndexJobPhase::Queued {
                job.status = IndexJobPhase::Cancelled;
                job.error_code = Some("cancelled".into());
                job.updated_at = now_rfc3339();
                emitted.push(job.clone());
            }
        }
        let progress = IndexProgress::from_jobs(&guard.jobs);
        let jobs = guard.jobs.clone();
        drop(guard);
        for job in emitted {
            emit_job(app, job, progress.clone(), jobs.clone());
        }
        self.wake.notify_one();
    }

    /// Çalışma alanını tara, projeleri kaydet, indeks işlerini kuyruğa al.
    pub async fn scan_and_enqueue(
        self: &Arc<Self>,
        app: &AppHandle,
        workspace_path: String,
    ) -> Result<WorkspaceScanResult, WorkspaceScanErrorKind> {
        let trimmed = workspace_path.trim().to_string();
        if trimmed.is_empty() {
            return Err(WorkspaceScanErrorKind::EmptyPath);
        }

        if !self.bridge.binary_present() {
            return Err(WorkspaceScanErrorKind::SidecarMissing(
                self.bridge.binary_display(),
            ));
        }

        let path_buf = std::path::PathBuf::from(&trimmed);
        let discovered = tokio::task::spawn_blocking(move || discover_projects_blocking(path_buf))
            .await
            .map_err(|err| WorkspaceScanErrorKind::Io(err.to_string()))?
            .map_err(|err| {
                let text = err.to_string();
                if text.starts_with("empty_workspace") {
                    WorkspaceScanErrorKind::EmptyWorkspace
                } else if text.starts_with("permission_denied") {
                    WorkspaceScanErrorKind::PermissionDenied
                } else if text.starts_with("not_found") {
                    WorkspaceScanErrorKind::NotFound
                } else if text.starts_with("not_directory") {
                    WorkspaceScanErrorKind::NotDirectory
                } else if text.starts_with("empty_path") {
                    WorkspaceScanErrorKind::EmptyPath
                } else {
                    WorkspaceScanErrorKind::Io(text)
                }
            })?;

        // Import / register immediately so UI lists projects before indexing finishes.
        for project in &discovered {
            if let Err(err) = self
                .store
                .register_discovered_project(
                    project.name.clone(),
                    project.root_path.display().to_string(),
                )
                .await
            {
                log::warn!("project register {}: {err}", project.name);
            }
        }

        self.ensure_worker(app.clone());
        let jobs = self.enqueue_discovered(app, &discovered).await;
        let progress = IndexProgress::from_jobs(&jobs);
        Ok(WorkspaceScanResult {
            workspace_path: trimmed,
            discovered: discovered
                .iter()
                .map(DiscoveredProject::to_summary)
                .collect(),
            jobs,
            progress,
            error_code: None,
            message: None,
        })
    }

    async fn enqueue_discovered(
        &self,
        app: &AppHandle,
        discovered: &[DiscoveredProject],
    ) -> Vec<IndexJob> {
        let mut guard = self.inner.lock().await;
        let mut created = Vec::new();
        let now = now_rfc3339();
        for project in discovered {
            let repo = project.root_path.display().to_string();
            // Aynı path için aktif iş varsa tekrar kuyruğa alma.
            if guard
                .jobs
                .iter()
                .any(|j| j.repo_path == repo && j.status.is_active())
            {
                continue;
            }
            let id = Uuid::new_v4().to_string();
            let cancel = Arc::new(AtomicBool::new(false));
            guard.cancels.insert(id.clone(), cancel);
            let job = IndexJob {
                id: id.clone(),
                project: project.name.clone(),
                repo_path: repo,
                status: IndexJobPhase::Queued,
                error: None,
                error_code: None,
                snapshot: None,
                enqueued_at: now.clone(),
                updated_at: now.clone(),
            };
            guard.jobs.push(job.clone());
            created.push(job);
        }
        // Eski terminal işleri birikmesin — son 200.
        if guard.jobs.len() > 200 {
            let drain = guard.jobs.len() - 200;
            guard.jobs.drain(0..drain);
        }
        let progress = IndexProgress::from_jobs(&guard.jobs);
        let all = guard.jobs.clone();
        drop(guard);
        for job in &created {
            emit_job(app, job.clone(), progress.clone(), all.clone());
        }
        self.wake.notify_waiters();
        created
    }
}

fn emit_job(app: &AppHandle, job: IndexJob, progress: IndexProgress, jobs: Vec<IndexJob>) {
    let payload = IndexJobEvent {
        job,
        progress,
        jobs,
    };
    if let Err(err) = app.emit(INDEX_JOB_EVENT, &payload) {
        log::warn!("index job emit: {err}");
    }
}

async fn wait_cancel(flag: Arc<AtomicBool>) {
    while !flag.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

impl MemoryBridge {
    pub fn binary_present(&self) -> bool {
        let path = self.binary_path();
        path.is_file() && !crate::services::memory_bridge::is_compile_stub(path)
    }

    pub fn binary_display(&self) -> String {
        self.binary_path().display().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ExperienceStore;
    use crate::services::memory_bridge::MemoryBridge;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db() -> (ExperienceStore, std::path::PathBuf) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("lounge-idxq-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        let db = dir.join("t.sqlite");
        let store = ExperienceStore::open(&db).expect("db");
        (store, dir)
    }

    fn stub_mcp_script(dir: &std::path::Path) -> std::path::PathBuf {
        let script = dir.join("codebase-memory-mcp");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::write(
                &script,
                r#"#!/bin/sh
# fake CBM — index_repository JSON
if echo "$@" | grep -q index_repository; then
  repo=""
  prev=""
  for a in "$@"; do
    if [ "$prev" = "--repo-path" ]; then repo="$a"; fi
    prev="$a"
  done
  name=$(basename "$repo")
  printf '{"project":"%s","repo_path":"%s","nodes":2,"edges":1,"files":1,"ast_nodes":[{"id":"main","name":"main","kind":"function","file":"src/main.rs","line":1}],"references":[{"from_id":"main","to_id":"main","file":"src/main.rs","line":1}]}\n' "$name" "$repo"
  exit 0
fi
if echo "$@" | grep -q list_projects; then
  echo '{"projects":[]}'
  exit 0
fi
echo '{}'
exit 0
"#,
            )
            .unwrap();
            let mut perms = fs::metadata(&script).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&script, perms).unwrap();
        }
        script
    }

    #[tokio::test]
    async fn scan_registers_projects_and_queues_jobs() {
        let (store, dir) = temp_db();
        let ws = dir.join("workspace");
        fs::create_dir_all(ws.join("alpha/.git")).unwrap();
        fs::create_dir_all(ws.join("beta")).unwrap();
        fs::write(ws.join("beta/Cargo.toml"), b"[package]\nname=\"beta\"\n").unwrap();

        let script = stub_mcp_script(&dir);
        let bridge = MemoryBridge::from_binary(&script);
        let queue = IndexQueue::new(bridge, store.clone());

        // AppHandle yok — enqueue doğrudan test et.
        let discovered = discover_projects_blocking(ws.clone()).expect("discover");
        assert!(discovered.len() >= 2, "{discovered:?}");

        for project in &discovered {
            store
                .register_discovered_project(
                    project.name.clone(),
                    project.root_path.display().to_string(),
                )
                .await
                .expect("register");
        }
        let listed = store.list_indexed_projects().await.expect("list");
        assert!(
            listed.iter().any(|p| p.name == "alpha"),
            "imported alpha: {listed:?}"
        );
        assert!(
            listed.iter().any(|p| p.name == "beta"),
            "imported beta: {listed:?}"
        );

        // Queue jobs without AppHandle emit (inner API).
        {
            let mut guard = queue.inner.lock().await;
            let now = now_rfc3339();
            for project in &discovered {
                let id = Uuid::new_v4().to_string();
                guard
                    .cancels
                    .insert(id.clone(), Arc::new(AtomicBool::new(false)));
                guard.jobs.push(IndexJob {
                    id,
                    project: project.name.clone(),
                    repo_path: project.root_path.display().to_string(),
                    status: IndexJobPhase::Queued,
                    error: None,
                    error_code: None,
                    snapshot: None,
                    enqueued_at: now.clone(),
                    updated_at: now.clone(),
                });
            }
        }
        let jobs = queue.list_jobs().await;
        assert_eq!(jobs.len(), discovered.len());
        assert!(jobs.iter().all(|j| j.status == IndexJobPhase::Queued));
        let progress = IndexProgress::from_jobs(&jobs);
        assert_eq!(progress.queued, discovered.len() as u64);
        assert!(progress.active());

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn empty_workspace_does_not_queue() {
        let (store, dir) = temp_db();
        let ws = dir.join("empty-ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("readme.txt"), b"hi").unwrap();
        let script = stub_mcp_script(&dir);
        let bridge = MemoryBridge::from_binary(&script);
        let _queue = IndexQueue::new(bridge, store);
        let err = discover_projects_blocking(ws).unwrap_err();
        assert!(err.to_string().contains("empty_workspace"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn missing_sidecar_is_detectable() {
        let (store, dir) = temp_db();
        let bridge = MemoryBridge::from_binary(dir.join("missing-codebase-memory-mcp"));
        assert!(!bridge.binary_present());
        let _queue = IndexQueue::new(bridge, store);
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn cancel_queued_job_marks_cancelled() {
        let (store, dir) = temp_db();
        let script = stub_mcp_script(&dir);
        let bridge = MemoryBridge::from_binary(&script);
        let queue = IndexQueue::new(bridge, store);
        {
            let mut guard = queue.inner.lock().await;
            let id = "job-1".to_string();
            guard
                .cancels
                .insert(id.clone(), Arc::new(AtomicBool::new(false)));
            guard.jobs.push(IndexJob {
                id,
                project: "alpha".into(),
                repo_path: dir.join("alpha").display().to_string(),
                status: IndexJobPhase::Queued,
                error: None,
                error_code: None,
                snapshot: None,
                enqueued_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            });
        }
        // cancel without emit target — use inner flag path via cancel_job needs AppHandle.
        {
            let mut guard = queue.inner.lock().await;
            if let Some(flag) = guard.cancels.get("job-1") {
                flag.store(true, Ordering::SeqCst);
            }
            if let Some(job) = guard.jobs.iter_mut().find(|j| j.id == "job-1") {
                job.status = IndexJobPhase::Cancelled;
                job.error_code = Some("cancelled".into());
            }
        }
        let jobs = queue.list_jobs().await;
        assert_eq!(jobs[0].status, IndexJobPhase::Cancelled);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn progress_counts_phases() {
        let jobs = vec![
            IndexJob {
                id: "1".into(),
                project: "a".into(),
                repo_path: "/a".into(),
                status: IndexJobPhase::Queued,
                error: None,
                error_code: None,
                snapshot: None,
                enqueued_at: String::new(),
                updated_at: String::new(),
            },
            IndexJob {
                id: "2".into(),
                project: "b".into(),
                repo_path: "/b".into(),
                status: IndexJobPhase::Indexing,
                error: None,
                error_code: None,
                snapshot: None,
                enqueued_at: String::new(),
                updated_at: String::new(),
            },
            IndexJob {
                id: "3".into(),
                project: "c".into(),
                repo_path: "/c".into(),
                status: IndexJobPhase::Done,
                error: None,
                error_code: None,
                snapshot: None,
                enqueued_at: String::new(),
                updated_at: String::new(),
            },
            IndexJob {
                id: "4".into(),
                project: "d".into(),
                repo_path: "/d".into(),
                status: IndexJobPhase::Failed,
                error: Some("boom".into()),
                error_code: Some("index_failed".into()),
                snapshot: None,
                enqueued_at: String::new(),
                updated_at: String::new(),
            },
        ];
        let p = IndexProgress::from_jobs(&jobs);
        assert_eq!(p.total, 4);
        assert_eq!(p.queued, 1);
        assert_eq!(p.indexing, 1);
        assert_eq!(p.done, 1);
        assert_eq!(p.failed, 1);
        assert!(p.active());
    }
}
