//! SHA-256 hash-chained tamper-evident event journal (F2 / PR-Journal-1).
//!
//! Append-only SQLite table `event_journal` with deterministic genesis chaining.
//! Every write scrubs secrets/IPs/home paths **before** hashing and persistence.

use anyhow::{Context, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bridge::hygiene;
use crate::models::now_rfc3339;

pub const GENESIS_PREV_HASH: &str = "GENESIS";

pub const DEFAULT_JOURNAL_QUERY_LIMIT: usize = 100;
pub const MAX_JOURNAL_QUERY_LIMIT: usize = 1000;

// A2A task lifecycle
pub const ACTION_TASK_REQUESTED: &str = "task.requested";
pub const ACTION_TASK_CLAIMED: &str = "task.claimed";
pub const ACTION_TASK_APPROVAL_PENDING: &str = "task.approval_pending";
pub const ACTION_TASK_APPROVAL_APPROVED: &str = "task.approval_approved";
pub const ACTION_TASK_APPROVAL_REJECTED: &str = "task.approval_rejected";
pub const ACTION_TASK_COMPLETED: &str = "task.completed";
pub const ACTION_TASK_CANCELLED: &str = "task.cancelled";
pub const ACTION_TASK_EXPIRED_OR_ORPHAN: &str = "task.expired_or_orphan";

// Knowledge vault / review queue
pub const ACTION_EXPERIENCE_RECORDED: &str = "experience.recorded";
pub const ACTION_EXPERIENCE_REVIEWED: &str = "experience.reviewed";
pub const ACTION_EXPERIENCE_REJECTED: &str = "experience.rejected";
pub const ACTION_EXPERIENCE_UNDO_REJECTED: &str = "experience.undo_rejected";
pub const ACTION_EXPERIENCE_ARCHIVED: &str = "experience.archived";
pub const ACTION_EXPERIENCE_UNARCHIVED: &str = "experience.unarchived";

// Global halt / resume
pub const ACTION_CONTROL_HALTED: &str = "control.halted";
pub const ACTION_CONTROL_RESUMED: &str = "control.resumed";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalEntry {
    pub seq: i64,
    pub event_id: String,
    pub timestamp: String,
    pub actor: String,
    pub action_type: String,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub payload_json: String,
    pub prev_hash: String,
    pub entry_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalVerificationReport {
    pub valid: bool,
    pub checked_count: usize,
    pub head_hash: Option<String>,
    pub broken_at_seq: Option<i64>,
    pub broken_event_id: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalQueryFilter {
    pub task_id: Option<String>,
    pub project_id: Option<String>,
    pub action_type: Option<String>,
    pub actor: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalWhyReport {
    pub target_id: String,
    pub task_id: Option<String>,
    pub project_id: Option<String>,
    pub events: Vec<JournalEntry>,
    pub total_duration_ms: Option<u64>,
    pub approval_wait_ms: u64,
    pub execution_ms: Option<u64>,
    pub chain_valid: bool,
}

#[derive(Debug, Clone)]
pub struct JournalAppendInput {
    pub event_id: Option<String>,
    pub timestamp: Option<String>,
    pub actor: String,
    pub action_type: String,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub payload_json: String,
}

impl JournalAppendInput {
    pub fn new(
        actor: impl Into<String>,
        action_type: impl Into<String>,
        payload_json: impl Into<String>,
    ) -> Self {
        Self {
            event_id: None,
            timestamp: None,
            actor: actor.into(),
            action_type: action_type.into(),
            project_id: None,
            task_id: None,
            payload_json: payload_json.into(),
        }
    }

    pub fn with_task_id(mut self, task_id: impl Into<String>) -> Self {
        self.task_id = Some(task_id.into());
        self
    }

    pub fn with_project_id(mut self, project_id: Option<String>) -> Self {
        self.project_id = project_id;
        self
    }

    pub fn with_timestamp(mut self, timestamp: impl Into<String>) -> Self {
        self.timestamp = Some(timestamp.into());
        self
    }

    pub fn with_event_id(mut self, event_id: impl Into<String>) -> Self {
        self.event_id = Some(event_id.into());
        self
    }
}

/// Idempotent schema migration for `event_journal`.
pub fn migrate_journal(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS event_journal (
            seq INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id TEXT NOT NULL UNIQUE,
            timestamp TEXT NOT NULL,
            actor TEXT NOT NULL,
            action_type TEXT NOT NULL,
            project_id TEXT,
            task_id TEXT,
            payload_json TEXT NOT NULL,
            prev_hash TEXT NOT NULL,
            entry_hash TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_event_journal_task_seq
            ON event_journal(task_id, seq);
        CREATE INDEX IF NOT EXISTS idx_event_journal_project_seq
            ON event_journal(project_id, seq);
        CREATE INDEX IF NOT EXISTS idx_event_journal_action_seq
            ON event_journal(action_type, seq);
        CREATE INDEX IF NOT EXISTS idx_event_journal_timestamp
            ON event_journal(timestamp);
        "#,
    )?;
    Ok(())
}

/// Normalize project id via alias/canonical resolution when present.
pub fn canonical_project_id(conn: &Connection, project_id: Option<&str>) -> Option<String> {
    let raw = project_id.map(str::trim).filter(|s| !s.is_empty())?;
    match crate::db::project_identity::prefer_canonical(conn, raw) {
        Ok(c) => {
            let scrubbed = hygiene::scrub(&c).text;
            if scrubbed.trim().is_empty() {
                None
            } else {
                Some(scrubbed)
            }
        }
        Err(_) => {
            let scrubbed = hygiene::scrub(raw).text;
            if scrubbed.trim().is_empty() {
                None
            } else {
                Some(scrubbed)
            }
        }
    }
}

fn project_id_for_hash(project_id: &Option<String>) -> &str {
    project_id.as_deref().unwrap_or("")
}

fn task_id_for_hash(task_id: &Option<String>) -> &str {
    task_id.as_deref().unwrap_or("")
}

/// Fields required to build the deterministic hash preimage.
pub struct JournalHashParts<'a> {
    pub seq: i64,
    pub event_id: &'a str,
    pub timestamp: &'a str,
    pub actor: &'a str,
    pub action_type: &'a str,
    pub project_id: &'a Option<String>,
    pub task_id: &'a Option<String>,
    pub scrubbed_payload_json: &'a str,
    pub prev_hash: &'a str,
}

/// Deterministic canonical preimage for `entry_hash`.
pub fn journal_entry_preimage(parts: &JournalHashParts<'_>) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}",
        parts.seq,
        parts.event_id,
        parts.timestamp,
        parts.actor,
        parts.action_type,
        project_id_for_hash(parts.project_id),
        task_id_for_hash(parts.task_id),
        parts.scrubbed_payload_json,
        parts.prev_hash,
    )
}

pub fn compute_entry_hash(preimage: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(preimage.as_bytes());
    hex_encode(&hasher.finalize())
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn scrub_actor(actor: &str) -> String {
    hygiene::scrub(actor).text
}

fn scrub_payload(payload_json: &str) -> String {
    hygiene::scrub(payload_json).text
}

fn normalize_optional_id(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let t = v.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JournalEntry> {
    Ok(JournalEntry {
        seq: row.get(0)?,
        event_id: row.get(1)?,
        timestamp: row.get(2)?,
        actor: row.get(3)?,
        action_type: row.get(4)?,
        project_id: row.get(5)?,
        task_id: row.get(6)?,
        payload_json: row.get(7)?,
        prev_hash: row.get(8)?,
        entry_hash: row.get(9)?,
    })
}

const SELECT_COLS: &str = "seq, event_id, timestamp, actor, action_type, project_id, task_id, payload_json, prev_hash, entry_hash";

/// Append one journal entry under the caller's connection (mutex / txn serialized).
pub fn append_journal_entry_conn(
    conn: &Connection,
    input: JournalAppendInput,
) -> Result<JournalEntry> {
    let event_id = input
        .event_id
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let timestamp = input
        .timestamp
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(now_rfc3339);
    let actor = scrub_actor(&input.actor);
    let action_type = input.action_type.trim().to_string();
    if action_type.is_empty() {
        anyhow::bail!("journal action_type gerekli");
    }
    let task_id = normalize_optional_id(input.task_id);
    let project_id = canonical_project_id(conn, input.project_id.as_deref());
    let payload_json = scrub_payload(&input.payload_json);

    let (prev_seq, prev_hash) = conn
        .query_row(
            "SELECT seq, entry_hash FROM event_journal ORDER BY seq DESC LIMIT 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .map(|(seq, hash)| (Some(seq), hash))
        .unwrap_or((None, GENESIS_PREV_HASH.to_string()));

    let seq = match prev_seq {
        None => 1,
        Some(prev) => {
            let next = prev + 1;
            if next <= prev {
                anyhow::bail!("journal seq monotonicity broken: prev={prev}");
            }
            next
        }
    };

    let preimage = journal_entry_preimage(&JournalHashParts {
        seq,
        event_id: &event_id,
        timestamp: &timestamp,
        actor: &actor,
        action_type: &action_type,
        project_id: &project_id,
        task_id: &task_id,
        scrubbed_payload_json: &payload_json,
        prev_hash: &prev_hash,
    });
    let entry_hash = compute_entry_hash(&preimage);

    conn.execute(
        r#"
        INSERT INTO event_journal (
            seq, event_id, timestamp, actor, action_type, project_id, task_id,
            payload_json, prev_hash, entry_hash
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        "#,
        params![
            seq,
            event_id,
            timestamp,
            actor,
            action_type,
            project_id,
            task_id,
            payload_json,
            prev_hash,
            entry_hash,
        ],
    )?;

    Ok(JournalEntry {
        seq,
        event_id,
        timestamp,
        actor,
        action_type,
        project_id,
        task_id,
        payload_json,
        prev_hash,
        entry_hash,
    })
}

/// Best-effort append — never fails the caller.
pub fn append_journal_best_effort(conn: &Connection, input: JournalAppendInput) {
    if let Err(err) = append_journal_entry_conn(conn, input) {
        log::warn!("event_journal append failed (best-effort): {err:#}");
    }
}

/// Append with `BEGIN IMMEDIATE` when the caller holds only the store mutex.
pub fn append_journal_entry_immediate(
    conn: &mut Connection,
    input: JournalAppendInput,
) -> Result<JournalEntry> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let entry = append_journal_entry_conn(&tx, input)?;
    tx.commit()?;
    Ok(entry)
}

pub fn verify_journal_chain(conn: &Connection) -> Result<JournalVerificationReport> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {SELECT_COLS} FROM event_journal ORDER BY seq ASC"
    ))?;
    let rows = stmt.query_map([], map_row)?;

    let mut checked = 0usize;
    let mut expected_prev = GENESIS_PREV_HASH.to_string();
    let mut head_hash: Option<String> = None;

    for (expected_seq, row) in (1_i64..).zip(rows) {
        let entry = row?;
        checked += 1;

        if entry.seq != expected_seq {
            return Ok(JournalVerificationReport {
                valid: false,
                checked_count: checked,
                head_hash,
                broken_at_seq: Some(entry.seq),
                broken_event_id: Some(entry.event_id),
                reason: Some(format!(
                    "seq gap: expected {expected_seq}, got {}",
                    entry.seq
                )),
            });
        }

        if entry.prev_hash != expected_prev {
            return Ok(JournalVerificationReport {
                valid: false,
                checked_count: checked,
                head_hash,
                broken_at_seq: Some(entry.seq),
                broken_event_id: Some(entry.event_id),
                reason: Some(format!(
                    "prev_hash mismatch: expected {expected_prev}, got {}",
                    entry.prev_hash
                )),
            });
        }

        let preimage = journal_entry_preimage(&JournalHashParts {
            seq: entry.seq,
            event_id: &entry.event_id,
            timestamp: &entry.timestamp,
            actor: &entry.actor,
            action_type: &entry.action_type,
            project_id: &entry.project_id,
            task_id: &entry.task_id,
            scrubbed_payload_json: &entry.payload_json,
            prev_hash: &entry.prev_hash,
        });
        let expected_hash = compute_entry_hash(&preimage);
        if entry.entry_hash != expected_hash {
            return Ok(JournalVerificationReport {
                valid: false,
                checked_count: checked,
                head_hash,
                broken_at_seq: Some(entry.seq),
                broken_event_id: Some(entry.event_id),
                reason: Some("entry_hash mismatch (payload or fields tampered)".into()),
            });
        }

        expected_prev = entry.entry_hash.clone();
        head_hash = Some(entry.entry_hash);
    }

    Ok(JournalVerificationReport {
        valid: true,
        checked_count: checked,
        head_hash,
        broken_at_seq: None,
        broken_event_id: None,
        reason: None,
    })
}

pub fn query_journal(conn: &Connection, filter: &JournalQueryFilter) -> Result<Vec<JournalEntry>> {
    let limit = filter
        .limit
        .unwrap_or(DEFAULT_JOURNAL_QUERY_LIMIT)
        .clamp(1, MAX_JOURNAL_QUERY_LIMIT);

    let mut sql = format!("SELECT {SELECT_COLS} FROM event_journal WHERE 1=1");
    let mut vals: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(ref task_id) = filter.task_id {
        let t = task_id.trim();
        if !t.is_empty() {
            sql.push_str(&format!(" AND task_id = ?{}", vals.len() + 1));
            vals.push(Box::new(t.to_string()));
        }
    }
    if let Some(ref project_id) = filter.project_id {
        let resolved = canonical_project_id(conn, Some(project_id))
            .unwrap_or_else(|| project_id.trim().to_string());
        if !resolved.is_empty() {
            sql.push_str(&format!(" AND project_id = ?{}", vals.len() + 1));
            vals.push(Box::new(resolved));
        }
    }
    if let Some(ref action_type) = filter.action_type {
        let a = action_type.trim();
        if !a.is_empty() {
            sql.push_str(&format!(" AND action_type = ?{}", vals.len() + 1));
            vals.push(Box::new(a.to_string()));
        }
    }
    if let Some(ref actor) = filter.actor {
        let a = actor.trim();
        if !a.is_empty() {
            sql.push_str(&format!(" AND actor = ?{}", vals.len() + 1));
            vals.push(Box::new(a.to_string()));
        }
    }
    if let Some(ref since) = filter.since {
        let s = since.trim();
        if !s.is_empty() {
            sql.push_str(&format!(" AND timestamp >= ?{}", vals.len() + 1));
            vals.push(Box::new(s.to_string()));
        }
    }
    if let Some(ref until) = filter.until {
        let u = until.trim();
        if !u.is_empty() {
            sql.push_str(&format!(" AND timestamp <= ?{}", vals.len() + 1));
            vals.push(Box::new(u.to_string()));
        }
    }

    sql.push_str(&format!(" ORDER BY seq ASC LIMIT {limit}"));

    let mut stmt = conn.prepare(&sql)?;
    let param_refs: Vec<&dyn rusqlite::ToSql> = vals.iter().map(|v| v.as_ref()).collect();
    let rows = stmt.query_map(param_refs.as_slice(), map_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn parse_rfc3339_ms(ts: &str) -> Option<DateTime<chrono::FixedOffset>> {
    DateTime::parse_from_rfc3339(ts).ok()
}

fn duration_ms(a: DateTime<chrono::FixedOffset>, b: DateTime<chrono::FixedOffset>) -> u64 {
    (b - a).num_milliseconds().max(0) as u64
}

pub fn compute_approval_wait_ms(events: &[JournalEntry]) -> u64 {
    let mut total = 0u64;
    let mut pending_at: Option<DateTime<chrono::FixedOffset>> = None;
    let last_ts = events.last().and_then(|e| parse_rfc3339_ms(&e.timestamp));

    for e in events {
        match e.action_type.as_str() {
            ACTION_TASK_APPROVAL_PENDING => {
                pending_at = parse_rfc3339_ms(&e.timestamp);
            }
            ACTION_TASK_APPROVAL_APPROVED
            | ACTION_TASK_APPROVAL_REJECTED
            | ACTION_TASK_CANCELLED => {
                if let (Some(start), Some(end)) =
                    (pending_at.take(), parse_rfc3339_ms(&e.timestamp))
                {
                    total = total.saturating_add(duration_ms(start, end));
                }
            }
            _ => {}
        }
    }

    if let (Some(start), Some(end)) = (pending_at, last_ts) {
        total = total.saturating_add(duration_ms(start, end));
    }
    total
}

pub fn compute_total_duration_ms(events: &[JournalEntry]) -> Option<u64> {
    let first = events.first()?;
    let last = events.last()?;
    let a = parse_rfc3339_ms(&first.timestamp)?;
    let b = parse_rfc3339_ms(&last.timestamp)?;
    Some(duration_ms(a, b))
}

pub fn explain_why(conn: &Connection, target_id: &str) -> Result<JournalWhyReport> {
    let target = target_id.trim();
    if target.is_empty() {
        anyhow::bail!("explain_why: target_id gerekli");
    }

    // Prefer event_id → task_id resolution when the id is an event.
    let mut task_id: Option<String> = None;
    let mut project_id: Option<String> = None;

    let events = if let Some(entry) = conn
        .query_row(
            &format!("SELECT {SELECT_COLS} FROM event_journal WHERE event_id = ?1"),
            params![target],
            map_row,
        )
        .optional()?
    {
        if let Some(ref tid) = entry.task_id {
            task_id = Some(tid.clone());
            project_id = entry.project_id.clone();
            query_journal(
                conn,
                &JournalQueryFilter {
                    task_id: Some(tid.clone()),
                    limit: Some(MAX_JOURNAL_QUERY_LIMIT),
                    ..Default::default()
                },
            )?
        } else {
            project_id = entry.project_id.clone();
            vec![entry]
        }
    } else {
        // Treat as task_id.
        let events = query_journal(
            conn,
            &JournalQueryFilter {
                task_id: Some(target.to_string()),
                limit: Some(MAX_JOURNAL_QUERY_LIMIT),
                ..Default::default()
            },
        )?;
        if !events.is_empty() {
            task_id = Some(target.to_string());
            project_id = events.iter().find_map(|e| e.project_id.clone());
        }
        events
    };

    let approval_wait_ms = compute_approval_wait_ms(&events);
    let total_duration_ms = compute_total_duration_ms(&events);
    let execution_ms = total_duration_ms.map(|t| t.saturating_sub(approval_wait_ms));

    // Local chain validity among returned rows + full journal when non-empty.
    let chain_valid = if events.is_empty() {
        true
    } else {
        let full = verify_journal_chain(conn)?;
        full.valid && verify_returned_chain_slice(&events)
    };

    Ok(JournalWhyReport {
        target_id: target.to_string(),
        task_id,
        project_id,
        events,
        total_duration_ms,
        approval_wait_ms,
        execution_ms,
        chain_valid,
    })
}

fn verify_returned_chain_slice(events: &[JournalEntry]) -> bool {
    if events.is_empty() {
        return true;
    }
    for window in events.windows(2) {
        if window[1].seq <= window[0].seq {
            return false;
        }
        // Contiguous within a task slice is not required (other tasks may interleave),
        // but each entry's hash must match recomputation.
    }
    for e in events {
        let preimage = journal_entry_preimage(&JournalHashParts {
            seq: e.seq,
            event_id: &e.event_id,
            timestamp: &e.timestamp,
            actor: &e.actor,
            action_type: &e.action_type,
            project_id: &e.project_id,
            task_id: &e.task_id,
            scrubbed_payload_json: &e.payload_json,
            prev_hash: &e.prev_hash,
        });
        if compute_entry_hash(&preimage) != e.entry_hash {
            return false;
        }
    }
    true
}

/// Map A2A [`TaskStatus`](crate::models::TaskStatus) to a journal action, if any.
pub fn action_for_task_status(status: &crate::models::TaskStatus) -> Option<&'static str> {
    use crate::models::TaskStatus;
    match status {
        TaskStatus::PendingApproval => Some(ACTION_TASK_APPROVAL_PENDING),
        TaskStatus::Completed => Some(ACTION_TASK_COMPLETED),
        TaskStatus::Cancelled => Some(ACTION_TASK_CANCELLED),
        TaskStatus::Expired => Some(ACTION_TASK_EXPIRED_OR_ORPHAN),
        _ => None,
    }
}

pub fn load_task_project_id(conn: &Connection, task_id: &str) -> Option<String> {
    conn.query_row(
        "SELECT project_id FROM a2a_tasks WHERE id = ?1",
        params![task_id],
        |row| row.get::<_, Option<String>>(0),
    )
    .ok()
    .flatten()
}

pub fn journal_task_lifecycle(
    conn: &Connection,
    task_id: &str,
    action_type: &str,
    actor: &str,
    project_id: Option<String>,
    payload: serde_json::Value,
) {
    let project_id = project_id.or_else(|| load_task_project_id(conn, task_id));
    append_journal_best_effort(
        conn,
        JournalAppendInput::new(actor, action_type, payload.to_string())
            .with_task_id(task_id.to_string())
            .with_project_id(project_id),
    );
}

/// RFC3339 timestamp offset by `offset_ms` from a fixed epoch (tests / deterministic hooks).
pub fn timestamp_at_offset_ms(base: &str, offset_ms: i64) -> Result<String> {
    let dt = DateTime::parse_from_rfc3339(base).context("base timestamp")?;
    let shifted = dt + chrono::Duration::milliseconds(offset_ms);
    Ok(shifted
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Millis, true))
}

impl crate::db::ExperienceStore {
    pub fn append_journal_entry(&self, input: JournalAppendInput) -> Result<JournalEntry> {
        let mut conn = self.conn.lock().expect("experience db lock");
        append_journal_entry_immediate(&mut conn, input)
    }

    pub fn verify_journal_chain(&self) -> Result<JournalVerificationReport> {
        let conn = self.conn.lock().expect("experience db lock");
        verify_journal_chain(&conn)
    }

    pub fn query_journal(&self, filter: &JournalQueryFilter) -> Result<Vec<JournalEntry>> {
        let conn = self.conn.lock().expect("experience db lock");
        query_journal(&conn, filter)
    }

    pub fn explain_why(&self, target_id: &str) -> Result<JournalWhyReport> {
        let conn = self.conn.lock().expect("experience db lock");
        explain_why(&conn, target_id)
    }

    /// Test helper: run raw SQL against the shared connection.
    #[cfg(test)]
    pub fn exec_sql_for_tests(&self, sql: &str) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        conn.execute_batch(sql)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ExperienceStore;
    use crate::models::{ExperienceOutcome, ExperienceRecord, LoungeTask, TaskStatus};
    use crate::services::halt::{halt, resume, HALT_BY_UI};

    fn base_ts() -> &'static str {
        "2026-01-01T00:00:00.000Z"
    }

    fn append_at(
        store: &ExperienceStore,
        actor: &str,
        action: &str,
        task_id: &str,
        offset_ms: i64,
        payload: &str,
    ) -> JournalEntry {
        let ts = timestamp_at_offset_ms(base_ts(), offset_ms).unwrap();
        store
            .append_journal_entry(
                JournalAppendInput::new(actor, action, payload)
                    .with_task_id(task_id)
                    .with_project_id(Some("proj-demo".into()))
                    .with_timestamp(ts),
            )
            .unwrap()
    }

    #[test]
    fn genesis_and_chain_continuity() {
        let store = ExperienceStore::memory().unwrap();
        let e1 = store
            .append_journal_entry(JournalAppendInput::new("ui", ACTION_TASK_REQUESTED, "{}"))
            .unwrap();
        assert_eq!(e1.seq, 1);
        assert_eq!(e1.prev_hash, GENESIS_PREV_HASH);

        let e2 = store
            .append_journal_entry(
                JournalAppendInput::new("agent", ACTION_TASK_CLAIMED, r#"{"ok":true}"#)
                    .with_task_id("t1"),
            )
            .unwrap();
        assert_eq!(e2.seq, 2);
        assert_eq!(e2.prev_hash, e1.entry_hash);

        let report = store.verify_journal_chain().unwrap();
        assert!(report.valid, "{report:?}");
        assert_eq!(report.checked_count, 2);
        assert_eq!(report.head_hash.as_deref(), Some(e2.entry_hash.as_str()));
    }

    #[test]
    fn tamper_payload_breaks_at_seq_2() {
        let store = ExperienceStore::memory().unwrap();
        store
            .append_journal_entry(JournalAppendInput::new("a", "x.one", "{}"))
            .unwrap();
        let e2 = store
            .append_journal_entry(JournalAppendInput::new("a", "x.two", r#"{"v":1}"#))
            .unwrap();
        store
            .append_journal_entry(JournalAppendInput::new("a", "x.three", r#"{"v":2}"#))
            .unwrap();

        store
            .exec_sql_for_tests(&format!(
                "UPDATE event_journal SET payload_json = '{}' WHERE seq = 2",
                r#"{"v":"TAMPERED"}"#
            ))
            .unwrap();

        let report = store.verify_journal_chain().unwrap();
        assert!(!report.valid);
        assert_eq!(report.broken_at_seq, Some(2));
        assert_eq!(
            report.broken_event_id.as_deref(),
            Some(e2.event_id.as_str())
        );
    }

    #[test]
    fn tamper_prev_hash_breaks_at_seq_3() {
        let store = ExperienceStore::memory().unwrap();
        store
            .append_journal_entry(JournalAppendInput::new("a", "x.one", "{}"))
            .unwrap();
        store
            .append_journal_entry(JournalAppendInput::new("a", "x.two", "{}"))
            .unwrap();
        let e3 = store
            .append_journal_entry(JournalAppendInput::new("a", "x.three", "{}"))
            .unwrap();

        store
            .exec_sql_for_tests("UPDATE event_journal SET prev_hash = 'deadbeef' WHERE seq = 3")
            .unwrap();

        let report = store.verify_journal_chain().unwrap();
        assert!(!report.valid);
        assert_eq!(report.broken_at_seq, Some(3));
        assert_eq!(
            report.broken_event_id.as_deref(),
            Some(e3.event_id.as_str())
        );
    }

    #[test]
    fn delete_middle_row_breaks_at_seq_3() {
        let store = ExperienceStore::memory().unwrap();
        store
            .append_journal_entry(JournalAppendInput::new("a", "x.one", "{}"))
            .unwrap();
        store
            .append_journal_entry(JournalAppendInput::new("a", "x.two", "{}"))
            .unwrap();
        let e3 = store
            .append_journal_entry(JournalAppendInput::new("a", "x.three", "{}"))
            .unwrap();

        store
            .exec_sql_for_tests("DELETE FROM event_journal WHERE seq = 2")
            .unwrap();

        let report = store.verify_journal_chain().unwrap();
        assert!(!report.valid);
        assert_eq!(report.broken_at_seq, Some(3));
        assert_eq!(
            report.broken_event_id.as_deref(),
            Some(e3.event_id.as_str())
        );
    }

    #[test]
    fn pre_hash_hygiene_scrubs_secrets_ip_and_home() {
        let store = ExperienceStore::memory().unwrap();
        let secret = "sk-proj-1234567890abcdef1234567890abcdef";
        let payload = format!(
            r#"{{"token":"{secret}","endpoint":"192.168.1.42:8080","path":"/Users/alice/secret/repo"}}"#
        );
        let entry = store
            .append_journal_entry(JournalAppendInput::new("tester", "hygiene.check", payload))
            .unwrap();

        assert!(
            entry.payload_json.contains("<redacted-token>"),
            "token scrub missing: {}",
            entry.payload_json
        );
        assert!(
            entry.payload_json.contains("<device-ip>:8080"),
            "ip scrub missing: {}",
            entry.payload_json
        );
        assert!(
            entry.payload_json.contains("<home>"),
            "home scrub missing: {}",
            entry.payload_json
        );
        assert!(!entry.payload_json.contains(secret));
        assert!(!entry.payload_json.contains("192.168.1.42"));
        assert!(!entry.payload_json.contains("/Users/alice"));
        assert!(!entry.entry_hash.contains(secret));

        // Raw dump of table must not contain secrets.
        let dump: String = {
            let conn = store.conn.lock().expect("lock");
            let mut stmt = conn
                .prepare(
                    "SELECT payload_json || '|' || entry_hash || '|' || actor FROM event_journal",
                )
                .unwrap();
            let mut rows = stmt.query([]).unwrap();
            let mut out = String::new();
            while let Some(row) = rows.next().unwrap() {
                let s: String = row.get(0).unwrap();
                out.push_str(&s);
            }
            out
        };
        assert!(!dump.contains(secret));
        assert!(!dump.contains("192.168.1.42"));
        assert!(!dump.contains("/Users/alice"));

        let report = store.verify_journal_chain().unwrap();
        assert!(report.valid, "{report:?}");
    }

    #[test]
    fn explain_why_approval_wait_metrics() {
        let store = ExperienceStore::memory().unwrap();
        let task_id = "task-why-1";
        append_at(&store, "mcp", ACTION_TASK_REQUESTED, task_id, 0, "{}");
        append_at(
            &store,
            "kernel",
            ACTION_TASK_APPROVAL_PENDING,
            task_id,
            1000,
            "{}",
        );
        append_at(
            &store,
            "ui",
            ACTION_TASK_APPROVAL_APPROVED,
            task_id,
            4500,
            "{}",
        );
        append_at(&store, "agent", ACTION_TASK_COMPLETED, task_id, 6000, "{}");

        let why = store.explain_why(task_id).unwrap();
        assert_eq!(why.events.len(), 4);
        assert_eq!(why.approval_wait_ms, 3500);
        assert_eq!(why.total_duration_ms, Some(6000));
        assert_eq!(why.execution_ms, Some(2500));
        assert!(why.chain_valid);
        assert_eq!(why.task_id.as_deref(), Some(task_id));
    }

    #[tokio::test]
    async fn automatic_hooks_a2a_experience_halt() {
        let store = ExperienceStore::memory().unwrap();

        // A2A admit → task.requested
        let mut task = LoungeTask::new("cursor", "agent-lounge-os", "journal hook");
        task.project_id = "proj-hooks".into();
        let outcome = store.admit_a2a_task(&mut task, 8).unwrap();
        assert!(matches!(outcome, crate::db::AdmitOutcome::Accepted(_)));

        // claim → task.claimed
        assert!(store.claim_a2a_task(&task.id, "sess-1").unwrap());

        // approval pending via status
        store
            .set_a2a_task_status(&task.id, TaskStatus::PendingApproval)
            .unwrap();

        // complete
        store
            .complete_a2a_with_result(
                &task.id,
                TaskStatus::Completed,
                Some(r#"{"ok":true}"#),
                false,
            )
            .unwrap();

        // experience lifecycle
        let record = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj-hooks", "journal topic"),
            "journal solution",
            "adr",
            ExperienceOutcome::Success,
            vec![],
        );
        let exp_id = record.id.clone();
        store.insert_record(record).await.unwrap();
        store
            .mark_experience_reviewed(exp_id.clone())
            .await
            .unwrap();
        store.archive_experience(exp_id.clone()).await.unwrap();
        store.unarchive_experience(exp_id.clone()).await.unwrap();
        store.reject_experience(exp_id.clone()).await.unwrap();
        store.undo_reject_experience(exp_id.clone()).await.unwrap();

        // halt / resume
        halt(&store, Some("maintenance".into()), HALT_BY_UI)
            .await
            .unwrap();
        resume(&store, HALT_BY_UI).await.unwrap();

        let actions: Vec<String> = store
            .query_journal(&JournalQueryFilter {
                limit: Some(1000),
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .map(|e| e.action_type)
            .collect();

        for expected in [
            ACTION_TASK_REQUESTED,
            ACTION_TASK_CLAIMED,
            ACTION_TASK_APPROVAL_PENDING,
            ACTION_TASK_COMPLETED,
            ACTION_EXPERIENCE_RECORDED,
            ACTION_EXPERIENCE_REVIEWED,
            ACTION_EXPERIENCE_ARCHIVED,
            ACTION_EXPERIENCE_UNARCHIVED,
            ACTION_EXPERIENCE_REJECTED,
            ACTION_EXPERIENCE_UNDO_REJECTED,
            ACTION_CONTROL_HALTED,
            ACTION_CONTROL_RESUMED,
        ] {
            assert!(
                actions.iter().any(|a| a == expected),
                "missing {expected} in {actions:?}"
            );
        }

        let report = store.verify_journal_chain().unwrap();
        assert!(report.valid, "{report:?}");
        assert!(report.checked_count >= 12);
    }

    #[test]
    fn query_journal_filters_and_limit_cap() {
        let store = ExperienceStore::memory().unwrap();
        for i in 0..5 {
            store
                .append_journal_entry(
                    JournalAppendInput::new("actor-a", "demo.action", "{}")
                        .with_task_id(format!("t-{i}"))
                        .with_project_id(Some("p1".into())),
                )
                .unwrap();
        }
        store
            .append_journal_entry(
                JournalAppendInput::new("actor-b", "other.action", "{}").with_task_id("t-x"),
            )
            .unwrap();

        let filtered = store
            .query_journal(&JournalQueryFilter {
                actor: Some("actor-a".into()),
                action_type: Some("demo.action".into()),
                limit: Some(2),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(filtered.len(), 2);
        assert!(filtered.iter().all(|e| e.actor == "actor-a"));

        let capped = store
            .query_journal(&JournalQueryFilter {
                limit: Some(50_000),
                ..Default::default()
            })
            .unwrap();
        assert!(capped.len() <= MAX_JOURNAL_QUERY_LIMIT);
        assert_eq!(capped.len(), 6);
    }

    #[test]
    fn migrate_is_idempotent() {
        let store = ExperienceStore::memory().unwrap();
        {
            let conn = store.conn.lock().expect("lock");
            migrate_journal(&conn).unwrap();
            migrate_journal(&conn).unwrap();
        }
        store
            .append_journal_entry(JournalAppendInput::new("a", "x", "{}"))
            .unwrap();
        assert!(store.verify_journal_chain().unwrap().valid);
    }
}
