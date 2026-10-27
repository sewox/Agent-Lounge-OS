use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::embedding::{cosine_similarity, decode_embedding, encode_embedding, lexical_embedding};
use crate::models::{
    now_rfc3339, ExperienceHit, ExperienceRecord, LoungeExperience, RoutingPolicy,
};

const DEFAULT_SIMILAR_LIMIT: usize = 3;
const MIN_COSINE: f32 = 0.22;

#[derive(Clone)]
pub struct ExperienceStore {
    pub(crate) conn: Arc<Mutex<Connection>>,
}

impl ExperienceStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("experiences dizini oluşturulamadı: {}", parent.display())
            })?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("SQLite açılamadı: {}", path.display()))?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.migrate()?;
        Ok(store)
    }

    pub fn memory() -> Result<Self> {
        let store = Self {
            conn: Arc::new(Mutex::new(Connection::open_in_memory()?)),
        };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        migrate_schema(&conn)
    }

    /// Integration-test helper: list columns for a table.
    pub fn table_columns(&self, table: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().expect("experience db lock");
        column_names(&conn, table)
    }

    /// Integration-test helper: run arbitrary SQL then re-apply governance migrate.
    pub fn exec_sql_and_migrate_governance(&self, sql: &str) -> Result<()> {
        let conn = self.conn.lock().expect("experience db lock");
        if !sql.trim().is_empty() {
            conn.execute_batch(sql)?;
        }
        super::experience_governance::migrate_experience_governance(&conn)
    }

    /// Integration-test helper: `(status, archived_at)` for an experience id.
    pub fn conn_query_status_for_tests(&self, id: &str) -> Result<(String, Option<String>)> {
        let conn = self.conn.lock().expect("experience db lock");
        Ok(conn.query_row(
            "SELECT status, archived_at FROM experiences WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    pub async fn insert(&self, experience: &LoungeExperience) -> Result<()> {
        let solution = if experience
            .original_content
            .as_deref()
            .unwrap_or("")
            .is_empty()
        {
            if experience.adr_summary.trim().is_empty() {
                experience.title.clone()
            } else {
                experience.adr_summary.clone()
            }
        } else {
            experience
                .original_content
                .clone()
                .unwrap_or_else(|| experience.adr_summary.clone())
        };
        let topic = if experience.title.trim().is_empty() {
            solution.clone()
        } else {
            experience.title.clone()
        };
        let record = ExperienceRecord::from_lounge(experience, topic, solution);
        self.insert_record(record).await
    }

    /// Persist a title when allowed (never overwrites agent/user unless `force`).
    pub async fn set_experience_title(
        &self,
        id: String,
        title: String,
        source: crate::models::TitleSource,
        force: bool,
    ) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            set_title_blocking(&conn, &id, &title, source, force)
        })
        .await
        .context("set_experience_title join")?
    }

    /// Rows with empty/fallback title that are due for (re)try — excludes give-up.
    pub async fn list_ids_needing_title(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            list_needing_title_blocking(&conn)
        })
        .await
        .context("list_ids_needing_title join")?
    }

    pub async fn record_title_retry(
        &self,
        id: String,
        attempts: u32,
        next_retry_at: String,
    ) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            record_title_retry_blocking(&conn, &id, attempts, &next_retry_at)
        })
        .await
        .context("record_title_retry join")?
    }

    pub async fn mark_title_give_up(&self, id: String) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            mark_title_give_up_blocking(&conn, &id)
        })
        .await
        .context("mark_title_give_up join")?
    }

    pub async fn clear_title_retry(&self, id: String) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            clear_title_retry_blocking(&conn, &id)
        })
        .await
        .context("clear_title_retry join")?
    }

    pub async fn count_title_backlog(&self) -> Result<u64> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            count_title_backlog_blocking(&conn)
        })
        .await
        .context("count_title_backlog join")?
    }

    /// Health-1b: experiences that permanently gave up on LLM titles.
    pub async fn count_title_give_ups(&self) -> Result<u64> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            count_title_give_ups_blocking(&conn)
        })
        .await
        .context("count_title_give_ups join")?
    }

    pub async fn settings_flag_done(&self, key: String) -> Result<bool> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            settings_flag_done_blocking(&conn, &key)
        })
        .await
        .context("settings_flag_done join")?
    }

    pub async fn set_settings_flag(&self, key: String) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            set_settings_flag_blocking(&conn, &key)
        })
        .await
        .context("set_settings_flag join")?
    }

    /// Title retry bookkeeping for tests / Health-1.
    pub async fn title_retry_state(&self, id: String) -> Result<(u32, Option<String>, bool)> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            conn.query_row(
                "SELECT COALESCE(title_attempts,0), title_next_retry_at, COALESCE(title_give_up,0) FROM experiences WHERE id = ?1",
                params![id],
                |row| {
                    let attempts: i64 = row.get(0)?;
                    let next: Option<String> = row.get(1)?;
                    let give: i64 = row.get(2)?;
                    Ok((attempts.max(0) as u32, next, give != 0))
                },
            )
            .map_err(Into::into)
        })
        .await
        .context("title_retry_state join")?
    }

    pub async fn insert_record(&self, record: ExperienceRecord) -> Result<()> {
        let prepared = prepare_record_embedding(record);
        let conn = self.conn.clone();
        let for_vector = prepared.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            insert_record_blocking(&conn, &prepared)
        })
        .await
        .context("experience insert join")??;
        super::vector_memory::spawn_upsert(for_vector);
        Ok(())
    }

    /// SQLite (+ yerel embedding) ve yapılandırılmış uzak vektör indeksini atomik yazar.
    /// Uzak yazım başarısız olursa SQLite satırı geri alınır.
    /// Uzak backend yoksa tek SQLite yazımı yeterlidir (yerel embedding aynı satırda).
    pub async fn insert_record_atomic(&self, record: ExperienceRecord) -> Result<()> {
        let prepared = prepare_record_embedding(record);
        let id = prepared.id.clone();
        let conn = self.conn.clone();
        let for_sqlite = prepared.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            insert_record_blocking(&conn, &for_sqlite)
        })
        .await
        .context("experience atomic insert join")??;

        if !super::vector_memory::remote_configured() {
            return Ok(());
        }

        match super::vector_memory::upsert_experience(&prepared).await {
            Ok(()) => Ok(()),
            Err(err) => {
                if let Err(rollback_err) = self.delete_record(id.clone()).await {
                    log::error!(
                        "vektör upsert başarısız ve SQLite rollback de başarısız: {err}; {rollback_err}"
                    );
                }
                Err(err).context("vektör indeksi yazılamadı; SQLite kaydı geri alındı")
            }
        }
    }

    pub async fn delete_record(&self, id: String) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            conn.execute(
                "DELETE FROM experiences WHERE id = ?1",
                rusqlite::params![id],
            )?;
            Ok(())
        })
        .await
        .context("experience delete join")?
    }

    pub async fn get(&self, id: String) -> Result<Option<LoungeExperience>> {
        let record = self.get_record(id).await?;
        Ok(record.map(|row| row.to_lounge()))
    }

    pub async fn get_record(&self, id: String) -> Result<Option<ExperienceRecord>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            get_record_blocking(&conn, &id)
        })
        .await
        .context("experience get join")?
    }

    pub async fn latest(&self, limit: usize) -> Result<Vec<LoungeExperience>> {
        self.list_experiences(limit, false).await
    }

    /// List experiences ordered by `is_pinned DESC, created_at DESC`.
    /// When `include_archived` is false (default), only `active` rows are returned.
    /// Optional `project_id` is resolved via Project-ID-Core (`prefer_canonical`) and
    /// matched exactly (never substring `LIKE`).
    pub async fn list_experiences(
        &self,
        limit: usize,
        include_archived: bool,
    ) -> Result<Vec<LoungeExperience>> {
        self.list_experiences_page(limit, include_archived, 0, None)
            .await
    }

    pub async fn list_experiences_page(
        &self,
        limit: usize,
        include_archived: bool,
        offset: usize,
        project_id: Option<String>,
    ) -> Result<Vec<LoungeExperience>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            latest_blocking(
                &conn,
                limit,
                include_archived,
                offset,
                project_id.as_deref(),
            )
        })
        .await
        .context("experience list join")?
    }

    pub async fn count_experiences(
        &self,
        include_archived: bool,
        project_id: Option<String>,
    ) -> Result<u64> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            count_experiences_blocking(&conn, include_archived, project_id.as_deref())
        })
        .await
        .context("experience count join")?
    }

    /// Command Palette / vault: lexical+vektör benzerliği, boşsa substring fallback.
    pub async fn search_experiences(
        &self,
        query: String,
        limit: Option<usize>,
    ) -> Result<Vec<LoungeExperience>> {
        let lim = limit.unwrap_or(12).max(1);
        let needle = query.trim().to_string();
        if needle.is_empty() {
            return self.latest(lim).await;
        }

        let embedding = lexical_embedding(&needle);
        let hits = self
            .similar_cross(String::new(), embedding, Some(lim * 2))
            .await
            .unwrap_or_default();

        let mut out: Vec<LoungeExperience> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for hit in hits {
            if out.len() >= lim {
                break;
            }
            if !seen.insert(hit.id.clone()) {
                continue;
            }
            if let Some(row) = self.get(hit.id.clone()).await? {
                out.push(row);
            } else {
                out.push(crate::models::LoungeExperience {
                    id: hit.id,
                    msg_type: "experience".into(),
                    agent: hit.agent_id,
                    project_id: hit.project_id,
                    adr_summary: if hit.adr_record.trim().is_empty() {
                        hit.solution_summary
                    } else {
                        hit.adr_record
                    },
                    outcome: crate::models::ExperienceOutcome::Partial,
                    related_task_id: None,
                    tags: vec![format!("score:{:.2}", hit.score)],
                    created_at: crate::models::now_rfc3339(),
                    ..Default::default()
                });
            }
        }

        if out.len() < lim {
            let lower = needle.to_ascii_lowercase();
            for row in self.latest(80).await? {
                if out.len() >= lim {
                    break;
                }
                if !seen.insert(row.id.clone()) {
                    continue;
                }
                let hay = format!(
                    "{} {} {} {}",
                    row.project_id,
                    row.adr_summary,
                    row.agent,
                    row.tags.join(" ")
                )
                .to_ascii_lowercase();
                if hay.contains(&lower) {
                    out.push(row);
                }
            }
        }

        Ok(out)
    }

    /// Semantik (kosinüs) arama: aynı `project_id` öncelikli, yoksa küresel.
    pub async fn similar(
        &self,
        project_id: String,
        query: String,
        limit: Option<usize>,
    ) -> Result<Vec<ExperienceHit>> {
        let embedding = lexical_embedding(&query);
        self.similar_to(project_id, embedding, limit).await
    }

    pub async fn similar_to(
        &self,
        project_id: String,
        query: Vec<f32>,
        limit: Option<usize>,
    ) -> Result<Vec<ExperienceHit>> {
        let conn = self.conn.clone();
        let limit = limit.unwrap_or(DEFAULT_SIMILAR_LIMIT);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            similar_blocking(&conn, &project_id, &query, limit)
        })
        .await
        .context("experience similar join")?
    }

    /// Tüm projeler üzerinde kosinüs araması (Cross-Project Memory).
    pub async fn similar_cross(
        &self,
        project_id: String,
        query: Vec<f32>,
        limit: Option<usize>,
    ) -> Result<Vec<ExperienceHit>> {
        let conn = self.conn.clone();
        let limit = limit.unwrap_or(DEFAULT_SIMILAR_LIMIT);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            similar_cross_blocking(&conn, &project_id, &query, limit)
        })
        .await
        .context("experience similar_cross join")?
    }

    pub async fn get_setting(&self, key: String) -> Result<Option<String>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            conn.query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                rusqlite::params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(anyhow::Error::from)
        })
        .await
        .context("settings get join")?
    }

    pub async fn set_setting(&self, key: String, value: String) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            conn.execute(
                "INSERT INTO settings(key, value_json) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
                rusqlite::params![key, value],
            )?;
            Ok(())
        })
        .await
        .context("settings set join")?
    }

    pub async fn get_routing_policy(&self) -> Result<RoutingPolicy> {
        match self.get_setting("routing_policy".into()).await? {
            Some(raw) => Ok(serde_json::from_str(&raw).unwrap_or_default()),
            None => Ok(RoutingPolicy::default()),
        }
    }

    pub async fn set_routing_policy(&self, policy: &RoutingPolicy) -> Result<()> {
        let raw = serde_json::to_string(policy)?;
        self.set_setting("routing_policy".into(), raw).await
    }
}

fn create_experiences_sql() -> &'static str {
    r#"
            CREATE TABLE IF NOT EXISTS experiences (
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
                status TEXT NOT NULL DEFAULT 'active',
                reviewed INTEGER NOT NULL DEFAULT 0,
                reviewed_at TEXT,
                use_count INTEGER NOT NULL DEFAULT 0,
                last_used_at TEXT,
                archived_at TEXT,
                is_pinned INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT,
                original_content TEXT,
                archived_by TEXT,
                title TEXT NOT NULL DEFAULT '',
                title_source TEXT NOT NULL DEFAULT 'fallback',
                source TEXT NOT NULL DEFAULT '',
                title_attempts INTEGER NOT NULL DEFAULT 0,
                title_next_retry_at TEXT,
                title_give_up INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_experiences_project ON experiences(project_id);
            CREATE INDEX IF NOT EXISTS idx_experiences_agent ON experiences(agent_id);
            CREATE INDEX IF NOT EXISTS idx_experiences_topic ON experiences(project_id, topic);
            CREATE INDEX IF NOT EXISTS idx_experiences_status ON experiences(status);
            CREATE INDEX IF NOT EXISTS idx_experiences_reviewed ON experiences(reviewed);
            "#
}

fn create_settings_sql() -> &'static str {
    r#"
            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value_json TEXT NOT NULL
            );
            "#
}

fn migrate_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(create_settings_sql())?;
    // FTS triggers (from a prior open) may fire during later migrations; the
    // fold UDF must exist on this connection before any experiences/project_index writes.
    crate::db::vault_fts::register_turkish_fold_fn(conn)?;
    crate::db::connected_tools::migrate_connected_tools(conn)?;
    crate::db::project_index::migrate_project_index(conn)?;
    crate::db::project_identity::migrate_project_identity(conn)?;
    crate::db::feedback::migrate_feedback(conn)?;
    crate::db::a2a::migrate_a2a(conn)?;
    crate::db::journal::migrate_journal(conn)?;
    let exists = table_exists(conn, "experiences")?;
    if !exists {
        conn.execute_batch(create_experiences_sql())?;
        let _ = crate::db::experience_governance::cleanup_placeholder_projects(conn);
        crate::db::project_identity::migrate_project_ids_to_canonical(conn)?;
        crate::db::vault_fts::migrate_vault_fts(conn)?;
        return Ok(());
    }

    let cols = column_names(conn, "experiences")?;
    let required = [
        "id",
        "project_id",
        "agent_id",
        "topic",
        "solution_summary",
        "adr_record",
    ];
    let has_required = required
        .iter()
        .all(|name| cols.iter().any(|col| col == name));
    if !has_required {
        rebuild_legacy(conn, &cols)?;
        crate::db::experience_governance::migrate_experience_governance(conn)?;
        let _ = crate::db::experience_governance::cleanup_placeholder_projects(conn);
        crate::db::project_identity::migrate_project_ids_to_canonical(conn)?;
        crate::db::vault_fts::migrate_vault_fts(conn)?;
        return Ok(());
    }

    if !cols.iter().any(|col| col == "embedding") {
        conn.execute("ALTER TABLE experiences ADD COLUMN embedding BLOB", [])?;
    }
    conn.execute_batch(
        r#"
            CREATE INDEX IF NOT EXISTS idx_experiences_project ON experiences(project_id);
            CREATE INDEX IF NOT EXISTS idx_experiences_agent ON experiences(agent_id);
            CREATE INDEX IF NOT EXISTS idx_experiences_topic ON experiences(project_id, topic);
            "#,
    )?;
    crate::db::experience_governance::migrate_experience_governance(conn)?;
    let _ = crate::db::experience_governance::cleanup_placeholder_projects(conn);
    crate::db::project_identity::migrate_project_ids_to_canonical(conn)?;
    crate::db::vault_fts::migrate_vault_fts(conn)?;
    Ok(())
}

fn rebuild_legacy(conn: &Connection, cols: &[String]) -> Result<()> {
    conn.execute("ALTER TABLE experiences RENAME TO experiences_legacy", [])?;
    conn.execute_batch(create_experiences_sql())?;

    let agent = expr(cols, &["agent_id", "agent"], "'unknown'");
    let topic = expr(cols, &["topic", "adr_summary"], "''");
    let solution = expr(cols, &["solution_summary", "adr_summary"], "''");
    let adr = expr(cols, &["adr_record", "adr_summary"], "''");
    let outcome = expr(cols, &["outcome"], "'success'");
    let related = expr(cols, &["related_task_id"], "NULL");
    let tags = expr(cols, &["tags_json"], "'[]'");
    let created = expr(cols, &["created_at"], "''");
    let embedding = if has_col(cols, "embedding") {
        "embedding"
    } else {
        "NULL"
    };
    let payload = expr(cols, &["payload_json"], "'{}'");
    let id = expr(cols, &["id"], "lower(hex(randomblob(16)))");
    let project = expr(cols, &["project_id"], "''");

    conn.execute(
        &format!(
            "INSERT INTO experiences (
                id, project_id, agent_id, topic, solution_summary, adr_record,
                outcome, related_task_id, tags_json, created_at, embedding, payload_json
            ) SELECT {id}, {project}, {agent}, {topic}, {solution}, {adr},
                {outcome}, {related}, {tags}, {created}, {embedding}, {payload}
            FROM experiences_legacy"
        ),
        [],
    )?;
    conn.execute("DROP TABLE experiences_legacy", [])?;
    Ok(())
}

fn prepare_record_embedding(mut record: ExperienceRecord) -> ExperienceRecord {
    if record.embedding.is_empty() {
        record.embedding = lexical_embedding(&record.search_text());
    }
    record
}

fn insert_record_blocking(conn: &Connection, record: &ExperienceRecord) -> Result<()> {
    let payload = serde_json::to_string(&record.to_lounge())?;
    let tags = serde_json::to_string(&record.tags)?;
    let outcome = serde_json::to_value(&record.outcome)?
        .as_str()
        .unwrap_or("success")
        .to_string();
    let embedding = if record.embedding.is_empty() {
        encode_embedding(&lexical_embedding(&record.search_text()))
    } else {
        encode_embedding(&record.embedding)
    };
    let status = if record.status.trim().is_empty() {
        crate::models::EXPERIENCE_STATUS_ACTIVE
    } else {
        record.status.as_str()
    };
    let updated_at = record
        .updated_at
        .clone()
        .unwrap_or_else(|| record.created_at.clone());
    let original = record.original_content.clone().unwrap_or_else(|| {
        if record.solution_summary.trim().is_empty() {
            record.topic.clone()
        } else {
            record.solution_summary.clone()
        }
    });
    let title = if record.title.trim().is_empty() {
        crate::bridge::hygiene::fallback_title(
            &record.solution_summary,
            crate::models::TITLE_MAX_CHARS,
        )
    } else {
        record.title.clone()
    };
    let title_source = record.title_source.as_str();
    let source = record.source.clone();

    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM experiences WHERE id = ?1",
            params![record.id],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);

    if exists {
        // Content upsert: never REPLACE. Preserve governance flags unless the
        // caller explicitly carries non-default pin/review/archive state.
        conn.execute(
            r#"
            UPDATE experiences SET
                project_id = ?1,
                agent_id = ?2,
                topic = ?3,
                solution_summary = ?4,
                adr_record = ?5,
                outcome = ?6,
                related_task_id = ?7,
                tags_json = ?8,
                embedding = ?9,
                payload_json = ?10,
                updated_at = ?11,
                original_content = COALESCE(original_content, ?12),
                title = CASE
                    WHEN title_source IN ('agent','user') AND length(trim(title)) > 0 THEN title
                    ELSE ?13
                END,
                title_source = CASE
                    WHEN title_source IN ('agent','user') AND length(trim(title)) > 0 THEN title_source
                    ELSE ?14
                END,
                source = CASE WHEN length(trim(source)) > 0 THEN source ELSE ?15 END
            WHERE id = ?16
            "#,
            params![
                record.project_id,
                record.agent_id,
                record.topic,
                record.solution_summary,
                record.adr_record,
                outcome,
                record.related_task_id,
                tags,
                embedding,
                payload,
                updated_at,
                original,
                title,
                title_source,
                source,
                record.id,
            ],
        )?;
    } else {
        conn.execute(
            r#"
            INSERT INTO experiences (
                id, project_id, agent_id, topic, solution_summary, adr_record,
                outcome, related_task_id, tags_json, created_at, embedding, payload_json,
                status, reviewed, use_count, last_used_at, archived_at, is_pinned,
                updated_at, original_content, archived_by, title, title_source, source
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                      ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)
            "#,
            params![
                record.id,
                record.project_id,
                record.agent_id,
                record.topic,
                record.solution_summary,
                record.adr_record,
                outcome,
                record.related_task_id,
                tags,
                record.created_at,
                embedding,
                payload,
                status,
                if record.reviewed { 1 } else { 0 },
                record.use_count as i64,
                record.last_used_at,
                record.archived_at,
                if record.is_pinned { 1 } else { 0 },
                updated_at,
                original,
                record.archived_by,
                title,
                title_source,
                source,
            ],
        )?;
        crate::db::journal::append_journal_best_effort(
            conn,
            crate::db::journal::JournalAppendInput::new(
                record.agent_id.clone(),
                crate::db::journal::ACTION_EXPERIENCE_RECORDED,
                serde_json::json!({
                    "experience_id": record.id,
                    "topic": record.topic,
                    "source": source,
                })
                .to_string(),
            )
            .with_project_id(Some(record.project_id.clone()))
            .with_task_id(record.related_task_id.clone().unwrap_or_default()),
        );
    }
    Ok(())
}

fn set_title_blocking(
    conn: &Connection,
    id: &str,
    title: &str,
    source: crate::models::TitleSource,
    force: bool,
) -> Result<()> {
    let existing: Option<(String, String)> = conn
        .query_row(
            "SELECT COALESCE(title_source,''), COALESCE(title,'') FROM experiences WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((cur_source, cur_title)) = existing else {
        anyhow::bail!("not found: {id}");
    };
    let cur = crate::models::TitleSource::parse(&cur_source);
    if !force
        && matches!(
            cur,
            crate::models::TitleSource::Agent | crate::models::TitleSource::User
        )
        && !cur_title.trim().is_empty()
    {
        return Ok(());
    }
    conn.execute(
        "UPDATE experiences SET title = ?1, title_source = ?2, updated_at = ?3 WHERE id = ?4",
        params![title, source.as_str(), now_rfc3339(), id],
    )?;
    Ok(())
}

fn list_needing_title_blocking(conn: &Connection) -> Result<Vec<(String, String)>> {
    // Include rows whose `title_next_retry_at` is still in the future so a process
    // restart can resume mid-backoff retries (not only due-now rows).
    let mut stmt = conn.prepare(
        r#"
        SELECT id, COALESCE(NULLIF(trim(solution_summary), ''), topic)
        FROM experiences
        WHERE COALESCE(status, 'active') = 'active'
          AND COALESCE(title_give_up, 0) = 0
          AND lower(COALESCE(title_source, 'fallback')) NOT IN ('agent', 'user', 'llm')
          AND (
            title IS NULL OR trim(title) = ''
            OR lower(COALESCE(title_source, 'fallback')) = 'fallback'
          )
        "#,
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn record_title_retry_blocking(
    conn: &Connection,
    id: &str,
    attempts: u32,
    next_retry_at: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE experiences SET title_attempts = ?1, title_next_retry_at = ?2, updated_at = ?3 WHERE id = ?4",
        params![attempts as i64, next_retry_at, now_rfc3339(), id],
    )?;
    Ok(())
}

fn mark_title_give_up_blocking(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE experiences SET title_give_up = 1, title_next_retry_at = NULL, updated_at = ?1 WHERE id = ?2",
        params![now_rfc3339(), id],
    )?;
    Ok(())
}

fn clear_title_retry_blocking(conn: &Connection, id: &str) -> Result<()> {
    conn.execute(
        "UPDATE experiences SET title_attempts = 0, title_next_retry_at = NULL, title_give_up = 0, updated_at = ?1 WHERE id = ?2",
        params![now_rfc3339(), id],
    )?;
    Ok(())
}

fn count_title_backlog_blocking(conn: &Connection) -> Result<u64> {
    // Same membership as `list_needing_title_blocking` (includes future next_retry_at).
    let n: i64 = conn.query_row(
        r#"
        SELECT COUNT(*) FROM experiences
        WHERE COALESCE(status, 'active') = 'active'
          AND COALESCE(title_give_up, 0) = 0
          AND lower(COALESCE(title_source, 'fallback')) NOT IN ('agent', 'user', 'llm')
          AND (
            title IS NULL OR trim(title) = ''
            OR lower(COALESCE(title_source, 'fallback')) = 'fallback'
          )
        "#,
        [],
        |row| row.get(0),
    )?;
    Ok(n.max(0) as u64)
}

fn count_title_give_ups_blocking(conn: &Connection) -> Result<u64> {
    let n: i64 = conn.query_row(
        r#"
        SELECT COUNT(*) FROM experiences
        WHERE COALESCE(title_give_up, 0) != 0
        "#,
        [],
        |row| row.get(0),
    )?;
    Ok(n.max(0) as u64)
}

fn settings_flag_done_blocking(conn: &Connection, key: &str) -> Result<bool> {
    let done: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?;
    Ok(matches!(
        done.as_deref(),
        Some("1") | Some("true") | Some("\"1\"")
    ))
}

fn set_settings_flag_blocking(conn: &Connection, key: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value_json) VALUES(?1, '1')
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![key],
    )?;
    Ok(())
}

const RECORD_SELECT_COLS: &str = r#"
    id, project_id, agent_id, topic, solution_summary, adr_record,
    outcome, related_task_id, tags_json, created_at, embedding, payload_json,
    status, reviewed, use_count, last_used_at, archived_at, is_pinned,
    updated_at, original_content, archived_by, title, title_source, source
"#;

fn get_record_blocking(conn: &Connection, id: &str) -> Result<Option<ExperienceRecord>> {
    let sql = format!("SELECT {RECORD_SELECT_COLS} FROM experiences WHERE id = ?1");
    let row = conn.query_row(&sql, params![id], map_record).optional()?;
    Ok(row)
}

/// Expand a project filter into exact identity keys: prefer_canonical, path
/// canonicalize, and every alias that maps to the resolved canonical (OPEN-5).
/// Never uses substring / LIKE matching.
fn resolve_experience_project_keys(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Option<Vec<String>>> {
    use crate::db::accepts_cross_platform_path;
    use crate::db::project_identity::{canonicalize_repo_path, prefer_canonical};
    use std::collections::BTreeSet;

    let Some(raw) = project_id.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };

    let mut keys = BTreeSet::new();
    keys.insert(raw.to_string());
    let preferred = prefer_canonical(conn, raw)?;
    keys.insert(preferred.clone());

    if accepts_cross_platform_path(raw) {
        let path_key = canonicalize_repo_path(raw);
        if !path_key.is_empty() {
            keys.insert(path_key.clone());
            keys.insert(prefer_canonical(conn, &path_key)?);
            if let Some(id) = conn
                .query_row(
                    "SELECT canonical_id FROM projects WHERE repo_path = ?1 LIMIT 1",
                    params![path_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
            {
                keys.insert(id);
            }
        }
    }

    // Pull aliases that point at any resolved canonical so alias-keyed rows match.
    let canon_list: Vec<String> = keys.iter().cloned().collect();
    for canonical in canon_list {
        let sql = if cfg!(target_os = "linux") {
            "SELECT alias_display FROM project_aliases WHERE canonical_id = ?1"
        } else {
            "SELECT alias_display FROM project_aliases WHERE canonical_id = ?1 COLLATE NOCASE"
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![canonical], |row| row.get::<_, String>(0))?;
        for row in rows {
            let alias = row?;
            if !alias.trim().is_empty() {
                keys.insert(alias);
            }
        }
    }

    Ok(Some(
        keys.into_iter().filter(|s| !s.trim().is_empty()).collect(),
    ))
}

fn project_id_match_clause(key_count: usize) -> String {
    debug_assert!(key_count > 0);
    if cfg!(target_os = "linux") {
        let placeholders = (0..key_count).map(|_| "?").collect::<Vec<_>>().join(", ");
        format!("project_id IN ({placeholders})")
    } else {
        // macOS / Windows: case-insensitive exact match (Project-ID-Core).
        let parts = (0..key_count)
            .map(|_| "project_id = ? COLLATE NOCASE")
            .collect::<Vec<_>>()
            .join(" OR ");
        format!("({parts})")
    }
}

fn latest_blocking(
    conn: &Connection,
    limit: usize,
    include_archived: bool,
    offset: usize,
    project_id: Option<&str>,
) -> Result<Vec<LoungeExperience>> {
    let keys = resolve_experience_project_keys(conn, project_id)?;
    let mut clauses: Vec<String> = Vec::new();
    if !include_archived {
        clauses.push("COALESCE(status, 'active') = 'active'".into());
    }
    if let Some(ref keys) = keys {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        // Exact identity match after prefer_canonical / alias expand — never LIKE.
        clauses.push(project_id_match_clause(keys.len()));
    }
    let where_clause = if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    };
    // Governance columns are the single source of truth — never prefer stale payload_json.
    let sql = format!(
        "SELECT {RECORD_SELECT_COLS}
         FROM experiences
         {where_clause}
         ORDER BY is_pinned DESC, created_at DESC LIMIT ? OFFSET ?"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = if let Some(ref keys) = keys {
        let mut bind: Vec<rusqlite::types::Value> = keys
            .iter()
            .map(|k| rusqlite::types::Value::Text(k.clone()))
            .collect();
        bind.push(rusqlite::types::Value::Integer(limit as i64));
        bind.push(rusqlite::types::Value::Integer(offset as i64));
        stmt.query_map(rusqlite::params_from_iter(bind), map_record)?
    } else {
        stmt.query_map(params![limit as i64, offset as i64], map_record)?
    };
    let mut out = Vec::new();
    for row in rows {
        out.push(row?.to_lounge());
    }
    Ok(out)
}

fn count_experiences_blocking(
    conn: &Connection,
    include_archived: bool,
    project_id: Option<&str>,
) -> Result<u64> {
    let keys = resolve_experience_project_keys(conn, project_id)?;
    let n: i64 = match (include_archived, keys.as_ref()) {
        (true, None) => conn.query_row("SELECT COUNT(*) FROM experiences", [], |row| row.get(0))?,
        (false, None) => conn.query_row(
            "SELECT COUNT(*) FROM experiences WHERE COALESCE(status, 'active') = 'active'",
            [],
            |row| row.get(0),
        )?,
        (_, Some(keys)) if keys.is_empty() => 0,
        (include_arch, Some(keys)) => {
            let mut clauses: Vec<String> = Vec::new();
            if !include_arch {
                clauses.push("COALESCE(status, 'active') = 'active'".into());
            }
            clauses.push(project_id_match_clause(keys.len()));
            let sql = format!(
                "SELECT COUNT(*) FROM experiences WHERE {}",
                clauses.join(" AND ")
            );
            let bind: Vec<rusqlite::types::Value> = keys
                .iter()
                .map(|k| rusqlite::types::Value::Text(k.clone()))
                .collect();
            conn.query_row(&sql, rusqlite::params_from_iter(bind), |row| row.get(0))?
        }
    };
    Ok(n.max(0) as u64)
}

fn similar_blocking(
    conn: &Connection,
    project_id: &str,
    query: &[f32],
    limit: usize,
) -> Result<Vec<ExperienceHit>> {
    let mut hits = score_rows(conn, Some(project_id), query)?;
    if hits.is_empty() {
        hits = score_rows(conn, None, query)?;
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit);
    Ok(hits)
}

const SAME_PROJECT_BOOST: f32 = 0.04;

fn similar_cross_blocking(
    conn: &Connection,
    project_id: &str,
    query: &[f32],
    limit: usize,
) -> Result<Vec<ExperienceHit>> {
    let mut hits = score_rows(conn, None, query)?;
    for hit in &mut hits {
        if hit.project_id == project_id {
            hit.score = (hit.score + SAME_PROJECT_BOOST).min(1.0);
        }
    }
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(limit);
    Ok(hits)
}

fn score_rows(
    conn: &Connection,
    project_id: Option<&str>,
    query: &[f32],
) -> Result<Vec<ExperienceHit>> {
    let sql = if project_id.is_some() {
        r#"
            SELECT id, project_id, agent_id, topic, solution_summary, adr_record, embedding
            FROM experiences
            WHERE project_id = ?1
              AND COALESCE(status, 'active') = 'active'
            "#
    } else {
        // Cross-Project Memory: başarılı tecrübe + ADR satırları (outcome=success).
        r#"
            SELECT id, project_id, agent_id, topic, solution_summary, adr_record, embedding
            FROM experiences
            WHERE lower(outcome) = 'success'
              AND COALESCE(status, 'active') = 'active'
            "#
    };
    let mut stmt = conn.prepare(sql)?;
    let mapped = if let Some(project_id) = project_id {
        stmt.query_map(params![project_id], map_hit_row)?
    } else {
        stmt.query_map([], map_hit_row)?
    };

    let mut hits = Vec::new();
    for row in mapped {
        let (mut hit, blob, topic, solution) = row?;
        let vector = blob
            .as_deref()
            .and_then(decode_embedding)
            .unwrap_or_else(|| lexical_embedding(&format!("{topic} {solution}")));
        let Some(score) = cosine_similarity(query, &vector) else {
            continue;
        };
        if score < MIN_COSINE {
            continue;
        }
        hit.score = score;
        hits.push(hit);
    }
    Ok(hits)
}

fn map_hit_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<(ExperienceHit, Option<Vec<u8>>, String, String)> {
    let topic: String = row.get(3)?;
    let solution: String = row.get(4)?;
    let hit = ExperienceHit {
        id: row.get(0)?,
        project_id: row.get(1)?,
        agent_id: row.get(2)?,
        topic: topic.clone(),
        solution_summary: solution.clone(),
        adr_record: row.get(5)?,
        score: 0.0,
        source: "sqlite".into(),
    };
    Ok((hit, row.get(6)?, topic, solution))
}

fn map_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ExperienceRecord> {
    map_record_from_parts(row, 0)
}

fn map_record_from_parts(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<ExperienceRecord> {
    let outcome: String = row.get(offset + 6)?;
    let tags_json: String = row.get(offset + 8)?;
    let blob: Option<Vec<u8>> = row.get(offset + 10)?;
    let status = row
        .get::<_, Option<String>>(offset + 12)?
        .unwrap_or_else(|| crate::models::EXPERIENCE_STATUS_ACTIVE.into());
    let reviewed = row.get::<_, i64>(offset + 13)?;
    let use_count = row.get::<_, i64>(offset + 14)?;
    let last_used_at = row.get::<_, Option<String>>(offset + 15)?;
    let archived_at = row.get::<_, Option<String>>(offset + 16)?;
    let is_pinned = row.get::<_, i64>(offset + 17)?;
    let updated_at = row.get::<_, Option<String>>(offset + 18)?;
    let original_content = row.get::<_, Option<String>>(offset + 19)?;
    let archived_by = row.get::<_, Option<String>>(offset + 20).unwrap_or(None);
    let title = row
        .get::<_, Option<String>>(offset + 21)?
        .unwrap_or_default();
    let title_source = row
        .get::<_, Option<String>>(offset + 22)?
        .unwrap_or_default();
    let source = row
        .get::<_, Option<String>>(offset + 23)?
        .unwrap_or_default();
    Ok(ExperienceRecord {
        id: row.get(offset)?,
        project_id: row.get(offset + 1)?,
        agent_id: row.get(offset + 2)?,
        topic: row.get(offset + 3)?,
        solution_summary: row.get(offset + 4)?,
        adr_record: row.get(offset + 5)?,
        outcome: serde_json::from_value(serde_json::Value::String(outcome))
            .unwrap_or(crate::models::ExperienceOutcome::Success),
        related_task_id: row.get(offset + 7)?,
        tags: serde_json::from_str(&tags_json).unwrap_or_default(),
        created_at: row.get(offset + 9)?,
        embedding: blob
            .as_deref()
            .and_then(decode_embedding)
            .unwrap_or_default(),
        status,
        reviewed: reviewed != 0,
        use_count: use_count.max(0) as u64,
        last_used_at,
        archived_at,
        archived_by,
        is_pinned: is_pinned != 0,
        updated_at,
        original_content,
        title,
        title_source: crate::models::TitleSource::parse(&title_source),
        source,
    })
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let found: Option<String> = conn
        .query_row(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1",
            params![name],
            |row| row.get(0),
        )
        .optional()?;
    Ok(found.is_some())
}

pub fn column_names(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row?);
    }
    Ok(names)
}

pub(crate) fn has_col(cols: &[String], name: &str) -> bool {
    cols.iter().any(|col| col == name)
}

fn expr(cols: &[String], names: &[&str], fallback_sql: &str) -> String {
    names
        .iter()
        .find(|name| has_col(cols, name))
        .map(|name| (*name).to_string())
        .unwrap_or_else(|| fallback_sql.to_string())
}

pub fn default_db_path(workspace_root: impl AsRef<Path>) -> PathBuf {
    workspace_root.as_ref().join("experiences/lounge.sqlite")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ExperienceOutcome, LoungeTask};

    #[tokio::test]
    async fn schema_has_requested_columns() {
        let store = ExperienceStore::memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let cols = column_names(&conn, "experiences").unwrap();
        for name in [
            "id",
            "project_id",
            "agent_id",
            "topic",
            "solution_summary",
            "adr_record",
            "title",
            "title_source",
            "source",
            "title_attempts",
            "title_next_retry_at",
            "title_give_up",
        ] {
            assert!(cols.iter().any(|col| col == name), "missing {name}");
        }
    }

    /// Prove SELECT/map offsets after Project-ID-Core + MCP-Quality column merge:
    /// `project_id`, `title`, `title_source`, and `source` round-trip intact.
    #[tokio::test]
    async fn title_source_and_project_id_roundtrip_offsets() {
        let store = ExperienceStore::memory().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let record = ExperienceRecord {
            id: id.clone(),
            project_id: "/canonical/repo/path".into(),
            agent_id: "cursor".into(),
            topic: "topic about redis".into(),
            solution_summary: "use connection pool".into(),
            adr_record: "Decision: pool".into(),
            title: "Redis pool fix".into(),
            title_source: crate::models::TitleSource::Agent,
            source: "mcp,external".into(),
            tags: vec!["redis".into()],
            outcome: ExperienceOutcome::Success,
            ..Default::default()
        };
        store.insert_record(record).await.unwrap();

        let loaded = store.get_record(id.clone()).await.unwrap().unwrap();
        assert_eq!(loaded.project_id, "/canonical/repo/path");
        assert_eq!(loaded.title, "Redis pool fix");
        assert_eq!(loaded.title_source, crate::models::TitleSource::Agent);
        assert_eq!(loaded.source, "mcp,external");
        assert_eq!(loaded.solution_summary, "use connection pool");
        assert_eq!(loaded.adr_record, "Decision: pool");
        assert_eq!(loaded.tags, vec!["redis"]);

        // Latest path uses the same RECORD_SELECT_COLS / map offsets.
        let latest = store.latest(5).await.unwrap();
        let card = latest.iter().find(|e| e.id == id).expect("in latest");
        assert_eq!(card.project_id, "/canonical/repo/path");
        assert_eq!(card.title, "Redis pool fix");
        assert_eq!(card.title_source, crate::models::TitleSource::Agent);
        assert_eq!(card.source, "mcp,external");
    }

    #[tokio::test]
    async fn persists_experience_payload() {
        let store = ExperienceStore::memory().unwrap();
        let task = LoungeTask::new("cursor", "agent-lounge-os", "save experience");
        let exp = LoungeExperience::from_task(
            &task,
            "SQLite tecrübe kaydı",
            ExperienceOutcome::Success,
            vec!["kernel".into()],
        );
        store.insert(&exp).await.unwrap();
        let loaded = store.get(exp.id.clone()).await.unwrap().unwrap();
        assert_eq!(loaded.msg_type, "experience");
        assert_eq!(loaded.related_task_id.as_deref(), Some(task.id.as_str()));
        assert_eq!(loaded.tags, vec!["kernel"]);
        let record = store.get_record(exp.id.clone()).await.unwrap().unwrap();
        assert_eq!(record.agent_id, "cursor");
        assert_eq!(record.topic, "SQLite tecrübe kaydı");
        assert_eq!(record.adr_record, "SQLite tecrübe kaydı");
    }

    #[tokio::test]
    async fn similar_topics_surface_prior_experience() {
        let store = ExperienceStore::memory().unwrap();
        let nats = LoungeTask::new("cursor", "agent-lounge-os", "NATS dispatcher dinleyici");
        store
            .insert_record(ExperienceRecord::from_task(
                &nats,
                "NATS dispatcher listen + spawn_blocking",
                "sync nats client blocking thread",
                ExperienceOutcome::Success,
                vec![],
            ))
            .await
            .unwrap();
        let rustc = LoungeTask::new("cursor", "agent-lounge-os", "dead code prune rustc flags");
        store
            .insert_record(ExperienceRecord::from_task(
                &rustc,
                "unused imports silindi",
                "cargo fix",
                ExperienceOutcome::Success,
                vec![],
            ))
            .await
            .unwrap();

        let hits = store
            .similar(
                "agent-lounge-os".into(),
                "dispatcher NATS mesajlarını dinle".into(),
                Some(2),
            )
            .await
            .unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].topic.to_ascii_lowercase().contains("nats"));
        assert!(hits[0].score >= MIN_COSINE);
    }

    #[tokio::test]
    async fn search_experiences_surfaces_nats_topic() {
        let store = ExperienceStore::memory().unwrap();
        let nats = LoungeTask::new("cursor", "agent-lounge-os", "NATS dispatcher dinleyici");
        store
            .insert_record(ExperienceRecord::from_task(
                &nats,
                "NATS dispatcher listen + spawn_blocking",
                "sync nats client blocking thread",
                ExperienceOutcome::Success,
                vec!["memory_bridge".into()],
            ))
            .await
            .unwrap();
        let other = LoungeTask::new("cursor", "echo-mind", "ollama tags probe");
        store
            .insert_record(ExperienceRecord::from_task(
                &other,
                "Ollama tags probe",
                "tags endpoint",
                ExperienceOutcome::Partial,
                vec![],
            ))
            .await
            .unwrap();

        let rows = store
            .search_experiences("dispatcher NATS".into(), Some(4))
            .await
            .unwrap();
        assert!(!rows.is_empty());
        let hay = rows
            .iter()
            .map(|row| row.adr_summary.to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            hay.contains("nats") || hay.contains("dispatcher"),
            "expected NATS/dispatcher hit, got {hay}"
        );
    }

    #[tokio::test]
    async fn migrates_legacy_agent_column() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE experiences (
                id TEXT PRIMARY KEY,
                type TEXT NOT NULL,
                agent TEXT NOT NULL,
                project_id TEXT NOT NULL,
                adr_summary TEXT NOT NULL,
                outcome TEXT NOT NULL,
                related_task_id TEXT,
                tags_json TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL,
                payload_json TEXT NOT NULL
            );
            INSERT INTO experiences VALUES (
                'legacy-1', 'experience', 'cursor', 'agent-lounge-os',
                'eski adr', 'success', null, '[]', '2026-09-18T00:00:00Z', '{}'
            );
            "#,
        )
        .unwrap();
        migrate_schema(&conn).unwrap();
        let cols = column_names(&conn, "experiences").unwrap();
        assert!(cols.iter().any(|col| col == "agent_id"));
        assert!(cols.iter().any(|col| col == "topic"));
        let agent_id: String = conn
            .query_row(
                "SELECT agent_id FROM experiences WHERE id = 'legacy-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(agent_id, "cursor");
    }

    #[tokio::test]
    async fn persists_routing_policy() {
        let store = ExperienceStore::memory().unwrap();
        let policy = crate::models::RoutingPolicy {
            on_quota_exhausted: crate::models::QuotaExhaustedAction::Stop,
            ..Default::default()
        };
        store.set_routing_policy(&policy).await.unwrap();
        let loaded = store.get_routing_policy().await.unwrap();
        assert_eq!(
            loaded.on_quota_exhausted,
            crate::models::QuotaExhaustedAction::Stop
        );
        assert!(loaded.require_user_approval);
    }

    #[tokio::test]
    async fn archive_filter_excludes_archived_from_default_paths() {
        let store = ExperienceStore::memory().unwrap();
        let topic = "unique-archive-filter-topic-zz";
        let active = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj-af", topic),
            "active solution about archive filter",
            "active adr archive filter",
            ExperienceOutcome::Success,
            vec!["archive-filter".into()],
        );
        let active_id = active.id.clone();
        store.insert_record(active.clone()).await.unwrap();

        let mut archived = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj-af", topic),
            "archived solution about archive filter",
            "archived adr archive filter",
            ExperienceOutcome::Success,
            vec!["archive-filter".into()],
        );
        let archived_id = archived.id.clone();
        archived.status = crate::models::EXPERIENCE_STATUS_ARCHIVED.into();
        store.insert_record(archived).await.unwrap();
        store.archive_experience(archived_id.clone()).await.unwrap();

        let similar = store
            .similar("proj-af".into(), topic.into(), Some(10))
            .await
            .unwrap();
        assert!(
            similar.iter().all(|h| h.id == active_id),
            "similar must only return active: {:?}",
            similar.iter().map(|h| &h.id).collect::<Vec<_>>()
        );
        assert!(!similar.iter().any(|h| h.id == archived_id));

        let searched = store
            .search_experiences("archive filter".into(), Some(20))
            .await
            .unwrap();
        assert!(searched.iter().any(|r| r.id == active_id));
        assert!(!searched.iter().any(|r| r.id == archived_id));

        let listed = store.list_experiences(50, false).await.unwrap();
        assert!(listed.iter().any(|r| r.id == active_id));
        assert!(!listed.iter().any(|r| r.id == archived_id));

        let with_archived = store.list_experiences(50, true).await.unwrap();
        assert!(with_archived.iter().any(|r| r.id == active_id));
        assert!(with_archived.iter().any(|r| r.id == archived_id));

        let ctx = store
            .fast_retrieve(crate::db::FastRetrieveQuery {
                project_id: "proj-af".into(),
                text: topic.into(),
                embedding: None,
                knowledge_hit: 0.55,
                ast_refs: Vec::new(),
                limit: Some(10),
            })
            .await
            .unwrap();
        assert!(
            ctx.experiences.iter().all(|h| h.id != archived_id),
            "fast_retrieve must exclude archived"
        );
    }

    #[tokio::test]
    async fn re_save_preserves_governance_flags() {
        let store = ExperienceStore::memory().unwrap();
        let mut record = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj-gov", "governance topic"),
            "solution",
            "adr body",
            ExperienceOutcome::Success,
            vec![],
        );
        let id = record.id.clone();
        store.insert_record(record.clone()).await.unwrap();
        store.pin_experience(id.clone(), true).await.unwrap();
        store.mark_experience_reviewed(id.clone()).await.unwrap();
        store.archive_experience(id.clone()).await.unwrap();

        let before = store.get_record(id.clone()).await.unwrap().unwrap();
        assert!(before.is_pinned);
        assert!(before.reviewed);
        assert_eq!(before.status, crate::models::EXPERIENCE_STATUS_ARCHIVED);

        // Full SELECT via list(include_archived) path must surface the row.
        let listed = store.list_experiences(20, true).await.unwrap();
        assert!(listed.iter().any(|r| r.id == id));
        let listed_active_only = store.list_experiences(20, false).await.unwrap();
        assert!(!listed_active_only.iter().any(|r| r.id == id));

        // Re-save content with default governance must not clobber flags.
        record.solution_summary = "updated solution".into();
        record.adr_record = "updated adr".into();
        record.status = crate::models::EXPERIENCE_STATUS_ACTIVE.into();
        record.is_pinned = false;
        record.reviewed = false;
        store.insert_record(record).await.unwrap();

        let after = store.get_record(id).await.unwrap().unwrap();
        assert!(after.is_pinned, "pin must survive re-save");
        assert!(after.reviewed, "reviewed must survive re-save");
        assert_eq!(
            after.status,
            crate::models::EXPERIENCE_STATUS_ARCHIVED,
            "archive status must survive re-save"
        );
        assert_eq!(after.solution_summary, "updated solution");
    }

    #[tokio::test]
    async fn list_and_search_read_governance_from_columns_not_payload() {
        let store = ExperienceStore::memory().unwrap();
        let record = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj-cols", "column truth topic"),
            "solution body",
            "adr body for columns",
            ExperienceOutcome::Success,
            vec!["gov".into()],
        );
        let id = record.id.clone();
        store.insert_record(record).await.unwrap();

        store.pin_experience(id.clone(), true).await.unwrap();
        store.mark_experience_reviewed(id.clone()).await.unwrap();
        store.archive_experience(id.clone()).await.unwrap();

        let listed = store.list_experiences(20, true).await.unwrap();
        let row = listed.iter().find(|r| r.id == id).expect("listed");
        assert!(row.is_pinned, "list must reflect pin column");
        assert!(row.reviewed, "list must reflect reviewed column");
        assert_eq!(row.status, crate::models::EXPERIENCE_STATUS_ARCHIVED);
        assert_eq!(
            row.archived_by.as_deref(),
            Some(crate::models::ARCHIVED_BY_USER)
        );

        let searched = store
            .search_experiences_filtered("column truth".into(), Some(20), None)
            .await
            .unwrap();
        let hit = searched.iter().find(|r| r.id == id).expect("searched");
        assert!(hit.is_pinned);
        assert!(hit.reviewed);
        assert_eq!(hit.status, crate::models::EXPERIENCE_STATUS_ARCHIVED);
    }

    #[tokio::test]
    async fn list_orders_pinned_first_and_count_supports_paging() {
        let store = ExperienceStore::memory().unwrap();
        for i in 0..5 {
            let mut record = ExperienceRecord::from_task(
                &LoungeTask::new("cursor", "proj-page", format!("topic {i}")),
                format!("sol {i}"),
                format!("adr {i}"),
                ExperienceOutcome::Success,
                vec![],
            );
            record.created_at = format!("2026-01-0{}T00:00:00+00:00", i + 1);
            record.updated_at = Some(record.created_at.clone());
            if i == 2 {
                record.is_pinned = true;
            }
            store.insert_record(record).await.unwrap();
        }
        store
            .pin_experience(
                store
                    .list_experiences(50, false)
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|r| r.adr_summary.contains("adr 2"))
                    .unwrap()
                    .id,
                true,
            )
            .await
            .unwrap();

        let page = store
            .list_experiences_page(2, false, 0, None)
            .await
            .unwrap();
        assert_eq!(page.len(), 2);
        assert!(page[0].is_pinned, "pinned row sorts first");
        let total = store.count_experiences(false, None).await.unwrap();
        assert_eq!(total, 5);
        let page2 = store
            .list_experiences_page(2, false, 2, None)
            .await
            .unwrap();
        assert_eq!(page2.len(), 2);
        assert!(!page2.iter().any(|r| r.is_pinned) || page2[0].id != page[0].id);

        // OPEN-5: project-scoped count/list use exact project_id match.
        let scoped = store
            .count_experiences(false, Some("proj-page".into()))
            .await
            .unwrap();
        assert_eq!(scoped, 5);
        let other = store
            .count_experiences(false, Some("other-proj".into()))
            .await
            .unwrap();
        assert_eq!(other, 0);
        let scoped_page = store
            .list_experiences_page(10, false, 0, Some("proj-page".into()))
            .await
            .unwrap();
        assert_eq!(scoped_page.len(), 5);
        let empty_page = store
            .list_experiences_page(10, false, 0, Some("other-proj".into()))
            .await
            .unwrap();
        assert!(empty_page.is_empty());
    }

    #[tokio::test]
    async fn count_and_list_resolve_alias_and_canonical_path() {
        use crate::db::project_identity::{canonicalize_repo_path, upsert_alias};

        let store = ExperienceStore::memory().unwrap();
        let repo = "/tmp/Agent-Lounge-OS-alias-test";
        let canonical = canonicalize_repo_path(repo);

        // Seed an experience under the slug alias (pre-canonical write shape).
        let record = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "agent-lounge-os", "alias topic"),
            "sol",
            "adr alias",
            ExperienceOutcome::Success,
            vec![],
        );
        let id = record.id.clone();
        store.insert_record(record).await.unwrap();

        // And one under the canonical path id.
        let mut canon_row = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", &canonical, "canonical topic"),
            "sol2",
            "adr canonical",
            ExperienceOutcome::Success,
            vec![],
        );
        canon_row.id = format!("{id}-canon");
        store.insert_record(canon_row).await.unwrap();

        {
            let conn = store.conn.lock().expect("db lock");
            upsert_alias(&conn, "agent-lounge-os", &canonical).unwrap();
        }

        // Query by alias → includes alias-keyed + canonical-keyed rows.
        let by_alias = store
            .count_experiences(false, Some("agent-lounge-os".into()))
            .await
            .unwrap();
        assert_eq!(
            by_alias, 2,
            "alias filter must expand to canonical + aliases"
        );

        let listed = store
            .list_experiences_page(10, false, 0, Some("agent-lounge-os".into()))
            .await
            .unwrap();
        assert_eq!(listed.len(), 2);

        // Query by canonical path → same expansion.
        let by_canon = store
            .count_experiences(false, Some(canonical.clone()))
            .await
            .unwrap();
        assert_eq!(by_canon, 2);

        // Unrelated project stays empty (exact match, never substring).
        let other = store
            .count_experiences(false, Some("agent-lounge".into()))
            .await
            .unwrap();
        assert_eq!(other, 0);
    }
}
