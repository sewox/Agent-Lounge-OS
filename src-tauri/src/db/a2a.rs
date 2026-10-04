//! A2A Atomic Core — `agent_sessions`, `idempotency_keys`, `a2a_tasks` + atomik kabul.
//!
//! Görev satırı ve idempotency kaydı **tek SQLite transaction** içinde yazılır.
//! Hop / zombi kurtarma mantığı test edilebilir saat damgası alır.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::models::{now_rfc3339, AgentSession, LoungeTask, TaskStatus, DEFAULT_MAX_HOPS};

/// `LOUNGE_MAX_HOPS` yoksa veya geçersizse [`DEFAULT_MAX_HOPS`].
pub fn configured_max_hops() -> u32 {
    std::env::var("LOUNGE_MAX_HOPS")
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_MAX_HOPS)
}

/// Varsayılan ajan sessizlik süresi — EXECUTING/DISPATCHED zombi → NEEDS_HUMAN.
pub const DEFAULT_AGENT_SILENCE: Duration = Duration::from_secs(120);

pub fn migrate_a2a(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS agent_sessions (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            agent_id TEXT NOT NULL,
            app_kind TEXT NOT NULL,
            native_id TEXT,
            native_ref TEXT,
            workspace_path TEXT NOT NULL,
            is_primary INTEGER NOT NULL DEFAULT 1,
            state TEXT NOT NULL DEFAULT 'unknown',
            owner TEXT NOT NULL DEFAULT 'lounge',
            created_by TEXT NOT NULL,
            last_seen TEXT NOT NULL,
            replaced_by TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_agent_sessions_project_agent
            ON agent_sessions(project_id, agent_id);
        CREATE INDEX IF NOT EXISTS idx_agent_sessions_workspace
            ON agent_sessions(workspace_path);

        CREATE TABLE IF NOT EXISTS session_lock (
            session_id TEXT PRIMARY KEY,
            holder TEXT NOT NULL,
            locked_at TEXT NOT NULL,
            FOREIGN KEY(session_id) REFERENCES agent_sessions(id)
        );

        CREATE TABLE IF NOT EXISTS idempotency_keys (
            idempotency_key TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            source_agent TEXT,
            project_id TEXT,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_idempotency_created
            ON idempotency_keys(created_at);

        CREATE TABLE IF NOT EXISTS a2a_tasks (
            id TEXT PRIMARY KEY,
            root_id TEXT NOT NULL,
            parent_id TEXT,
            project_id TEXT NOT NULL,
            source_agent TEXT NOT NULL,
            target_agent TEXT,
            status TEXT NOT NULL DEFAULT 'QUEUED',
            hop_count INTEGER NOT NULL DEFAULT 0,
            idempotency_key TEXT,
            session_id TEXT,
            source_verified INTEGER NOT NULL DEFAULT 0,
            summary TEXT NOT NULL DEFAULT '',
            payload_json TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_a2a_tasks_root ON a2a_tasks(root_id);
        CREATE INDEX IF NOT EXISTS idx_a2a_tasks_parent ON a2a_tasks(parent_id);
        CREATE INDEX IF NOT EXISTS idx_a2a_tasks_status ON a2a_tasks(status);
        CREATE INDEX IF NOT EXISTS idx_a2a_tasks_updated ON a2a_tasks(updated_at);
        "#,
    )
    .context("a2a schema migrate")?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmitError {
    Duplicate {
        idempotency_key: String,
        existing_task_id: String,
    },
    HopLimitExceeded {
        hop_count: u32,
        max_hops: u32,
    },
    ParentMissing {
        parent_id: String,
    },
}

impl std::fmt::Display for AdmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate {
                idempotency_key,
                existing_task_id,
            } => write!(
                f,
                "idempotency_key tekrarlandı: key={idempotency_key} existing_task={existing_task_id}"
            ),
            Self::HopLimitExceeded {
                hop_count,
                max_hops,
            } => write!(
                f,
                "hop limiti aşıldı: hop_count={hop_count} max_hops={max_hops}"
            ),
            Self::ParentMissing { parent_id } => {
                write!(f, "parent_id zinciri kırık: parent bulunamadı ({parent_id})")
            }
        }
    }
}

impl std::error::Error for AdmitError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmitOk {
    pub task: LoungeTask,
}

/// Sunucu tarafı parent/hop/root normalizasyonu (istemci hop_count yok sayılır).
pub fn normalize_lineage(
    conn: &Connection,
    task: &mut LoungeTask,
    max_hops: u32,
) -> Result<(), AdmitError> {
    // İstemci hop iddiasını sil — yalnızca parent zinciri geçerlidir.
    let claimed_parent = task.parent_task_id.clone();
    match claimed_parent {
        Some(parent_id) => {
            let parent = match load_task_row(conn, &parent_id) {
                Ok(Some(parent)) => parent,
                Ok(None) => return Err(AdmitError::ParentMissing { parent_id }),
                Err(err) => {
                    log::error!("parent load failed ({parent_id}): {err}");
                    return Err(AdmitError::ParentMissing { parent_id });
                }
            };
            task.parent_task_id = Some(parent.id.clone());
            task.root_id = Some(parent.effective_root_id().to_string());
            task.hop_count = parent.hop_count.saturating_add(1);
        }
        None => {
            task.parent_task_id = None;
            if task.root_id.as_deref().unwrap_or("").is_empty() {
                task.root_id = Some(task.id.clone());
            }
            task.hop_count = 0;
        }
    }

    if task.hop_count >= max_hops {
        return Err(AdmitError::HopLimitExceeded {
            hop_count: task.hop_count,
            max_hops,
        });
    }
    Ok(())
}

/// Görev + idempotency kaydını tek transaction'da yazar.
pub fn admit_task_atomic(
    conn: &Connection,
    task: &mut LoungeTask,
    max_hops: u32,
) -> Result<AdmitOk, AdmitError> {
    admit_task_atomic_inner(conn, task, max_hops, false)
}

/// Test: task INSERT sonrası kasıtlı hata — hiçbir satır kalmamalı (atomiklik).
#[cfg(test)]
pub fn admit_task_atomic_fault_after_task(
    conn: &Connection,
    task: &mut LoungeTask,
    max_hops: u32,
) -> Result<AdmitOk, AdmitError> {
    admit_task_atomic_inner(conn, task, max_hops, true)
}

fn admit_task_atomic_inner(
    conn: &Connection,
    task: &mut LoungeTask,
    max_hops: u32,
    fault_after_task: bool,
) -> Result<AdmitOk, AdmitError> {
    normalize_lineage(conn, task, max_hops)?;

    let key = task
        .idempotency_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string);

    if let Some(ref k) = key {
        match lookup_idempotency(conn, k) {
            Ok(Some(existing)) => {
                return Err(AdmitError::Duplicate {
                    idempotency_key: k.clone(),
                    existing_task_id: existing,
                });
            }
            Ok(None) => {}
            Err(err) => {
                log::error!("idempotency lookup: {err}");
                return Err(AdmitError::Duplicate {
                    idempotency_key: k.clone(),
                    existing_task_id: String::new(),
                });
            }
        }
    }

    let tx = match conn.unchecked_transaction() {
        Ok(tx) => tx,
        Err(err) => {
            log::error!("a2a tx begin: {err}");
            return Err(AdmitError::Duplicate {
                idempotency_key: key.clone().unwrap_or_default(),
                existing_task_id: String::new(),
            });
        }
    };

    if let Err(err) = insert_task_row(&tx, task) {
        // Benzersiz id çakışması vb.
        let _ = tx.rollback();
        log::error!("a2a_tasks insert: {err}");
        return Err(AdmitError::Duplicate {
            idempotency_key: key.unwrap_or_default(),
            existing_task_id: task.id.clone(),
        });
    }

    if fault_after_task {
        let _ = tx.rollback();
        return Err(AdmitError::Duplicate {
            idempotency_key: "__fault_injection__".into(),
            existing_task_id: String::new(),
        });
    }

    if let Some(ref k) = key {
        if let Err(err) = insert_idempotency_row(&tx, k, task) {
            let _ = tx.rollback();
            if is_unique_violation(&err) {
                let existing = lookup_idempotency(conn, k)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                return Err(AdmitError::Duplicate {
                    idempotency_key: k.clone(),
                    existing_task_id: existing,
                });
            }
            log::error!("idempotency insert: {err}");
            return Err(AdmitError::Duplicate {
                idempotency_key: k.clone(),
                existing_task_id: String::new(),
            });
        }
    }

    tx.commit().map_err(|e| {
        log::error!("a2a tx commit: {e}");
        AdmitError::Duplicate {
            idempotency_key: key.unwrap_or_default(),
            existing_task_id: String::new(),
        }
    })?;

    Ok(AdmitOk { task: task.clone() })
}

fn is_unique_violation(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(code, _) => {
            code.code == rusqlite::ErrorCode::ConstraintViolation
        }
        _ => false,
    }
}

fn insert_task_row(tx: &Transaction<'_>, task: &LoungeTask) -> Result<()> {
    let now = now_rfc3339();
    let payload = serde_json::to_string(task)?;
    tx.execute(
        r#"
        INSERT INTO a2a_tasks (
            id, root_id, parent_id, project_id, source_agent, target_agent,
            status, hop_count, idempotency_key, session_id, source_verified,
            summary, payload_json, created_at, updated_at
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6,
            ?7, ?8, ?9, ?10, ?11,
            ?12, ?13, ?14, ?15
        )
        "#,
        params![
            task.id,
            task.effective_root_id(),
            task.parent_task_id,
            task.project_id,
            task.source_agent,
            task.target_agent,
            task.status.as_str(),
            task.hop_count as i64,
            task.idempotency_key,
            task.session_id,
            if task.source_verified { 1 } else { 0 },
            task.summary,
            payload,
            task.created_at,
            now,
        ],
    )?;
    Ok(())
}

fn insert_idempotency_row(
    tx: &Transaction<'_>,
    key: &str,
    task: &LoungeTask,
) -> rusqlite::Result<usize> {
    tx.execute(
        r#"
        INSERT INTO idempotency_keys (idempotency_key, task_id, source_agent, project_id, created_at)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
        params![
            key,
            task.id,
            task.source_agent,
            task.project_id,
            now_rfc3339(),
        ],
    )
}

pub fn lookup_idempotency(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT task_id FROM idempotency_keys WHERE idempotency_key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

pub fn load_task_row(conn: &Connection, id: &str) -> Result<Option<LoungeTask>> {
    let payload: Option<String> = conn
        .query_row(
            "SELECT payload_json FROM a2a_tasks WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    match payload {
        Some(raw) => Ok(Some(serde_json::from_str(&raw)?)),
        None => Ok(None),
    }
}

pub fn task_status(conn: &Connection, id: &str) -> Result<Option<TaskStatus>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT status FROM a2a_tasks WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(raw.map(|s| TaskStatus::parse(&s)))
}

pub fn update_task_status(conn: &Connection, id: &str, status: TaskStatus) -> Result<()> {
    let n = conn.execute(
        "UPDATE a2a_tasks SET status = ?1, updated_at = ?2 WHERE id = ?3",
        params![status.as_str(), now_rfc3339(), id],
    )?;
    if n == 0 {
        bail!("a2a_tasks satırı yok: {id}");
    }
    // payload_json içindeki status'u da senkron tut
    if let Some(mut task) = load_task_row(conn, id)? {
        task.status = status;
        let payload = serde_json::to_string(&task)?;
        conn.execute(
            "UPDATE a2a_tasks SET payload_json = ?1 WHERE id = ?2",
            params![payload, id],
        )?;
    }
    Ok(())
}

/// `updated_at` damgasını test edilebilir şekilde ayarla (zombi senaryosu).
pub fn touch_task_updated_at(conn: &Connection, id: &str, updated_at: &str) -> Result<()> {
    conn.execute(
        "UPDATE a2a_tasks SET updated_at = ?1 WHERE id = ?2",
        params![updated_at, id],
    )?;
    Ok(())
}

/// EXECUTING / DISPATCHED görevlerde sessizlik → NEEDS_HUMAN (zombi kalmasın).
///
/// `now_rfc3339` ve `silence` enjekte edilebilir — birim testlerde saat kontrolü.
pub fn mark_silent_tasks_needs_human(
    conn: &Connection,
    now_rfc3339: &str,
    silence: Duration,
) -> Result<Vec<String>> {
    let silence_secs = silence.as_secs() as i64;
    let mut stmt = conn.prepare(
        r#"
        SELECT id, updated_at FROM a2a_tasks
        WHERE status IN ('EXECUTING', 'DISPATCHED', 'RECOVERY_PENDING')
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;

    let now = chrono::DateTime::parse_from_rfc3339(now_rfc3339)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::DateTime::parse_from_str(now_rfc3339, "%+")
                .map(|dt| dt.with_timezone(&chrono::Utc))
        })
        .unwrap_or_else(|_| chrono::Utc::now());

    let mut marked = Vec::new();
    for row in rows {
        let (id, updated_at) = row?;
        let updated = chrono::DateTime::parse_from_rfc3339(&updated_at)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .ok();
        let Some(updated) = updated else {
            continue;
        };
        let age = now.signed_duration_since(updated);
        if age.num_seconds() >= silence_secs {
            update_task_status(conn, &id, TaskStatus::NeedsHuman)?;
            marked.push(id);
        }
    }
    Ok(marked)
}

pub fn upsert_agent_session(conn: &Connection, session: &AgentSession) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO agent_sessions (
            id, project_id, agent_id, app_kind, native_id, native_ref,
            workspace_path, is_primary, state, owner, created_by, last_seen, replaced_by
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
        ON CONFLICT(id) DO UPDATE SET
            native_id = excluded.native_id,
            native_ref = excluded.native_ref,
            state = excluded.state,
            last_seen = excluded.last_seen,
            replaced_by = excluded.replaced_by,
            is_primary = excluded.is_primary
        "#,
        params![
            session.id,
            session.project_id,
            session.agent_id,
            session.app_kind,
            session.native_id,
            session.native_ref,
            session.workspace_path,
            if session.is_primary { 1 } else { 0 },
            session.state,
            session.owner,
            session.created_by,
            session.last_seen,
            session.replaced_by,
        ],
    )?;
    Ok(())
}

pub fn count_agent_sessions(conn: &Connection) -> Result<u64> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM agent_sessions", [], |row| row.get(0))?;
    Ok(n as u64)
}

#[cfg(test)]
fn count_a2a_tasks(conn: &Connection) -> Result<u64> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM a2a_tasks", [], |row| row.get(0))?;
    Ok(n as u64)
}

#[cfg(test)]
fn count_idempotency_keys(conn: &Connection) -> Result<u64> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM idempotency_keys", [], |row| {
        row.get(0)
    })?;
    Ok(n as u64)
}

/// ExperienceStore üzerinden senkron A2A işlemleri.
impl crate::db::ExperienceStore {
    pub fn admit_a2a_task(
        &self,
        task: &mut LoungeTask,
        max_hops: u32,
    ) -> Result<AdmitOk, AdmitError> {
        let conn = self.conn.lock().expect("experience db lock");
        admit_task_atomic(&conn, task, max_hops)
    }

    pub fn a2a_task_status(&self, id: &str) -> Result<Option<TaskStatus>> {
        let conn = self.conn.lock().expect("experience db lock");
        task_status(&conn, id)
    }

    pub fn set_a2a_task_status(&self, id: &str, status: TaskStatus) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        update_task_status(&conn, id, status)
    }

    pub fn recover_silent_a2a_tasks(
        &self,
        now_rfc3339: &str,
        silence: Duration,
    ) -> Result<Vec<String>> {
        let conn = self.conn.lock().expect("experience db lock");
        mark_silent_tasks_needs_human(&conn, now_rfc3339, silence)
    }

    pub fn touch_a2a_updated_at(&self, id: &str, updated_at: &str) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        touch_task_updated_at(&conn, id, updated_at)
    }

    pub fn upsert_session(&self, session: &AgentSession) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        upsert_agent_session(&conn, session)
    }

    pub fn session_count(&self) -> Result<u64> {
        let conn = self.conn.lock().expect("experience db lock");
        count_agent_sessions(&conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ExperienceStore;

    #[test]
    fn migration_creates_a2a_tables() {
        let store = ExperienceStore::memory().unwrap();
        let cols = store.table_columns("agent_sessions").unwrap();
        assert!(cols.iter().any(|c| c == "workspace_path"));
        assert!(cols.iter().any(|c| c == "native_id"));
        let id_cols = store.table_columns("idempotency_keys").unwrap();
        assert!(id_cols.iter().any(|c| c == "idempotency_key"));
        let task_cols = store.table_columns("a2a_tasks").unwrap();
        assert!(task_cols.iter().any(|c| c == "hop_count"));
        assert!(task_cols.iter().any(|c| c == "parent_id"));
    }

    #[test]
    fn hop_chain_blocked_at_max_hops() {
        let store = ExperienceStore::memory().unwrap();
        let max = 10u32;
        let mut prev = LoungeTask::new("a", "p", "root");
        store.admit_a2a_task(&mut prev, max).unwrap();
        assert_eq!(prev.hop_count, 0);

        for hop in 1..max {
            let mut child = LoungeTask::new("b", "p", format!("hop-{hop}"));
            child.parent_task_id = Some(prev.id.clone());
            // İstemci sahte hop — sunucu ezecek.
            child.hop_count = 999;
            store.admit_a2a_task(&mut child, max).unwrap();
            assert_eq!(child.hop_count, hop);
            assert_eq!(child.root_id.as_deref(), Some(prev.effective_root_id()));
            prev = child;
        }

        let mut over = LoungeTask::new("c", "p", "too-deep");
        over.parent_task_id = Some(prev.id.clone());
        let err = store.admit_a2a_task(&mut over, max).unwrap_err();
        assert!(
            matches!(
                err,
                AdmitError::HopLimitExceeded {
                    hop_count: 10,
                    max_hops: 10
                }
            ),
            "unexpected: {err}"
        );
    }

    #[test]
    fn duplicate_idempotency_key_rejected() {
        let store = ExperienceStore::memory().unwrap();
        let mut t1 = LoungeTask::new("a", "p", "first");
        t1.idempotency_key = Some("same-key".into());
        store.admit_a2a_task(&mut t1, 10).unwrap();

        let mut t2 = LoungeTask::new("a", "p", "second");
        t2.idempotency_key = Some("same-key".into());
        let err = store.admit_a2a_task(&mut t2, 10).unwrap_err();
        match err {
            AdmitError::Duplicate {
                idempotency_key,
                existing_task_id,
            } => {
                assert_eq!(idempotency_key, "same-key");
                assert_eq!(existing_task_id, t1.id);
            }
            other => panic!("expected Duplicate, got {other}"),
        }
        let conn = store.conn.lock().unwrap();
        assert_eq!(count_a2a_tasks(&conn).unwrap(), 1);
        assert_eq!(count_idempotency_keys(&conn).unwrap(), 1);
    }

    #[test]
    fn admit_transaction_rolls_back_on_fault() {
        let store = ExperienceStore::memory().unwrap();
        let mut task = LoungeTask::new("a", "p", "faulty");
        task.idempotency_key = Some("fault-key".into());
        let conn = store.conn.lock().unwrap();
        let err = admit_task_atomic_fault_after_task(&conn, &mut task, 10).unwrap_err();
        assert!(matches!(err, AdmitError::Duplicate { .. }));
        assert_eq!(count_a2a_tasks(&conn).unwrap(), 0);
        assert_eq!(count_idempotency_keys(&conn).unwrap(), 0);
    }

    #[test]
    fn silent_executing_task_becomes_needs_human() {
        let store = ExperienceStore::memory().unwrap();
        let mut task = LoungeTask::new("a", "p", "zombie");
        store.admit_a2a_task(&mut task, 10).unwrap();
        store
            .set_a2a_task_status(&task.id, TaskStatus::Executing)
            .unwrap();
        store
            .touch_a2a_updated_at(&task.id, "2026-10-04T12:00:00.000Z")
            .unwrap();

        let marked = store
            .recover_silent_a2a_tasks("2026-10-04T12:03:00.000Z", Duration::from_secs(120))
            .unwrap();
        assert_eq!(marked, vec![task.id.clone()]);
        assert_eq!(
            store.a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::NeedsHuman)
        );
    }

    #[test]
    fn fresh_executing_task_not_marked_needs_human() {
        let store = ExperienceStore::memory().unwrap();
        let mut task = LoungeTask::new("a", "p", "alive");
        store.admit_a2a_task(&mut task, 10).unwrap();
        store
            .set_a2a_task_status(&task.id, TaskStatus::Executing)
            .unwrap();
        store
            .touch_a2a_updated_at(&task.id, "2026-10-04T12:02:30.000Z")
            .unwrap();
        let marked = store
            .recover_silent_a2a_tasks("2026-10-04T12:03:00.000Z", Duration::from_secs(120))
            .unwrap();
        assert!(marked.is_empty());
        assert_eq!(
            store.a2a_task_status(&task.id).unwrap(),
            Some(TaskStatus::Executing)
        );
    }

    #[test]
    fn agent_session_registry_count() {
        let store = ExperienceStore::memory().unwrap();
        assert_eq!(store.session_count().unwrap(), 0);
        let s = AgentSession::new("p", "cursor", "gui", "/tmp/ws", "user_pin");
        store.upsert_session(&s).unwrap();
        assert_eq!(store.session_count().unwrap(), 1);
        store.upsert_session(&s).unwrap();
        assert_eq!(store.session_count().unwrap(), 1, "aynı id tekrar sayılmaz");
    }
}
