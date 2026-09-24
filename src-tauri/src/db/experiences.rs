use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::embedding::{cosine_similarity, decode_embedding, encode_embedding, lexical_embedding};
use crate::models::{ExperienceHit, ExperienceRecord, LoungeExperience, RoutingPolicy};

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

    pub async fn insert(&self, experience: &LoungeExperience) -> Result<()> {
        let record = ExperienceRecord::from_lounge(experience, experience.adr_summary.clone());
        self.insert_record(record).await
    }

    pub async fn insert_record(&self, record: ExperienceRecord) -> Result<()> {
        let conn = self.conn.clone();
        let for_vector = record.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            insert_record_blocking(&conn, &record)
        })
        .await
        .context("experience insert join")??;
        super::vector_memory::spawn_upsert(for_vector);
        Ok(())
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
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            latest_blocking(&conn, limit)
        })
        .await
        .context("experience latest join")?
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
                payload_json TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_experiences_project ON experiences(project_id);
            CREATE INDEX IF NOT EXISTS idx_experiences_agent ON experiences(agent_id);
            CREATE INDEX IF NOT EXISTS idx_experiences_topic ON experiences(project_id, topic);
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
    crate::db::connected_tools::migrate_connected_tools(conn)?;
    crate::db::project_index::migrate_project_index(conn)?;
    crate::db::feedback::migrate_feedback(conn)?;
    let exists = table_exists(conn, "experiences")?;
    if !exists {
        conn.execute_batch(create_experiences_sql())?;
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

    conn.execute(
        r#"
            INSERT OR REPLACE INTO experiences (
                id, project_id, agent_id, topic, solution_summary, adr_record,
                outcome, related_task_id, tags_json, created_at, embedding, payload_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
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
        ],
    )?;
    Ok(())
}

fn get_record_blocking(conn: &Connection, id: &str) -> Result<Option<ExperienceRecord>> {
    let row = conn
        .query_row(
            r#"
            SELECT id, project_id, agent_id, topic, solution_summary, adr_record,
                   outcome, related_task_id, tags_json, created_at, embedding, payload_json
            FROM experiences WHERE id = ?1
            "#,
            params![id],
            map_record,
        )
        .optional()?;
    Ok(row)
}

fn latest_blocking(conn: &Connection, limit: usize) -> Result<Vec<LoungeExperience>> {
    let mut stmt = conn.prepare(
        "SELECT payload_json, id, project_id, agent_id, topic, solution_summary, adr_record,
                outcome, related_task_id, tags_json, created_at, embedding
         FROM experiences ORDER BY created_at DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |row| {
        Ok((row.get::<_, String>(0)?, map_record_from_parts(row, 1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (payload, record) = row?;
        if let Ok(experience) = serde_json::from_str::<LoungeExperience>(&payload) {
            out.push(experience);
        } else {
            out.push(record.to_lounge());
        }
    }
    Ok(out)
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
            FROM experiences WHERE project_id = ?1
            "#
    } else {
        // Cross-Project Memory: başarılı tecrübe + ADR satırları (outcome=success).
        r#"
            SELECT id, project_id, agent_id, topic, solution_summary, adr_record, embedding
            FROM experiences
            WHERE lower(outcome) = 'success'
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

fn column_names(conn: &Connection, table: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row?);
    }
    Ok(names)
}

fn has_col(cols: &[String], name: &str) -> bool {
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
        ] {
            assert!(cols.iter().any(|col| col == name), "missing {name}");
        }
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
}
