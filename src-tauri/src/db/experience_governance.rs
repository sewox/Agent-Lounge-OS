//! Experience CRUD beyond insert/search: update, archive, pin, reviewed, usage, auto-archive.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::experiences::{column_names, has_col};
use super::ExperienceStore;
use crate::models::{
    now_rfc3339, ExperienceRecord, EXPERIENCE_STATUS_ACTIVE, EXPERIENCE_STATUS_ARCHIVED,
};

/// Settings key for auto-archive TTL in days (default 90).
pub const SETTING_EXPERIENCE_TTL_DAYS: &str = "experience_ttl_days";
pub const DEFAULT_EXPERIENCE_TTL_DAYS: u64 = 90;
/// One-time soft-hide of empty placeholder project_index rows.
pub const MIGRATION_PLACEHOLDER_CLEANUP_V1: &str = "migrations.placeholder_cleanup_v1";

const PLACEHOLDER_NAMES: &[&str] = &["backend-legacy", "frontend-new"];

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ExperienceUpdate {
    pub adr_summary: Option<String>,
    pub tags: Option<Vec<String>>,
    pub outcome: Option<crate::models::ExperienceOutcome>,
    pub project_id: Option<String>,
    pub topic: Option<String>,
}

/// Injectable clock for TTL tests (seconds since UNIX epoch as RFC3339 helper).
pub trait ArchiveClock: Send + Sync {
    fn now_rfc3339(&self) -> String;
}

pub struct SystemClock;

impl ArchiveClock for SystemClock {
    fn now_rfc3339(&self) -> String {
        now_rfc3339()
    }
}

/// Fake clock: fixed "now" for tests.
pub struct FakeClock {
    pub now: String,
}

impl ArchiveClock for FakeClock {
    fn now_rfc3339(&self) -> String {
        self.now.clone()
    }
}

impl ExperienceStore {
    pub async fn update_experience(&self, id: String, patch: ExperienceUpdate) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            update_experience_blocking(&conn, &id, &patch)
        })
        .await
        .context("experience update join")?
    }

    pub async fn archive_experience(&self, id: String) -> Result<()> {
        self.set_archive_state(id, true).await
    }

    pub async fn unarchive_experience(&self, id: String) -> Result<()> {
        self.set_archive_state(id, false).await
    }

    async fn set_archive_state(&self, id: String, archive: bool) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            set_archive_blocking(&conn, &id, archive)
        })
        .await
        .context("experience archive join")?
    }

    pub async fn pin_experience(&self, id: String, pinned: bool) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            conn.execute(
                "UPDATE experiences SET is_pinned = ?1, updated_at = ?2 WHERE id = ?3",
                params![if pinned { 1 } else { 0 }, now_rfc3339(), id],
            )?;
            Ok(())
        })
        .await
        .context("experience pin join")?
    }

    pub async fn mark_experience_reviewed(&self, id: String) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            conn.execute(
                "UPDATE experiences SET reviewed = 1, updated_at = ?1 WHERE id = ?2",
                params![now_rfc3339(), id],
            )?;
            Ok(())
        })
        .await
        .context("experience mark_reviewed join")?
    }

    pub async fn count_unreviewed_experiences(&self) -> Result<u64> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM experiences WHERE reviewed = 0 AND status = ?1",
                params![EXPERIENCE_STATUS_ACTIVE],
                |row| row.get(0),
            )?;
            Ok(n as u64)
        })
        .await
        .context("experience count_unreviewed join")?
    }

    pub async fn bump_experience_usage(&self, id: String) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            let now = now_rfc3339();
            conn.execute(
                "UPDATE experiences SET use_count = use_count + 1, last_used_at = ?1, updated_at = ?1 WHERE id = ?2",
                params![now, id],
            )?;
            Ok(())
        })
        .await
        .context("experience bump usage join")?
    }

    /// Search active first; if empty, fall back to archived (O2). Returns (rows, from_archive).
    pub async fn search_experiences_with_archive_fallback(
        &self,
        query: String,
        limit: Option<usize>,
    ) -> Result<(Vec<crate::models::LoungeExperience>, bool)> {
        let active = self
            .search_experiences_filtered(query.clone(), limit, Some(EXPERIENCE_STATUS_ACTIVE))
            .await?;
        if !active.is_empty() {
            return Ok((active, false));
        }
        let archived = self
            .search_experiences_filtered(query, limit, Some(EXPERIENCE_STATUS_ARCHIVED))
            .await?;
        let from_archive = !archived.is_empty();
        Ok((archived, from_archive))
    }

    pub async fn search_experiences_filtered(
        &self,
        query: String,
        limit: Option<usize>,
        status: Option<&str>,
    ) -> Result<Vec<crate::models::LoungeExperience>> {
        let lim = limit.unwrap_or(12).max(1);
        let needle = query.trim().to_string();
        let status = status.map(str::to_string);
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            search_filtered_blocking(&conn, &needle, lim, status.as_deref())
        })
        .await
        .context("experience search filtered join")?
    }

    pub async fn experience_ttl_days(&self) -> Result<u64> {
        match self.get_setting(SETTING_EXPERIENCE_TTL_DAYS.into()).await? {
            Some(raw) => Ok(raw
                .trim()
                .parse::<u64>()
                .unwrap_or(DEFAULT_EXPERIENCE_TTL_DAYS)
                .max(1)),
            None => Ok(DEFAULT_EXPERIENCE_TTL_DAYS),
        }
    }

    pub async fn set_experience_ttl_days(&self, days: u64) -> Result<()> {
        self.set_setting(SETTING_EXPERIENCE_TTL_DAYS.into(), days.max(1).to_string())
            .await
    }

    /// Archive active, unpinned rows unused for `ttl_days` (based on last_used_at or created_at).
    pub async fn auto_archive_stale(&self, ttl_days: u64, clock: &dyn ArchiveClock) -> Result<u64> {
        let now = clock.now_rfc3339();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            auto_archive_blocking(&conn, ttl_days, &now)
        })
        .await
        .context("auto_archive join")?
    }
}

fn update_experience_blocking(conn: &Connection, id: &str, patch: &ExperienceUpdate) -> Result<()> {
    let mut record =
        get_full_record(conn, id)?.ok_or_else(|| anyhow::anyhow!("not found: {id}"))?;
    if record.original_content.is_none() {
        record.original_content = Some(if record.adr_record.trim().is_empty() {
            record.solution_summary.clone()
        } else {
            record.adr_record.clone()
        });
    }
    if let Some(adr) = &patch.adr_summary {
        record.adr_record = adr.clone();
        record.solution_summary = adr.clone();
    }
    if let Some(tags) = &patch.tags {
        record.tags = tags.clone();
    }
    if let Some(outcome) = &patch.outcome {
        record.outcome = outcome.clone();
    }
    if let Some(project_id) = &patch.project_id {
        record.project_id = project_id.clone();
    }
    if let Some(topic) = &patch.topic {
        record.topic = topic.clone();
    }
    let now = now_rfc3339();
    record.updated_at = Some(now.clone());
    let tags = serde_json::to_string(&record.tags)?;
    let outcome = serde_json::to_value(&record.outcome)?
        .as_str()
        .unwrap_or("success")
        .to_string();
    let payload = serde_json::to_string(&record.to_lounge())?;
    conn.execute(
        r#"
        UPDATE experiences SET
            project_id = ?1,
            topic = ?2,
            solution_summary = ?3,
            adr_record = ?4,
            outcome = ?5,
            tags_json = ?6,
            updated_at = ?7,
            original_content = COALESCE(original_content, ?8),
            payload_json = ?9
        WHERE id = ?10
        "#,
        params![
            record.project_id,
            record.topic,
            record.solution_summary,
            record.adr_record,
            outcome,
            tags,
            now,
            record.original_content,
            payload,
            id,
        ],
    )?;
    Ok(())
}

fn set_archive_blocking(conn: &Connection, id: &str, archive: bool) -> Result<()> {
    let now = now_rfc3339();
    if archive {
        conn.execute(
            "UPDATE experiences SET status = ?1, archived_at = ?2, updated_at = ?2 WHERE id = ?3",
            params![EXPERIENCE_STATUS_ARCHIVED, now, id],
        )?;
    } else {
        conn.execute(
            "UPDATE experiences SET status = ?1, archived_at = NULL, updated_at = ?2 WHERE id = ?3",
            params![EXPERIENCE_STATUS_ACTIVE, now, id],
        )?;
    }
    Ok(())
}

fn get_full_record(conn: &Connection, id: &str) -> Result<Option<ExperienceRecord>> {
    let row = conn
        .query_row(
            r#"
            SELECT id, project_id, agent_id, topic, solution_summary, adr_record,
                   outcome, related_task_id, tags_json, created_at, embedding, payload_json,
                   status, reviewed, use_count, last_used_at, archived_at, is_pinned,
                   updated_at, original_content
            FROM experiences WHERE id = ?1
            "#,
            params![id],
            map_full_record,
        )
        .optional()?;
    Ok(row)
}

fn map_full_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExperienceRecord> {
    let outcome: String = row.get(6)?;
    let tags_json: String = row.get(8)?;
    let blob: Option<Vec<u8>> = row.get(10)?;
    let reviewed: i64 = row.get(13)?;
    let use_count: i64 = row.get(14)?;
    let is_pinned: i64 = row.get(17)?;
    Ok(ExperienceRecord {
        id: row.get(0)?,
        project_id: row.get(1)?,
        agent_id: row.get(2)?,
        topic: row.get(3)?,
        solution_summary: row.get(4)?,
        adr_record: row.get(5)?,
        outcome: serde_json::from_value(serde_json::Value::String(outcome))
            .unwrap_or(crate::models::ExperienceOutcome::Success),
        related_task_id: row.get(7)?,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        created_at: row.get(9)?,
        embedding: blob
            .as_deref()
            .and_then(super::embedding::decode_embedding)
            .unwrap_or_default(),
        status: row
            .get::<_, Option<String>>(12)?
            .unwrap_or_else(|| EXPERIENCE_STATUS_ACTIVE.into()),
        reviewed: reviewed != 0,
        use_count: use_count.max(0) as u64,
        last_used_at: row.get(15)?,
        archived_at: row.get(16)?,
        is_pinned: is_pinned != 0,
        updated_at: row.get(18)?,
        original_content: row.get(19)?,
    })
}

fn search_filtered_blocking(
    conn: &Connection,
    needle: &str,
    limit: usize,
    status: Option<&str>,
) -> Result<Vec<crate::models::LoungeExperience>> {
    let status_clause = if status.is_some() {
        "AND status = ?2"
    } else {
        ""
    };
    let sql = format!(
        r#"
        SELECT payload_json, id, project_id, agent_id, topic, solution_summary, adr_record,
               outcome, related_task_id, tags_json, created_at
        FROM experiences
        WHERE (?1 = '' OR lower(topic || ' ' || solution_summary || ' ' || adr_record || ' ' || tags_json) LIKE '%' || lower(?1) || '%')
          {status_clause}
        ORDER BY is_pinned DESC, created_at DESC
        LIMIT ?{limit_idx}
        "#,
        limit_idx = if status.is_some() { 3 } else { 2 }
    );
    let mut stmt = conn.prepare(&sql)?;
    let mapped = if let Some(status) = status {
        stmt.query_map(params![needle, status, limit as i64], map_lounge_row)?
    } else {
        stmt.query_map(params![needle, limit as i64], map_lounge_row)?
    };
    let mut out = Vec::new();
    for row in mapped {
        out.push(row?);
    }
    Ok(out)
}

fn map_lounge_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<crate::models::LoungeExperience> {
    let payload: String = row.get(0)?;
    if let Ok(exp) = serde_json::from_str(&payload) {
        return Ok(exp);
    }
    let outcome: String = row.get(6)?;
    let tags_json: String = row.get(8)?;
    Ok(crate::models::LoungeExperience {
        id: row.get(1)?,
        msg_type: "experience".into(),
        agent: row.get(3)?,
        project_id: row.get(2)?,
        adr_summary: {
            let adr: String = row.get(5)?;
            if adr.trim().is_empty() {
                row.get(4)?
            } else {
                adr
            }
        },
        outcome: serde_json::from_value(serde_json::Value::String(outcome))
            .unwrap_or(crate::models::ExperienceOutcome::Success),
        related_task_id: row.get(7)?,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        created_at: row.get(9)?,
    })
}

fn auto_archive_blocking(conn: &Connection, ttl_days: u64, now_rfc: &str) -> Result<u64> {
    let ttl_secs = (ttl_days.max(1) as i64) * 86_400;
    let now = chrono::DateTime::parse_from_rfc3339(now_rfc)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .or_else(|_| {
            chrono::DateTime::parse_from_rfc3339(&format!("{now_rfc}Z"))
                .map(|dt| dt.with_timezone(&chrono::Utc))
        })
        .unwrap_or_else(|_| chrono::Utc::now());

    let mut stmt = conn.prepare(
        r#"
        SELECT id, COALESCE(last_used_at, created_at), is_pinned
        FROM experiences
        WHERE status = ?1 AND is_pinned = 0
        "#,
    )?;
    let rows = stmt.query_map(params![EXPERIENCE_STATUS_ACTIVE], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;

    let mut ids = Vec::new();
    for row in rows {
        let (id, anchor, pinned) = row?;
        if pinned != 0 {
            continue;
        }
        let Ok(anchor_dt) = chrono::DateTime::parse_from_rfc3339(&anchor) else {
            continue;
        };
        let age = now.signed_duration_since(anchor_dt.with_timezone(&chrono::Utc));
        if age.num_seconds() >= ttl_secs {
            ids.push(id);
        }
    }

    let archived_at = now.to_rfc3339();
    for id in &ids {
        conn.execute(
            "UPDATE experiences SET status = ?1, archived_at = ?2, updated_at = ?2 WHERE id = ?3",
            params![EXPERIENCE_STATUS_ARCHIVED, archived_at, id],
        )?;
    }
    Ok(ids.len() as u64)
}

/// Idempotent PR-1 column migration + K4 backfill (existing rows reviewed=1).
pub fn migrate_experience_governance(conn: &Connection) -> Result<()> {
    let cols = column_names(conn, "experiences")?;
    let additions: &[(&str, &str)] = &[
        ("status", "TEXT NOT NULL DEFAULT 'active'"),
        ("reviewed", "INTEGER NOT NULL DEFAULT 0"),
        ("use_count", "INTEGER NOT NULL DEFAULT 0"),
        ("last_used_at", "TEXT"),
        ("archived_at", "TEXT"),
        ("is_pinned", "INTEGER NOT NULL DEFAULT 0"),
        ("updated_at", "TEXT"),
        ("original_content", "TEXT"),
    ];
    let mut added_reviewed = false;
    for (name, decl) in additions {
        if !has_col(&cols, name) {
            conn.execute(
                &format!("ALTER TABLE experiences ADD COLUMN {name} {decl}"),
                [],
            )?;
            if *name == "reviewed" {
                added_reviewed = true;
            }
        }
    }
    // K4: existing rows at migration time count as reviewed.
    if added_reviewed {
        conn.execute("UPDATE experiences SET reviewed = 1", [])?;
    }
    // Normalize legacy status values: draft/approved → active; deprecated → archived.
    conn.execute(
        "UPDATE experiences SET status = 'active' WHERE status IS NULL OR status IN ('draft','approved','')",
        [],
    )?;
    let now = now_rfc3339();
    conn.execute(
        "UPDATE experiences SET status = 'archived', archived_at = COALESCE(archived_at, ?1), updated_at = COALESCE(updated_at, ?1) WHERE lower(status) = 'deprecated'",
        params![now],
    )?;
    conn.execute(
        "UPDATE experiences SET original_content = COALESCE(original_content, adr_record, solution_summary) WHERE original_content IS NULL OR original_content = ''",
        [],
    )?;
    conn.execute(
        "UPDATE experiences SET updated_at = COALESCE(updated_at, created_at) WHERE updated_at IS NULL OR updated_at = ''",
        [],
    )?;
    conn.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_experiences_status ON experiences(status);
        CREATE INDEX IF NOT EXISTS idx_experiences_reviewed ON experiences(reviewed);
        CREATE INDEX IF NOT EXISTS idx_experiences_pinned ON experiences(is_pinned);
        "#,
    )?;
    Ok(())
}

/// Soft-hide placeholder grandtest projects (K9) — never DELETE.
/// Runs once via settings key `migrations.placeholder_cleanup_v1`.
pub fn cleanup_placeholder_projects(conn: &Connection) -> Result<u64> {
    ensure_project_flags_table(conn)?;
    let done: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1",
            params![MIGRATION_PLACEHOLDER_CLEANUP_V1],
            |row| row.get(0),
        )
        .optional()?;
    if matches!(done.as_deref(), Some("1") | Some("true") | Some("\"1\"")) {
        return Ok(0);
    }

    let mut total = 0u64;
    for name in PLACEHOLDER_NAMES {
        let nodes: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM project_index WHERE lower(project_id) = lower(?1) AND kind = 'node'",
                params![name],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let files: i64 = conn
            .query_row(
                "SELECT COUNT(DISTINCT NULLIF(file_path, '')) FROM project_index WHERE lower(project_id) = lower(?1)",
                params![name],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let repo_path: Option<String> = conn
            .query_row(
                "SELECT MAX(repo_path) FROM project_index WHERE lower(project_id) = lower(?1)",
                params![name],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();
        let empty_repo = repo_path
            .as_deref()
            .map(|p| p.trim().is_empty())
            .unwrap_or(true);
        let zero_graph = nodes == 0 && files == 0;
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM project_index WHERE lower(project_id) = lower(?1) LIMIT 1",
                params![name],
                |_| Ok(true),
            )
            .optional()?
            .unwrap_or(false);
        if exists && (empty_repo || zero_graph) {
            conn.execute(
                r#"
                INSERT INTO project_flags(project_id, hidden) VALUES (?1, 1)
                ON CONFLICT(project_id) DO UPDATE SET hidden = 1
                "#,
                params![name],
            )?;
            total += 1;
        }
    }

    conn.execute(
        "INSERT INTO settings(key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![MIGRATION_PLACEHOLDER_CLEANUP_V1, "1"],
    )?;
    Ok(total)
}

pub fn ensure_project_flags_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS project_flags (
            project_id TEXT PRIMARY KEY,
            hidden INTEGER NOT NULL DEFAULT 0
        );
        "#,
    )?;
    Ok(())
}

/// True when project_id is soft-hidden via project_flags.
pub fn is_project_hidden(conn: &Connection, project_id: &str) -> Result<bool> {
    ensure_project_flags_table(conn)?;
    let hidden: Option<i64> = conn
        .query_row(
            "SELECT hidden FROM project_flags WHERE lower(project_id) = lower(?1)",
            params![project_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(hidden.unwrap_or(0) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ExperienceOutcome, LoungeTask};

    #[tokio::test]
    async fn migration_marks_existing_reviewed() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE experiences (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                topic TEXT NOT NULL,
                solution_summary TEXT NOT NULL,
                adr_record TEXT NOT NULL,
                outcome TEXT NOT NULL DEFAULT 'success',
                related_task_id TEXT,
                tags_json TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL,
                embedding BLOB,
                payload_json TEXT NOT NULL
            );
            INSERT INTO experiences (
                id, project_id, agent_id, topic, solution_summary, adr_record,
                outcome, tags_json, created_at, payload_json
            ) VALUES (
                'old-1', 'p', 'agent', 't', 's', 'adr',
                'success', '[]', '2026-01-01T00:00:00Z', '{}'
            );
            "#,
        )
        .unwrap();
        migrate_experience_governance(&conn).unwrap();
        let reviewed: i64 = conn
            .query_row(
                "SELECT reviewed FROM experiences WHERE id = 'old-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(reviewed, 1);
    }

    #[tokio::test]
    async fn crud_archive_pin_reviewed_roundtrip() {
        let store = ExperienceStore::memory().unwrap();
        let task = LoungeTask::new("cursor", "proj", "topic");
        let mut record = ExperienceRecord::from_task(
            &task,
            "solution",
            "adr body",
            ExperienceOutcome::Success,
            vec!["t".into()],
        );
        record.reviewed = false;
        let id = record.id.clone();
        store.insert_record(record).await.unwrap();

        store
            .update_experience(
                id.clone(),
                ExperienceUpdate {
                    adr_summary: Some("edited adr".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let loaded = store.get_record(id.clone()).await.unwrap().unwrap();
        assert_eq!(loaded.adr_record, "edited adr");
        assert_eq!(loaded.original_content.as_deref(), Some("adr body"));

        store.pin_experience(id.clone(), true).await.unwrap();
        store.mark_experience_reviewed(id.clone()).await.unwrap();
        store.archive_experience(id.clone()).await.unwrap();
        let archived = store.get_record(id.clone()).await.unwrap().unwrap();
        assert_eq!(archived.status, EXPERIENCE_STATUS_ARCHIVED);
        assert!(archived.is_pinned);
        assert!(archived.reviewed);
        assert!(archived.archived_at.is_some());

        store.unarchive_experience(id.clone()).await.unwrap();
        let active = store.get_record(id).await.unwrap().unwrap();
        assert_eq!(active.status, EXPERIENCE_STATUS_ACTIVE);
        assert!(active.archived_at.is_none());
    }

    #[tokio::test]
    async fn auto_archive_respects_ttl_and_pin_with_fake_clock() {
        let store = ExperienceStore::memory().unwrap();
        let task = LoungeTask::new("a", "p", "old");
        let mut old =
            ExperienceRecord::from_task(&task, "s", "a", ExperienceOutcome::Success, vec![]);
        old.created_at = "2020-01-01T00:00:00+00:00".into();
        old.last_used_at = Some("2020-01-01T00:00:00+00:00".into());
        old.is_pinned = false;
        let old_id = old.id.clone();
        store.insert_record(old).await.unwrap();

        let mut pinned = ExperienceRecord::from_task(
            &LoungeTask::new("a", "p", "pinned"),
            "s",
            "a",
            ExperienceOutcome::Success,
            vec![],
        );
        pinned.created_at = "2020-01-01T00:00:00+00:00".into();
        pinned.last_used_at = Some("2020-01-01T00:00:00+00:00".into());
        pinned.is_pinned = true;
        let pinned_id = pinned.id.clone();
        store.insert_record(pinned).await.unwrap();
        // insert_record may not persist is_pinned via old insert path — force pin.
        store.pin_experience(pinned_id.clone(), true).await.unwrap();

        let mut fresh = ExperienceRecord::from_task(
            &LoungeTask::new("a", "p", "fresh"),
            "s",
            "a",
            ExperienceOutcome::Success,
            vec![],
        );
        fresh.created_at = "2026-09-20T00:00:00+00:00".into();
        fresh.last_used_at = Some("2026-09-20T00:00:00+00:00".into());
        fresh.is_pinned = false;
        let fresh_id = fresh.id.clone();
        store.insert_record(fresh).await.unwrap();

        let clock = FakeClock {
            now: "2026-09-26T00:00:00+00:00".into(),
        };
        let n = store.auto_archive_stale(90, &clock).await.unwrap();
        assert_eq!(n, 1, "exactly one stale unpinned row should archive");
        let old_row = store.get_record(old_id).await.unwrap().unwrap();
        assert_eq!(old_row.status, EXPERIENCE_STATUS_ARCHIVED);
        let pin_row = store.get_record(pinned_id).await.unwrap().unwrap();
        assert_eq!(pin_row.status, EXPERIENCE_STATUS_ACTIVE);
        let fresh_row = store.get_record(fresh_id).await.unwrap().unwrap();
        assert_eq!(fresh_row.status, EXPERIENCE_STATUS_ACTIVE);
    }

    #[tokio::test]
    async fn archive_search_fallback_tags_archived() {
        let store = ExperienceStore::memory().unwrap();
        let record = ExperienceRecord::from_task(
            &LoungeTask::new("a", "p", "unique-archive-token-zz"),
            "unique-archive-token-zz solution",
            "unique-archive-token-zz adr",
            ExperienceOutcome::Success,
            vec![],
        );
        let id = record.id.clone();
        store.insert_record(record).await.unwrap();
        store.archive_experience(id.clone()).await.unwrap();

        let (hits, from_archive) = store
            .search_experiences_with_archive_fallback("unique-archive-token-zz".into(), Some(5))
            .await
            .unwrap();
        assert!(from_archive);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].id, id);
    }

    #[test]
    fn deprecated_status_maps_to_archived() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE experiences (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                topic TEXT NOT NULL,
                solution_summary TEXT NOT NULL,
                adr_record TEXT NOT NULL,
                outcome TEXT NOT NULL DEFAULT 'success',
                related_task_id TEXT,
                tags_json TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL,
                embedding BLOB,
                payload_json TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'deprecated'
            );
            INSERT INTO experiences (
                id, project_id, agent_id, topic, solution_summary, adr_record,
                outcome, tags_json, created_at, payload_json, status
            ) VALUES (
                'dep-1', 'p', 'agent', 't', 's', 'adr',
                'success', '[]', '2026-01-01T00:00:00Z', '{}', 'deprecated'
            );
            "#,
        )
        .unwrap();
        migrate_experience_governance(&conn).unwrap();
        let (status, archived_at): (String, Option<String>) = conn
            .query_row(
                "SELECT status, archived_at FROM experiences WHERE id = 'dep-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, EXPERIENCE_STATUS_ARCHIVED);
        assert!(archived_at.is_some());
    }

    #[test]
    fn placeholder_cleanup_runs_once_and_soft_hides() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE settings (key TEXT PRIMARY KEY, value_json TEXT NOT NULL);
            CREATE TABLE project_index (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL,
              repo_path TEXT NOT NULL,
              kind TEXT NOT NULL,
              name TEXT NOT NULL,
              file_path TEXT,
              line INTEGER,
              target TEXT,
              ref_count INTEGER NOT NULL DEFAULT 0,
              detail TEXT,
              payload_json TEXT NOT NULL DEFAULT '{}',
              indexed_at TEXT NOT NULL
            );
            INSERT INTO project_index (
                id, project_id, repo_path, kind, name, indexed_at
            ) VALUES
                ('p1', 'frontend-new', '', 'meta', 'frontend-new', '2026-01-01T00:00:00Z'),
                ('p2', 'backend-legacy', '', 'meta', 'backend-legacy', '2026-01-01T00:00:00Z'),
                ('p3', 'frontend-new-real', '/repos/frontend-new', 'node', 'App', '2026-01-01T00:00:00Z');
            "#,
        )
        .unwrap();
        // Real frontend-new with valid repo + node must stay visible.
        conn.execute(
            r#"
            INSERT INTO project_index (
                id, project_id, repo_path, kind, name, file_path, indexed_at
            ) VALUES (
                'p4', 'frontend-new', '/Users/dev/frontend-new', 'node', 'Main', 'src/main.ts', '2026-01-01T00:00:00Z'
            )
            "#,
            [],
        )
        .unwrap();

        let first = cleanup_placeholder_projects(&conn).unwrap();
        // backend-legacy (empty) hidden; frontend-new has valid repo+node so not hidden by zero_graph —
        // but wait: frontend-new also has empty row AND a real row. Signature is per project_id:
        // nodes > 0 and repo_path non-empty → should NOT hide.
        assert!(first >= 1, "should hide at least backend-legacy");
        assert!(is_project_hidden(&conn, "backend-legacy").unwrap());
        assert!(!is_project_hidden(&conn, "frontend-new").unwrap());

        let second = cleanup_placeholder_projects(&conn).unwrap();
        assert_eq!(second, 0, "migration must be one-shot");

        // Empty-only placeholder without nodes.
        let conn2 = Connection::open_in_memory().unwrap();
        conn2
            .execute_batch(
                r#"
            CREATE TABLE settings (key TEXT PRIMARY KEY, value_json TEXT NOT NULL);
            CREATE TABLE project_index (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL,
              repo_path TEXT NOT NULL,
              kind TEXT NOT NULL,
              name TEXT NOT NULL,
              file_path TEXT,
              line INTEGER,
              target TEXT,
              ref_count INTEGER NOT NULL DEFAULT 0,
              detail TEXT,
              payload_json TEXT NOT NULL DEFAULT '{}',
              indexed_at TEXT NOT NULL
            );
            INSERT INTO project_index (
                id, project_id, repo_path, kind, name, indexed_at
            ) VALUES
                ('e1', 'frontend-new', '', 'meta', 'frontend-new', '2026-01-01T00:00:00Z');
            "#,
            )
            .unwrap();
        assert_eq!(cleanup_placeholder_projects(&conn2).unwrap(), 1);
        assert!(is_project_hidden(&conn2, "frontend-new").unwrap());
        let remaining: i64 = conn2
            .query_row(
                "SELECT COUNT(*) FROM project_index WHERE project_id = 'frontend-new'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 1, "must soft-hide, never DELETE");
    }
}
