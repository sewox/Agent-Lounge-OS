//! Experience CRUD beyond insert/search: update, archive, pin, reviewed, usage, auto-archive.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::cmp::Ordering;

use super::experiences::{column_names, has_col};
use super::ExperienceStore;
use crate::models::{
    now_rfc3339, ExperienceRecord, LoungeExperience, TitleSource, ARCHIVED_BY_REJECTED,
    ARCHIVED_BY_TTL, ARCHIVED_BY_USER, EXPERIENCE_STATUS_ACTIVE, EXPERIENCE_STATUS_ARCHIVED,
};

/// Settings key for auto-archive TTL in days (default 90).
pub const SETTING_EXPERIENCE_TTL_DAYS: &str = "experience_ttl_days";
pub const DEFAULT_EXPERIENCE_TTL_DAYS: u64 = 90;

/// UD-5 / VR4: score **bucket** width for search ranking.
/// Bucket = `floor(normalize(score) / REVIEW_SCORE_BAND)`. Hits in the **same**
/// bucket are ordered by `reviewed` → score → `created_at` → `id`. Hits in
/// different buckets are ordered by bucket (higher score band first) — a total
/// order. Documented in `docs/mcp.md`.
pub const REVIEW_SCORE_BAND: f64 = 0.05;

/// Clamp non-finite / negative scores so bucket math and `partial_cmp` stay total.
pub fn normalize_search_score(score: f64) -> f64 {
    if !score.is_finite() || score < 0.0 {
        0.0
    } else {
        score
    }
}

/// Discrete relevance band: `floor(normalize(score) / REVIEW_SCORE_BAND)`.
pub fn review_score_bucket(score: f64) -> i64 {
    let s = normalize_search_score(score);
    (s / REVIEW_SCORE_BAND).floor() as i64
}

/// Normalized [0, 1] lexical relevance for LIKE-search hits (primary sort key).
pub fn lexical_search_score(query: &str, row: &LoungeExperience) -> f64 {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return if row.is_pinned { 0.05 } else { 0.0 };
    }
    let title = row.title.to_lowercase();
    let adr = row.adr_summary.to_lowercase();
    let tags = row.tags.join(" ").to_lowercase();
    let mut score = 0.0_f64;
    if !title.is_empty() && title == q {
        score = 1.0;
    } else if title.contains(&q) {
        score = 0.9;
    } else if adr.contains(&q) {
        let ratio = (q.len() as f64) / (adr.len().max(1) as f64);
        score = 0.55 + 0.25 * ratio.min(1.0);
    } else if tags.contains(&q) {
        score = 0.3;
    } else {
        let tokens: Vec<_> = q.split_whitespace().filter(|t| t.len() > 1).collect();
        let hay = format!("{title} {adr} {tags}");
        if !tokens.is_empty() {
            let hits = tokens.iter().filter(|t| hay.contains(**t)).count();
            score = 0.2 * (hits as f64 / tokens.len() as f64);
        }
    }
    if row.is_pinned {
        score = (score + 0.02).min(1.0);
    }
    score
}

/// One side of [`cmp_search_with_review_band`] (keeps the comparator ≤7 args for clippy).
#[derive(Clone, Copy, Debug)]
pub struct SearchRankKey<'a> {
    /// Hard same-project tier (main behavior): prefer caller's project above buckets.
    pub same_project: bool,
    pub score: f64,
    pub reviewed: bool,
    pub created_at: &'a str,
    pub id: &'a str,
}

/// Total order for MCP/search hits (never panics under `sort_by` since Rust 1.81):
/// same_project → bucket desc → reviewed → score desc → created_at desc → id asc.
pub fn cmp_search_with_review_band(a: SearchRankKey<'_>, b: SearchRankKey<'_>) -> Ordering {
    let sa = normalize_search_score(a.score);
    let sb = normalize_search_score(b.score);
    let ba = review_score_bucket(sa);
    let bb = review_score_bucket(sb);
    b.same_project
        .cmp(&a.same_project)
        .then_with(|| bb.cmp(&ba))
        .then_with(|| b.reviewed.cmp(&a.reviewed))
        .then_with(|| sb.partial_cmp(&sa).unwrap_or(Ordering::Equal))
        .then_with(|| b.created_at.cmp(a.created_at))
        .then_with(|| a.id.cmp(b.id))
}

/// Sort search hits: hard same-project first (main), then hybrid score buckets / reviewed.
///
/// Score = [`crate::db::vault_fts::hybrid_search_score`]: `0.7 * semantic + 0.3 * lexical`
/// where lexical is the LIKE-era [`lexical_search_score`] and semantic is cosine
/// similarity of hashed bag-of-words embeddings over title+ADR+tags. When the
/// embedding cosine is unavailable the hybrid helper treats semantic as `0.0`.
pub fn sort_experiences_for_search(
    hits: &mut [LoungeExperience],
    query: &str,
    prefer_project: Option<&str>,
) {
    let query_emb = super::embedding::lexical_embedding(query);
    hits.sort_by(|a, b| {
        let sa = hybrid_score_for_lounge(query, &query_emb, a);
        let sb = hybrid_score_for_lounge(query, &query_emb, b);
        let same_a = prefer_project
            .map(|pid| a.project_id == pid)
            .unwrap_or(false);
        let same_b = prefer_project
            .map(|pid| b.project_id == pid)
            .unwrap_or(false);
        cmp_search_with_review_band(
            SearchRankKey {
                same_project: same_a,
                score: sa,
                reviewed: a.reviewed,
                created_at: &a.created_at,
                id: &a.id,
            },
            SearchRankKey {
                same_project: same_b,
                score: sb,
                reviewed: b.reviewed,
                created_at: &b.created_at,
                id: &b.id,
            },
        )
    });
}

fn hybrid_score_for_lounge(query: &str, query_emb: &[f32], row: &LoungeExperience) -> f64 {
    let lexical = lexical_search_score(query, row);
    let doc = format!("{} {} {}", row.title, row.adr_summary, row.tags.join(" "));
    let semantic = if doc.trim().is_empty() && query.trim().is_empty() {
        None
    } else {
        let emb = super::embedding::lexical_embedding(&doc);
        super::embedding::cosine_similarity(query_emb, &emb).map(|s| s as f64)
    };
    super::vault_fts::hybrid_search_score(semantic, lexical)
}

/// UI IPC must never expose pre-hygiene `original_content` (B2). MCP paths are unchanged.
pub fn strip_original_content_for_ui(mut row: LoungeExperience) -> LoungeExperience {
    row.original_content = None;
    row
}

pub fn strip_original_content_for_ui_vec(rows: Vec<LoungeExperience>) -> Vec<LoungeExperience> {
    rows.into_iter()
        .map(strip_original_content_for_ui)
        .collect()
}
/// Optional use-count threshold for auto-archive (0 = disabled).
pub const SETTING_EXPERIENCE_USE_COUNT_THRESHOLD: &str = "experience_use_count_threshold";
pub const DEFAULT_EXPERIENCE_USE_COUNT_THRESHOLD: u64 = 0;
/// One-time soft-hide of empty placeholder project_index rows.
pub const MIGRATION_PLACEHOLDER_CLEANUP_V1: &str = "migrations.placeholder_cleanup_v1";
/// One-time transactional MCP-Quality migration (title/source columns + ADR cleanup).
pub const MIGRATION_MCP_QUALITY_V1: &str = "migrations.mcp_quality_v1";
/// One-time: enqueue title generation for existing untitled rows (runtime, not DDL).
pub const MIGRATION_TITLE_BACKFILL_ENQUEUE_V1: &str = "migrations.title_backfill_enqueue_v1";

const PLACEHOLDER_NAMES: &[&str] = &["backend-legacy", "frontend-new"];

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ExperienceUpdate {
    pub adr_summary: Option<String>,
    pub tags: Option<Vec<String>>,
    pub outcome: Option<crate::models::ExperienceOutcome>,
    pub project_id: Option<String>,
    pub topic: Option<String>,
    pub title: Option<String>,
}

/// Vault Review StatsStrip / list filter key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatFilter {
    #[default]
    Pending,
    Hygiene,
    ApprovedToday,
    Rejected,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ReviewQueueFilter {
    /// StatsStrip selection; default = pending (active unreviewed).
    #[serde(default)]
    pub stat: ReviewStatFilter,
    pub project_id: Option<String>,
    pub agent: Option<String>,
    /// Experience outcome / type filter (`success` | `failure` | `partial`).
    pub outcome: Option<String>,
    /// When true, further restrict pending/hygiene to hygiene-masked rows.
    #[serde(default)]
    pub hygiene_only: bool,
    #[serde(default = "default_review_limit")]
    pub limit: usize,
}

fn default_review_limit() -> usize {
    200
}

impl Default for ReviewQueueFilter {
    fn default() -> Self {
        Self {
            stat: ReviewStatFilter::Pending,
            project_id: None,
            agent: None,
            outcome: None,
            hygiene_only: false,
            limit: default_review_limit(),
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ReviewStats {
    /// Same source as [`ExperienceStore::count_unreviewed_experiences`].
    pub pending: u64,
    pub hygiene: u64,
    pub approved_today: u64,
    pub rejected: u64,
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
            let now = now_rfc3339();
            conn.execute(
                "UPDATE experiences SET reviewed = 1, reviewed_at = COALESCE(reviewed_at, ?1), updated_at = ?1 WHERE id = ?2",
                params![now, id],
            )?;
            Ok(())
        })
        .await
        .context("experience mark_reviewed join")?
    }

    /// Mark every active unreviewed row reviewed (not just a client-loaded page).
    pub async fn mark_all_experiences_reviewed(&self) -> Result<u64> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            let now = now_rfc3339();
            let n = conn.execute(
                "UPDATE experiences SET reviewed = 1, reviewed_at = COALESCE(reviewed_at, ?1), updated_at = ?1 WHERE reviewed = 0 AND status = ?2",
                params![now, EXPERIENCE_STATUS_ACTIVE],
            )?;
            Ok(n as u64)
        })
        .await
        .context("experience mark_all_reviewed join")?
    }

    /// Bulk-approve selected active rows (Vault Review). Local user action — Halt-safe.
    pub async fn mark_experiences_reviewed(&self, ids: Vec<String>) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            let now = now_rfc3339();
            let mut n = 0u64;
            for id in ids {
                let changed = conn.execute(
                    "UPDATE experiences SET reviewed = 1, reviewed_at = COALESCE(reviewed_at, ?1), updated_at = ?1 WHERE id = ?2 AND reviewed = 0 AND status = ?3",
                    params![now, id, EXPERIENCE_STATUS_ACTIVE],
                )?;
                n += changed as u64;
            }
            Ok(n)
        })
        .await
        .context("experience mark_many_reviewed join")?
    }

    /// Soft-reject = archived with `archived_by=rejected` (undoable). No hard delete.
    pub async fn reject_experience(&self, id: String) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            reject_blocking(&conn, &id)
        })
        .await
        .context("experience reject join")?
    }

    pub async fn reject_experiences(&self, ids: Vec<String>) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            let mut n = 0u64;
            for id in ids {
                reject_blocking(&conn, &id)?;
                n += 1;
            }
            Ok(n)
        })
        .await
        .context("experience reject_many join")?
    }

    /// Undo a soft-reject (only rows with `archived_by=rejected`).
    pub async fn undo_reject_experience(&self, id: String) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            undo_reject_blocking(&conn, &id)
        })
        .await
        .context("experience undo_reject join")?
    }

    /// Vault Review list: default = active unreviewed; hygiene flagged first, then newest.
    pub async fn list_review_queue(
        &self,
        filter: ReviewQueueFilter,
    ) -> Result<Vec<LoungeExperience>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            list_review_queue_blocking(&conn, &filter)
        })
        .await
        .context("experience list_review_queue join")?
    }

    /// StatsStrip counters for Vault Review (pending uses the same SQL as
    /// [`Self::count_unreviewed_experiences`]).
    pub async fn count_review_stats(&self) -> Result<ReviewStats> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            count_review_stats_blocking(&conn)
        })
        .await
        .context("experience count_review_stats join")?
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

    pub async fn experience_use_count_threshold(&self) -> Result<u64> {
        match self
            .get_setting(SETTING_EXPERIENCE_USE_COUNT_THRESHOLD.into())
            .await?
        {
            Some(raw) => Ok(raw.trim().parse::<u64>().unwrap_or(0)),
            None => Ok(DEFAULT_EXPERIENCE_USE_COUNT_THRESHOLD),
        }
    }

    pub async fn set_experience_use_count_threshold(&self, threshold: u64) -> Result<()> {
        self.set_setting(
            SETTING_EXPERIENCE_USE_COUNT_THRESHOLD.into(),
            threshold.to_string(),
        )
        .await
    }

    /// Archive active, unpinned rows unused for `ttl_days` (based on last_used_at or created_at).
    pub async fn auto_archive_stale(
        &self,
        ttl_days: u64,
        use_count_threshold: u64,
        clock: &dyn ArchiveClock,
    ) -> Result<u64> {
        let now = clock.now_rfc3339();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            auto_archive_blocking(&conn, ttl_days, use_count_threshold, &now)
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
        // User edits ADR only — do not re-copy into solution_summary.
        record.adr_record = adr.clone();
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
    if let Some(title) = &patch.title {
        let clipped: String = title.chars().take(crate::models::TITLE_MAX_CHARS).collect();
        record.title = clipped;
        record.title_source = TitleSource::User;
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
            payload_json = ?9,
            title = ?10,
            title_source = ?11
        WHERE id = ?12
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
            record.title,
            record.title_source.as_str(),
            id,
        ],
    )?;
    Ok(())
}

fn set_archive_blocking(conn: &Connection, id: &str, archive: bool) -> Result<()> {
    let now = now_rfc3339();
    if archive {
        conn.execute(
            "UPDATE experiences SET status = ?1, archived_at = ?2, archived_by = ?3, updated_at = ?2 WHERE id = ?4",
            params![EXPERIENCE_STATUS_ARCHIVED, now, ARCHIVED_BY_USER, id],
        )?;
    } else {
        conn.execute(
            "UPDATE experiences SET status = ?1, archived_at = NULL, archived_by = NULL, updated_at = ?2 WHERE id = ?3",
            params![EXPERIENCE_STATUS_ACTIVE, now, id],
        )?;
    }
    Ok(())
}

fn reject_blocking(conn: &Connection, id: &str) -> Result<()> {
    let now = now_rfc3339();
    conn.execute(
        "UPDATE experiences SET status = ?1, archived_at = ?2, archived_by = ?3, updated_at = ?2 WHERE id = ?4",
        params![EXPERIENCE_STATUS_ARCHIVED, now, ARCHIVED_BY_REJECTED, id],
    )?;
    Ok(())
}

fn undo_reject_blocking(conn: &Connection, id: &str) -> Result<()> {
    let now = now_rfc3339();
    let n = conn.execute(
        "UPDATE experiences SET status = ?1, archived_at = NULL, archived_by = NULL, updated_at = ?2 WHERE id = ?3 AND archived_by = ?4",
        params![EXPERIENCE_STATUS_ACTIVE, now, id, ARCHIVED_BY_REJECTED],
    )?;
    if n == 0 {
        anyhow::bail!("undo_reject: row not found or not rejected");
    }
    Ok(())
}

fn today_prefix_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

fn count_review_stats_blocking(conn: &Connection) -> Result<ReviewStats> {
    let pending: i64 = conn.query_row(
        "SELECT COUNT(*) FROM experiences WHERE reviewed = 0 AND status = ?1",
        params![EXPERIENCE_STATUS_ACTIVE],
        |row| row.get(0),
    )?;
    let hygiene_pred = crate::bridge::hygiene::hygiene_masked_sql("");
    let hygiene_sql = format!(
        "SELECT COUNT(*) FROM experiences WHERE reviewed = 0 AND status = ?1 AND {hygiene_pred}"
    );
    let hygiene: i64 = conn.query_row(&hygiene_sql, params![EXPERIENCE_STATUS_ACTIVE], |row| {
        row.get(0)
    })?;
    let today = today_prefix_utc();
    let approved_today: i64 = conn.query_row(
        "SELECT COUNT(*) FROM experiences WHERE reviewed = 1 AND status = ?1 AND reviewed_at IS NOT NULL AND reviewed_at LIKE ?2",
        params![EXPERIENCE_STATUS_ACTIVE, format!("{today}%")],
        |row| row.get(0),
    )?;
    let rejected: i64 = conn.query_row(
        "SELECT COUNT(*) FROM experiences WHERE status = ?1 AND archived_by = ?2",
        params![EXPERIENCE_STATUS_ARCHIVED, ARCHIVED_BY_REJECTED],
        |row| row.get(0),
    )?;
    Ok(ReviewStats {
        pending: pending.max(0) as u64,
        hygiene: hygiene.max(0) as u64,
        approved_today: approved_today.max(0) as u64,
        rejected: rejected.max(0) as u64,
    })
}

fn list_review_queue_blocking(
    conn: &Connection,
    filter: &ReviewQueueFilter,
) -> Result<Vec<LoungeExperience>> {
    let hygiene_pred = crate::bridge::hygiene::hygiene_masked_sql("");
    let lim = filter.limit.max(1) as i64;
    let today = today_prefix_utc();
    list_review_queue_positional(conn, filter, &hygiene_pred, &today, lim)
}

fn list_review_queue_positional(
    conn: &Connection,
    filter: &ReviewQueueFilter,
    hygiene_pred: &str,
    today: &str,
    lim: i64,
) -> Result<Vec<LoungeExperience>> {
    let mut clauses: Vec<String> = Vec::new();
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    match filter.stat {
        ReviewStatFilter::Pending => {
            clauses.push("status = ?".into());
            params_vec.push(Box::new(EXPERIENCE_STATUS_ACTIVE.to_string()));
            clauses.push("reviewed = 0".into());
        }
        ReviewStatFilter::Hygiene => {
            clauses.push("status = ?".into());
            params_vec.push(Box::new(EXPERIENCE_STATUS_ACTIVE.to_string()));
            clauses.push("reviewed = 0".into());
            clauses.push(format!("({hygiene_pred})"));
        }
        ReviewStatFilter::ApprovedToday => {
            clauses.push("status = ?".into());
            params_vec.push(Box::new(EXPERIENCE_STATUS_ACTIVE.to_string()));
            clauses.push("reviewed = 1".into());
            clauses.push("reviewed_at IS NOT NULL".into());
            clauses.push("reviewed_at LIKE ?".into());
            params_vec.push(Box::new(format!("{today}%")));
        }
        ReviewStatFilter::Rejected => {
            clauses.push("status = ?".into());
            params_vec.push(Box::new(EXPERIENCE_STATUS_ARCHIVED.to_string()));
            clauses.push("archived_by = ?".into());
            params_vec.push(Box::new(ARCHIVED_BY_REJECTED.to_string()));
        }
    }

    if let Some(ref project) = filter.project_id {
        let p = project.trim();
        if !p.is_empty() {
            clauses.push("project_id = ?".into());
            params_vec.push(Box::new(p.to_string()));
        }
    }
    if let Some(ref agent) = filter.agent {
        let a = agent.trim();
        if !a.is_empty() {
            clauses.push("agent_id = ?".into());
            params_vec.push(Box::new(a.to_string()));
        }
    }
    if let Some(ref outcome) = filter.outcome {
        let o = outcome.trim();
        if !o.is_empty() {
            clauses.push("lower(outcome) = lower(?)".into());
            params_vec.push(Box::new(o.to_string()));
        }
    }
    if filter.hygiene_only
        && matches!(
            filter.stat,
            ReviewStatFilter::Pending | ReviewStatFilter::Hygiene
        )
    {
        clauses.push(format!("({hygiene_pred})"));
    }

    let order = match filter.stat {
        ReviewStatFilter::Pending => {
            format!("CASE WHEN ({hygiene_pred}) THEN 0 ELSE 1 END ASC, created_at DESC")
        }
        ReviewStatFilter::Hygiene => "created_at DESC".into(),
        ReviewStatFilter::ApprovedToday => "reviewed_at DESC, created_at DESC".into(),
        ReviewStatFilter::Rejected => "archived_at DESC, created_at DESC".into(),
    };

    let sql = format!(
        r#"
        SELECT id, project_id, agent_id, topic, solution_summary, adr_record,
               outcome, related_task_id, tags_json, created_at, embedding, payload_json,
               status, reviewed, use_count, last_used_at, archived_at, is_pinned,
               updated_at, original_content, archived_by, title, title_source, source
        FROM experiences
        WHERE {}
        ORDER BY {order}
        LIMIT ?
        "#,
        clauses.join(" AND ")
    );
    params_vec.push(Box::new(lim));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> =
        params_vec.iter().map(|p| p.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let mapped = stmt.query_map(param_refs.as_slice(), map_lounge_row)?;
    let mut out = Vec::new();
    for row in mapped {
        out.push(row?);
    }
    Ok(out)
}

fn get_full_record(conn: &Connection, id: &str) -> Result<Option<ExperienceRecord>> {
    let row = conn
        .query_row(
            r#"
            SELECT id, project_id, agent_id, topic, solution_summary, adr_record,
                   outcome, related_task_id, tags_json, created_at, embedding, payload_json,
                   status, reviewed, use_count, last_used_at, archived_at, is_pinned,
                   updated_at, original_content, archived_by, title, title_source, source
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
    let title = row.get::<_, Option<String>>(21)?.unwrap_or_default();
    let title_source = row.get::<_, Option<String>>(22)?.unwrap_or_default();
    let source = row.get::<_, Option<String>>(23)?.unwrap_or_default();
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
        archived_by: row.get(20)?,
        title,
        title_source: TitleSource::parse(&title_source),
        source,
    })
}

fn search_filtered_blocking(
    conn: &Connection,
    needle: &str,
    limit: usize,
    status: Option<&str>,
) -> Result<Vec<crate::models::LoungeExperience>> {
    let trimmed = needle.trim();
    if trimmed.is_empty() {
        // Empty query → pinned/newest window (no FTS).
        let fetch_cap = (limit.saturating_mul(4)).clamp(limit, 256);
        let status_clause = if status.is_some() {
            "WHERE status = ?1"
        } else {
            ""
        };
        let sql = format!(
            r#"
            SELECT id, project_id, agent_id, topic, solution_summary, adr_record,
                   outcome, related_task_id, tags_json, created_at, embedding, payload_json,
                   status, reviewed, use_count, last_used_at, archived_at, is_pinned,
                   updated_at, original_content, archived_by, title, title_source, source
            FROM experiences
            {status_clause}
            ORDER BY is_pinned DESC, created_at DESC
            LIMIT ?{limit_idx}
            "#,
            limit_idx = if status.is_some() { 2 } else { 1 }
        );
        let mut stmt = conn.prepare(&sql)?;
        let mapped = if let Some(status) = status {
            stmt.query_map(params![status, fetch_cap as i64], map_lounge_row)?
        } else {
            stmt.query_map(params![fetch_cap as i64], map_lounge_row)?
        };
        let mut out = Vec::new();
        for row in mapped {
            out.push(row?);
        }
        out.truncate(limit);
        return Ok(out);
    }

    // Vault-1: active rows via FTS5. Archived rows are excluded from the FTS
    // index by design — O2 archive fallback uses folded LIKE instead.
    if status == Some(EXPERIENCE_STATUS_ARCHIVED) {
        return search_archived_folded_like(conn, trimmed, limit);
    }
    super::vault_fts::search_experiences_fts(conn, trimmed, limit, status)
}

/// Archive-only search (O2). Archived experiences are not in `vault_fts`.
fn search_archived_folded_like(
    conn: &Connection,
    needle: &str,
    limit: usize,
) -> Result<Vec<crate::models::LoungeExperience>> {
    super::vault_fts::register_turkish_fold_fn(conn);
    let folded = super::vault_fts::turkish_fold(needle);
    let fetch_cap = (limit.saturating_mul(4)).clamp(limit, 256);
    let sql = r#"
        SELECT id, project_id, agent_id, topic, solution_summary, adr_record,
               outcome, related_task_id, tags_json, created_at, embedding, payload_json,
               status, reviewed, use_count, last_used_at, archived_at, is_pinned,
               updated_at, original_content, archived_by, title, title_source, source
        FROM experiences
        WHERE status = ?1
          AND (
            al_turkish_fold(COALESCE(title, '')) LIKE '%' || ?2 || '%'
            OR al_turkish_fold(COALESCE(tags_json, '')) LIKE '%' || ?2 || '%'
            OR al_turkish_fold(COALESCE(source, '')) LIKE '%' || ?2 || '%'
            OR al_turkish_fold(COALESCE(adr_record, '')) LIKE '%' || ?2 || '%'
            OR al_turkish_fold(COALESCE(topic, '') || ' ' || COALESCE(solution_summary, ''))
                 LIKE '%' || ?2 || '%'
          )
        ORDER BY is_pinned DESC, created_at DESC
        LIMIT ?3
        "#;
    let mut stmt = conn.prepare(sql)?;
    let mapped = stmt.query_map(
        params![EXPERIENCE_STATUS_ARCHIVED, folded, fetch_cap as i64],
        map_lounge_row,
    )?;
    let mut out = Vec::new();
    for row in mapped {
        out.push(row?);
    }
    sort_experiences_for_search(&mut out, needle, None);
    out.truncate(limit);
    Ok(out)
}

pub(crate) fn map_lounge_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<crate::models::LoungeExperience> {
    Ok(map_full_record(row)?.to_lounge())
}

fn auto_archive_blocking(
    conn: &Connection,
    ttl_days: u64,
    use_count_threshold: u64,
    now_rfc: &str,
) -> Result<u64> {
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
        SELECT id, COALESCE(last_used_at, created_at), is_pinned, use_count
        FROM experiences
        WHERE status = ?1 AND is_pinned = 0
        "#,
    )?;
    let rows = stmt.query_map(params![EXPERIENCE_STATUS_ACTIVE], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, i64>(3)?,
        ))
    })?;

    let mut ids = Vec::new();
    for row in rows {
        let (id, anchor, pinned, use_count) = row?;
        if pinned != 0 {
            continue;
        }
        if use_count_threshold > 0 && use_count.max(0) as u64 > use_count_threshold {
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
            "UPDATE experiences SET status = ?1, archived_at = ?2, archived_by = ?3, updated_at = ?2 WHERE id = ?4",
            params![EXPERIENCE_STATUS_ARCHIVED, archived_at, ARCHIVED_BY_TTL, id],
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
        ("reviewed_at", "TEXT"),
        ("use_count", "INTEGER NOT NULL DEFAULT 0"),
        ("last_used_at", "TEXT"),
        ("archived_at", "TEXT"),
        ("is_pinned", "INTEGER NOT NULL DEFAULT 0"),
        ("updated_at", "TEXT"),
        ("original_content", "TEXT"),
        ("archived_by", "TEXT"),
    ];
    let mut added_reviewed = false;
    let mut cols = cols;
    for (name, decl) in additions {
        if !has_col(&cols, name) {
            conn.execute(
                &format!("ALTER TABLE experiences ADD COLUMN {name} {decl}"),
                [],
            )?;
            cols.push((*name).to_string());
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
        "UPDATE experiences SET original_content = COALESCE(original_content, solution_summary, adr_record) WHERE original_content IS NULL OR original_content = ''",
        [],
    )?;
    conn.execute(
        "UPDATE experiences SET updated_at = COALESCE(updated_at, created_at) WHERE updated_at IS NULL OR updated_at = ''",
        [],
    )?;
    migrate_mcp_quality_v1(conn)?;
    conn.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_experiences_status ON experiences(status);
        CREATE INDEX IF NOT EXISTS idx_experiences_reviewed ON experiences(reviewed);
        CREATE INDEX IF NOT EXISTS idx_experiences_pinned ON experiences(is_pinned);
        "#,
    )?;
    Ok(())
}

/// MCP-Quality-1 migration in a **single transaction**:
/// ALTER (title/source/retry cols) + title backfill + ADR cleanup + done flag.
/// All commit together or all roll back. Requires an existing `settings` table
/// (created by `migrate_schema` / test fixtures — not created here).
pub fn migrate_mcp_quality_v1(conn: &Connection) -> Result<()> {
    migrate_mcp_quality_v1_impl(conn, None)
}

/// `fail_before`: test hook — abort before this step name so the transaction rolls back.
/// Steps: `after_alter` | `after_title_backfill` | `after_adr_clear` | `before_done_flag`.
fn migrate_mcp_quality_v1_impl(conn: &Connection, fail_before: Option<&str>) -> Result<()> {
    let done: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1",
            params![MIGRATION_MCP_QUALITY_V1],
            |row| row.get(0),
        )
        .optional()?;
    if matches!(done.as_deref(), Some("1") | Some("true") | Some("\"1\"")) {
        return Ok(());
    }

    let tx = conn.unchecked_transaction()?;
    let cols = column_names(&tx, "experiences")?;
    let quality_cols: &[(&str, &str)] = &[
        ("title", "TEXT NOT NULL DEFAULT ''"),
        ("title_source", "TEXT NOT NULL DEFAULT 'fallback'"),
        ("source", "TEXT NOT NULL DEFAULT ''"),
        ("title_attempts", "INTEGER NOT NULL DEFAULT 0"),
        ("title_next_retry_at", "TEXT"),
        ("title_give_up", "INTEGER NOT NULL DEFAULT 0"),
    ];
    for (name, decl) in quality_cols {
        if !has_col(&cols, name) {
            tx.execute(
                &format!("ALTER TABLE experiences ADD COLUMN {name} {decl}"),
                [],
            )?;
        }
    }
    if fail_before == Some("after_alter") {
        anyhow::bail!("forced failure after_alter");
    }

    // Word-boundary title backfill (not raw substr).
    {
        let mut stmt = tx.prepare(
            r#"
            SELECT id, COALESCE(NULLIF(trim(solution_summary), ''), topic)
            FROM experiences
            WHERE trim(COALESCE(title, '')) = ''
            "#,
        )?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        for (id, content) in rows {
            let title =
                crate::bridge::hygiene::fallback_title(&content, crate::models::TITLE_MAX_CHARS);
            tx.execute(
                "UPDATE experiences SET title = ?1, title_source = 'fallback' WHERE id = ?2",
                params![title, id],
            )?;
        }
    }
    if fail_before == Some("after_title_backfill") {
        anyhow::bail!("forced failure after_title_backfill");
    }

    // Approved: clear adr_record where it was a duplicate of solution_summary.
    tx.execute(
        "UPDATE experiences SET adr_record = '' WHERE adr_record = solution_summary",
        [],
    )?;
    if fail_before == Some("after_adr_clear") {
        anyhow::bail!("forced failure after_adr_clear");
    }
    if fail_before == Some("before_done_flag") {
        anyhow::bail!("forced failure before_done_flag");
    }

    tx.execute(
        "INSERT INTO settings(key, value_json) VALUES(?1, '1')
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![MIGRATION_MCP_QUALITY_V1],
    )?;
    tx.commit()?;
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
        // F19: require both empty repo AND zero graph so a real project that
        // has a repo_path but is not indexed yet is not soft-hidden.
        if exists && empty_repo && zero_graph {
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

    fn fixture_legacy_experiences_with_settings(sql_extra: &str) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            r#"
            CREATE TABLE settings (key TEXT PRIMARY KEY, value_json TEXT NOT NULL);
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
            {sql_extra}
            "#
        ))
        .unwrap();
        conn
    }

    #[tokio::test]
    async fn migration_marks_existing_reviewed() {
        let conn = fixture_legacy_experiences_with_settings(
            r#"
            INSERT INTO experiences (
                id, project_id, agent_id, topic, solution_summary, adr_record,
                outcome, tags_json, created_at, payload_json
            ) VALUES (
                'old-1', 'p', 'agent', 't', 's', 'adr',
                'success', '[]', '2026-01-01T00:00:00Z', '{}'
            );
            "#,
        );
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

    #[test]
    fn mcp_quality_migration_success_clears_dup_adr_and_sets_flag() {
        let conn = fixture_legacy_experiences_with_settings(
            r#"
            INSERT INTO experiences (
                id, project_id, agent_id, topic, solution_summary, adr_record,
                outcome, tags_json, created_at, payload_json
            ) VALUES
                ('dup', 'p', 'a', 'topic here', 'same body', 'same body',
                 'success', '[]', '2026-01-01T00:00:00Z', '{}'),
                ('ok', 'p', 'a', 'topic', 'solution', 'real adr',
                 'success', '[]', '2026-01-01T00:00:00Z', '{}');
            "#,
        );
        migrate_mcp_quality_v1(&conn).unwrap();
        let (adr_dup, title_dup): (String, String) = conn
            .query_row(
                "SELECT adr_record, title FROM experiences WHERE id = 'dup'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(adr_dup, "");
        assert!(!title_dup.is_empty());
        assert!(title_dup.chars().count() <= 80);
        let adr_ok: String = conn
            .query_row(
                "SELECT adr_record FROM experiences WHERE id = 'ok'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(adr_ok, "real adr");
        let flag: String = conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                params![MIGRATION_MCP_QUALITY_V1],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(flag, "1");
    }

    #[test]
    fn mcp_quality_migration_midway_failure_rolls_back_no_flag() {
        let conn = fixture_legacy_experiences_with_settings(
            r#"
            INSERT INTO experiences (
                id, project_id, agent_id, topic, solution_summary, adr_record,
                outcome, tags_json, created_at, payload_json
            ) VALUES (
                'dup', 'p', 'a', 't', 'same', 'same',
                'success', '[]', '2026-01-01T00:00:00Z', '{}'
            );
            "#,
        );
        let err = migrate_mcp_quality_v1_impl(&conn, Some("before_done_flag")).unwrap_err();
        assert!(err.to_string().contains("forced failure"));
        // No done flag.
        let flag: Option<String> = conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                params![MIGRATION_MCP_QUALITY_V1],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert!(flag.is_none(), "done flag must not be written on rollback");
        // ADR cleanup rolled back.
        let adr: String = conn
            .query_row(
                "SELECT adr_record FROM experiences WHERE id = 'dup'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(adr, "same");
        // Title column must not remain from a partial commit.
        let cols = column_names(&conn, "experiences").unwrap();
        assert!(
            !has_col(&cols, "title"),
            "ALTER must roll back with the transaction"
        );
    }

    #[test]
    fn mcp_quality_migration_idempotent_rerun() {
        let conn = fixture_legacy_experiences_with_settings(
            r#"
            INSERT INTO experiences (
                id, project_id, agent_id, topic, solution_summary, adr_record,
                outcome, tags_json, created_at, payload_json
            ) VALUES (
                'dup', 'p', 'a', 't', 'body', 'body',
                'success', '[]', '2026-01-01T00:00:00Z', '{}'
            );
            "#,
        );
        migrate_mcp_quality_v1(&conn).unwrap();
        // Restore a duplicate ADR after first run — re-run must be a no-op (flag set).
        conn.execute(
            "UPDATE experiences SET adr_record = solution_summary WHERE id = 'dup'",
            [],
        )
        .unwrap();
        migrate_mcp_quality_v1(&conn).unwrap();
        let adr: String = conn
            .query_row(
                "SELECT adr_record FROM experiences WHERE id = 'dup'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            adr, "body",
            "idempotent re-run must not re-apply ADR cleanup"
        );
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
        assert_eq!(archived.archived_by.as_deref(), Some(ARCHIVED_BY_USER));

        store.unarchive_experience(id.clone()).await.unwrap();
        let active = store.get_record(id).await.unwrap().unwrap();
        assert_eq!(active.status, EXPERIENCE_STATUS_ACTIVE);
        assert!(active.archived_at.is_none());
        assert!(active.archived_by.is_none());
    }

    #[tokio::test]
    async fn mark_all_reviewed_updates_every_active_row() {
        let store = ExperienceStore::memory().unwrap();
        for i in 0..3 {
            let mut record = ExperienceRecord::from_task(
                &LoungeTask::new("cursor", "proj-all", format!("t{i}")),
                "s",
                "a",
                ExperienceOutcome::Success,
                vec![],
            );
            record.reviewed = false;
            store.insert_record(record).await.unwrap();
        }
        let mut archived = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj-all", "archived"),
            "s",
            "a",
            ExperienceOutcome::Success,
            vec![],
        );
        archived.reviewed = false;
        let archived_id = archived.id.clone();
        store.insert_record(archived).await.unwrap();
        store.archive_experience(archived_id.clone()).await.unwrap();

        assert_eq!(store.count_unreviewed_experiences().await.unwrap(), 3);
        let changed = store.mark_all_experiences_reviewed().await.unwrap();
        assert_eq!(changed, 3);
        assert_eq!(store.count_unreviewed_experiences().await.unwrap(), 0);
        let still = store.get_record(archived_id).await.unwrap().unwrap();
        assert!(!still.reviewed, "archived rows are outside mark-all scope");
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
        let n = store.auto_archive_stale(90, 0, &clock).await.unwrap();
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
        let conn = fixture_legacy_experiences_with_settings(
            r#"
            ALTER TABLE experiences ADD COLUMN status TEXT NOT NULL DEFAULT 'deprecated';
            INSERT INTO experiences (
                id, project_id, agent_id, topic, solution_summary, adr_record,
                outcome, tags_json, created_at, payload_json, status
            ) VALUES (
                'dep-1', 'p', 'agent', 't', 's', 'adr',
                'success', '[]', '2026-01-01T00:00:00Z', '{}', 'deprecated'
            );
            "#,
        );
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
        // backend-legacy: empty repo AND zero graph → hidden.
        // frontend-new: has valid repo + node → not hidden (empty_repo && zero_graph).
        assert!(first >= 1, "should hide at least backend-legacy");
        assert!(is_project_hidden(&conn, "backend-legacy").unwrap());
        assert!(!is_project_hidden(&conn, "frontend-new").unwrap());

        let second = cleanup_placeholder_projects(&conn).unwrap();
        assert_eq!(second, 0, "migration must be one-shot");

        // Repo present but not indexed yet must NOT be hidden (F19 AND rule).
        let conn_repo_only = Connection::open_in_memory().unwrap();
        conn_repo_only
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
                ('r1', 'frontend-new', '/repos/frontend-new', 'meta', 'frontend-new', '2026-01-01T00:00:00Z');
            "#,
            )
            .unwrap();
        assert_eq!(cleanup_placeholder_projects(&conn_repo_only).unwrap(), 0);
        assert!(!is_project_hidden(&conn_repo_only, "frontend-new").unwrap());

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

    #[tokio::test]
    async fn reject_and_undo_persist_rejected_provenance() {
        let store = ExperienceStore::memory().unwrap();
        let mut record = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj", "reject-me"),
            "solution",
            "adr",
            ExperienceOutcome::Success,
            vec![],
        );
        record.reviewed = false;
        let id = record.id.clone();
        store.insert_record(record).await.unwrap();

        store.reject_experience(id.clone()).await.unwrap();
        let rejected = store.get_record(id.clone()).await.unwrap().unwrap();
        assert_eq!(rejected.status, EXPERIENCE_STATUS_ARCHIVED);
        assert_eq!(rejected.archived_by.as_deref(), Some(ARCHIVED_BY_REJECTED));
        assert!(rejected.archived_at.is_some());
        assert_eq!(store.count_unreviewed_experiences().await.unwrap(), 0);

        let stats = store.count_review_stats().await.unwrap();
        assert_eq!(stats.rejected, 1);
        assert_eq!(stats.pending, 0);

        store.undo_reject_experience(id.clone()).await.unwrap();
        let active = store.get_record(id).await.unwrap().unwrap();
        assert_eq!(active.status, EXPERIENCE_STATUS_ACTIVE);
        assert!(active.archived_by.is_none());
        assert_eq!(store.count_unreviewed_experiences().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn review_queue_orders_hygiene_before_plain_unreviewed() {
        let store = ExperienceStore::memory().unwrap();
        let mut plain = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj", "plain-topic"),
            "plain solution",
            "plain adr",
            ExperienceOutcome::Success,
            vec![],
        );
        plain.reviewed = false;
        plain.created_at = "2026-10-20T12:00:00+00:00".into();
        let plain_id = plain.id.clone();
        store.insert_record(plain).await.unwrap();

        let mut hygiene = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj", "masked <home>/secret"),
            "token <redacted-token> gone",
            "adr ok",
            ExperienceOutcome::Success,
            vec![],
        );
        hygiene.reviewed = false;
        hygiene.created_at = "2026-10-19T12:00:00+00:00".into();
        let hygiene_id = hygiene.id.clone();
        store.insert_record(hygiene).await.unwrap();

        let unreviewed = store.count_unreviewed_experiences().await.unwrap();
        assert_eq!(unreviewed, 2, "both rows must be active unreviewed");

        let queue = store
            .list_review_queue(ReviewQueueFilter {
                stat: ReviewStatFilter::Pending,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            queue.len(),
            2,
            "queue ids={:?}",
            queue.iter().map(|r| r.id.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(queue[0].id, hygiene_id, "hygiene-flagged must sort first");
        assert!(queue.iter().any(|r| r.id == plain_id));

        let stats = store.count_review_stats().await.unwrap();
        assert_eq!(
            stats.pending,
            store.count_unreviewed_experiences().await.unwrap()
        );
        assert!(stats.hygiene >= 1);
    }

    #[tokio::test]
    async fn search_orders_reviewed_before_unreviewed_same_band() {
        let store = ExperienceStore::memory().unwrap();
        let mut approved = ExperienceRecord::from_task(
            &LoungeTask::new("a", "band-proj", "shared-keyword-alpha"),
            "shared-keyword-alpha solution",
            "shared-keyword-alpha adr",
            ExperienceOutcome::Success,
            vec![],
        );
        approved.reviewed = true;
        approved.created_at = "2026-01-01T00:00:00+00:00".into();
        let approved_id = approved.id.clone();
        store.insert_record(approved).await.unwrap();

        let mut pending = ExperienceRecord::from_task(
            &LoungeTask::new("a", "band-proj", "shared-keyword-beta"),
            "shared-keyword-beta solution",
            "shared-keyword-beta adr",
            ExperienceOutcome::Success,
            vec![],
        );
        pending.reviewed = false;
        pending.created_at = "2026-06-01T00:00:00+00:00".into();
        let pending_id = pending.id.clone();
        store.insert_record(pending).await.unwrap();

        let hits = store
            .search_experiences_filtered(
                "shared-keyword".into(),
                Some(10),
                Some(EXPERIENCE_STATUS_ACTIVE),
            )
            .await
            .unwrap();
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert!(ids.contains(&approved_id.as_str()));
        assert!(
            ids.contains(&pending_id.as_str()),
            "unreviewed still returned"
        );
        let ai = ids.iter().position(|id| *id == approved_id).unwrap();
        let pi = ids.iter().position(|id| *id == pending_id).unwrap();
        assert!(ai < pi, "reviewed must rank above unreviewed in band");
    }

    #[tokio::test]
    async fn search_strong_unreviewed_outranks_weak_approved() {
        let store = ExperienceStore::memory().unwrap();
        let needle = "redis-cluster-failover-protocol";
        let mut strong_pending = ExperienceRecord::from_task(
            &LoungeTask::new("a", "band-proj", needle),
            needle,
            format!("{needle} adr"),
            ExperienceOutcome::Success,
            vec![],
        );
        strong_pending.reviewed = false;
        strong_pending.created_at = "2026-01-01T00:00:00+00:00".into();
        let pending_id = strong_pending.id.clone();
        store.insert_record(strong_pending).await.unwrap();

        let mut weak_approved = ExperienceRecord::from_task(
            &LoungeTask::new("a", "band-proj", "unrelated-topic"),
            "entirely different solution about networking caches",
            "no needle in adr body",
            ExperienceOutcome::Success,
            vec![needle.into()],
        );
        weak_approved.reviewed = true;
        weak_approved.created_at = "2026-06-01T00:00:00+00:00".into();
        let approved_id = weak_approved.id.clone();
        store.insert_record(weak_approved).await.unwrap();

        let hits = store
            .search_experiences_filtered(needle.into(), Some(10), Some(EXPERIENCE_STATUS_ACTIVE))
            .await
            .unwrap();
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert!(ids.contains(&pending_id.as_str()));
        assert!(ids.contains(&approved_id.as_str()));
        let pi = ids.iter().position(|id| *id == pending_id).unwrap();
        let ai = ids.iter().position(|id| *id == approved_id).unwrap();
        assert!(
            pi < ai,
            "strongly relevant unreviewed must outrank weakly relevant approved (UD-5)"
        );

        let pending_score = lexical_search_score(needle, &hits[pi]);
        let approved_score = lexical_search_score(needle, &hits[ai]);
        assert!(
            (pending_score - approved_score).abs() > REVIEW_SCORE_BAND,
            "scores must differ by more than REVIEW_SCORE_BAND ({REVIEW_SCORE_BAND})"
        );
    }

    #[test]
    fn review_score_band_tiebreak_and_relevance_primary() {
        use super::{cmp_search_with_review_band, SearchRankKey, REVIEW_SCORE_BAND};
        use std::cmp::Ordering;
        // Same bucket → approved first.
        assert_eq!(
            cmp_search_with_review_band(
                SearchRankKey {
                    same_project: false,
                    score: 0.80,
                    reviewed: true,
                    created_at: "a",
                    id: "id-a",
                },
                SearchRankKey {
                    same_project: false,
                    score: 0.80 + REVIEW_SCORE_BAND / 2.0,
                    reviewed: false,
                    created_at: "z",
                    id: "id-b",
                },
            ),
            Ordering::Less
        );
        // Different buckets → higher score first even if unreviewed.
        assert_eq!(
            cmp_search_with_review_band(
                SearchRankKey {
                    same_project: false,
                    score: 0.95,
                    reviewed: false,
                    created_at: "a",
                    id: "id-a",
                },
                SearchRankKey {
                    same_project: false,
                    score: 0.40,
                    reviewed: true,
                    created_at: "z",
                    id: "id-b",
                },
            ),
            Ordering::Less
        );
        // Cycle repro from review: A(0.80,rev), B(0.84), C(0.88,older) — total order.
        let a = SearchRankKey {
            same_project: false,
            score: 0.80,
            reviewed: true,
            created_at: "2026-01-01",
            id: "a",
        };
        let b = SearchRankKey {
            same_project: false,
            score: 0.84,
            reviewed: false,
            created_at: "2026-06-01",
            id: "b",
        };
        let c = SearchRankKey {
            same_project: false,
            score: 0.88,
            reviewed: false,
            created_at: "2025-01-01",
            id: "c",
        };
        // C (bucket 17) before A/B (bucket 16); A reviewed before B in same bucket.
        assert_eq!(cmp_search_with_review_band(c, a), Ordering::Less);
        assert_eq!(cmp_search_with_review_band(c, b), Ordering::Less);
        assert_eq!(cmp_search_with_review_band(a, b), Ordering::Less);
    }

    #[test]
    fn same_project_hard_tier_outranks_higher_score_other_project() {
        use super::{cmp_search_with_review_band, sort_experiences_for_search, SearchRankKey};
        use crate::models::LoungeExperience;
        use std::cmp::Ordering;
        // Weak same-project beat strong other-project (main hard tier).
        assert_eq!(
            cmp_search_with_review_band(
                SearchRankKey {
                    same_project: true,
                    score: 0.20,
                    reviewed: false,
                    created_at: "a",
                    id: "same",
                },
                SearchRankKey {
                    same_project: false,
                    score: 0.99,
                    reviewed: true,
                    created_at: "z",
                    id: "other",
                },
            ),
            Ordering::Less
        );

        let mut hits = vec![
            LoungeExperience {
                id: "other-hi".into(),
                project_id: "other".into(),
                title: "needle-xyz".into(),
                adr_summary: "needle-xyz strong".into(),
                reviewed: true,
                created_at: "2026-01-02T00:00:00Z".into(),
                ..Default::default()
            },
            LoungeExperience {
                id: "same-lo".into(),
                project_id: "mine".into(),
                title: "unrelated".into(),
                adr_summary: "weak".into(),
                tags: vec!["needle-xyz".into()],
                reviewed: false,
                created_at: "2026-01-01T00:00:00Z".into(),
                ..Default::default()
            },
        ];
        sort_experiences_for_search(&mut hits, "needle-xyz", Some("mine"));
        assert_eq!(hits[0].id, "same-lo", "same project must hard-rank first");
        assert_eq!(hits[1].id, "other-hi");
    }

    #[test]
    fn normalize_search_score_clamps_nan_and_negative() {
        use super::{normalize_search_score, review_score_bucket};
        assert_eq!(normalize_search_score(f64::NAN), 0.0);
        assert_eq!(normalize_search_score(f64::NEG_INFINITY), 0.0);
        assert_eq!(normalize_search_score(-1.0), 0.0);
        assert_eq!(normalize_search_score(0.42), 0.42);
        assert_eq!(review_score_bucket(0.80), 16);
        assert_eq!(review_score_bucket(0.84), 16);
        assert_eq!(review_score_bucket(0.88), 17);
    }

    #[test]
    fn strip_original_content_for_ui_clears_field() {
        use super::strip_original_content_for_ui;
        use crate::models::LoungeExperience;
        let row = LoungeExperience {
            id: "strip-me".into(),
            original_content: Some("/Users/ci/secret".into()),
            ..Default::default()
        };
        let clean = strip_original_content_for_ui(row);
        assert!(clean.original_content.is_none());
    }

    /// Property: bucket comparator is a total order — no panic under `sort_by`,
    /// antisymmetric + transitive on samples, and a ≥2-bucket gap always ranks
    /// the higher score first. Scores include Vault-1 hybrid combinations
    /// (`0.7 * semantic + 0.3 * lexical`, including missing-semantic → 0.0).
    #[test]
    fn review_score_band_comparator_is_total_order_property() {
        use super::{
            cmp_search_with_review_band, normalize_search_score, review_score_bucket,
            SearchRankKey, REVIEW_SCORE_BAND,
        };
        use crate::db::vault_fts::{hybrid_search_score, normalize_bm25};
        use std::cmp::Ordering;

        #[derive(Clone, Debug)]
        struct Hit {
            same_project: bool,
            score: f64,
            reviewed: bool,
            created: String,
            id: String,
        }

        fn cmp_hit(a: &Hit, b: &Hit) -> Ordering {
            cmp_search_with_review_band(
                SearchRankKey {
                    same_project: a.same_project,
                    score: a.score,
                    reviewed: a.reviewed,
                    created_at: &a.created,
                    id: &a.id,
                },
                SearchRankKey {
                    same_project: b.same_project,
                    score: b.score,
                    reviewed: b.reviewed,
                    created_at: &b.created,
                    id: &b.id,
                },
            )
        }

        // xorshift64 — deterministic, no extra crate.
        let mut state: u64 = 0x00C0_FFEE_DEAD_BEEF;
        let mut next_u64 = || -> u64 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };

        for trial in 0..400 {
            let n = (next_u64() % 256) as usize + 1;
            let mut hits: Vec<Hit> = Vec::with_capacity(n);
            for i in 0..n {
                let raw = (next_u64() as f64) / (u64::MAX as f64);
                let score = match next_u64() % 32 {
                    0 => f64::NAN,
                    1 => -raw,
                    2 => f64::INFINITY,
                    3 => f64::NEG_INFINITY,
                    4 => raw * 3.0,
                    // Vault-1 hybrid: both present.
                    5 => hybrid_search_score(Some(raw), normalize_bm25(-(1.0 - raw) * 8.0)),
                    // Vault-1 hybrid: semantic missing → 0.0.
                    6 => hybrid_search_score(None, normalize_bm25(-(1.0 - raw) * 8.0)),
                    7 => hybrid_search_score(Some(1.0 - raw), raw),
                    _ => raw,
                };
                hits.push(Hit {
                    same_project: next_u64() % 2 == 0,
                    score,
                    reviewed: next_u64() % 2 == 0,
                    created: format!(
                        "2026-{:02}-{:02}T{:02}:00:00Z",
                        (i % 12) + 1,
                        (i % 28) + 1,
                        i % 24
                    ),
                    id: format!("id-{trial:04}-{i:04}-{:016x}", next_u64()),
                });
            }

            // Must not panic (Rust 1.81+ sort_by requires total order).
            hits.sort_by(cmp_hit);

            // Antisymmetry + equality: a≤b and b≤a ⇒ equal keys after normalize.
            for w in hits.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                let ab = cmp_hit(a, b);
                let ba = cmp_hit(b, a);
                match ab {
                    Ordering::Equal => assert_eq!(ba, Ordering::Equal),
                    Ordering::Less => assert_eq!(ba, Ordering::Greater),
                    Ordering::Greater => assert_eq!(ba, Ordering::Less),
                }
            }

            // Transitivity on random triples from this list.
            if hits.len() >= 3 {
                for _ in 0..32 {
                    let i = (next_u64() as usize) % hits.len();
                    let j = (next_u64() as usize) % hits.len();
                    let k = (next_u64() as usize) % hits.len();
                    let (a, b, c) = (&hits[i], &hits[j], &hits[k]);
                    if cmp_hit(a, b) == Ordering::Less && cmp_hit(b, c) == Ordering::Less {
                        assert_eq!(
                            cmp_hit(a, c),
                            Ordering::Less,
                            "transitivity failed at trial {trial}: {a:?} < {b:?} < {c:?}"
                        );
                    }
                }
            }

            // ≥2 bucket gap within the same project tier ⇒ higher score first.
            for a in &hits {
                for b in &hits {
                    if a.same_project != b.same_project {
                        continue;
                    }
                    let sa = normalize_search_score(a.score);
                    let sb = normalize_search_score(b.score);
                    let ba = review_score_bucket(sa);
                    let bb = review_score_bucket(sb);
                    if ba >= bb + 2 {
                        assert_eq!(
                            cmp_hit(a, b),
                            Ordering::Less,
                            "bucket gap≥2 ({ba} vs {bb}, band={REVIEW_SCORE_BAND}): higher must lead"
                        );
                    }
                }
            }
        }
    }
}
