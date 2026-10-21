//! Vault-1: SQLite FTS5 index for Experiences + ProjectPages.
//!
//! - Tokenizer: `unicode61` (no ICU).
//! - Turkish folding is applied in Rust at **index** and **query** time via
//!   `al_turkish_fold` (symmetric: İ/I/i/ı → i, plus ş/s ç/c ğ/g ö/o ü/u).
//! - Hybrid score: `0.7 * semantic + 0.3 * lexical` with BM25 mapped to [0, 1]
//!   by `|bm25| / (1 + |bm25|)` (monotonic in relevance: more-negative BM25 →
//!   higher score). Missing semantic ⇒ `0.0` (hybrid = `0.3 * lexical`).
//! - Archived experiences (`status != 'active'`) are excluded at the FTS layer
//!   (triggers never index them; archive UPDATE deletes the FTS row).

use anyhow::{Context, Result};
use rusqlite::functions::FunctionFlags;
use rusqlite::{params, Connection, OptionalExtension};

use super::embedding::{cosine_similarity, decode_embedding, lexical_embedding};
use super::experience_governance::normalize_search_score;
use crate::models::{LoungeExperience, EXPERIENCE_STATUS_ACTIVE};

/// Settings key for the Vault FTS schema version.
pub const VAULT_FTS_SCHEMA_VERSION_KEY: &str = "vault_fts.schema_version";
/// Current Vault FTS schema version (string in settings).
pub const VAULT_FTS_SCHEMA_VERSION: &str = "1";

/// Semantic weight in the hybrid score.
pub const HYBRID_SEMANTIC_WEIGHT: f64 = 0.7;
/// Lexical (BM25-normalized) weight in the hybrid score.
pub const HYBRID_LEXICAL_WEIGHT: f64 = 0.3;

const FTS_TABLE: &str = "vault_fts";

/// Symmetric Turkish fold: case- and diacritic-insensitive for TR letters.
///
/// `İ`, `I`, `i`, and `ı` all become `i`. `ş/Ş→s`, `ç/Ç→c`, `ğ/Ğ→g`,
/// `ö/Ö→o`, `ü/Ü→u`. All other characters are lowercased with Unicode
/// simple case folding (no ICU).
pub fn turkish_fold(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            'İ' | 'I' | 'i' | 'ı' => out.push('i'),
            'Ş' | 'ş' => out.push('s'),
            'Ç' | 'ç' => out.push('c'),
            'Ğ' | 'ğ' => out.push('g'),
            'Ö' | 'ö' => out.push('o'),
            'Ü' | 'ü' => out.push('u'),
            c => {
                for lower in c.to_lowercase() {
                    out.push(lower);
                }
            }
        }
    }
    out
}

/// BM25 → [0, 1] via `|bm25| / (1 + |bm25|)`.
///
/// FTS5 `bm25()` is typically ≤ 0 (**more negative = better**). Absolute
/// value therefore grows with relevance, and this map is strictly
/// increasing in `|bm25|`: a strong hit like `-5.63` scores ≈ `0.85` while
/// a weak hit like `-0.27` scores ≈ `0.21`. Result-set independent (stable
/// for score-bucket ranking). Non-finite inputs clamp to 0.
pub fn normalize_bm25(bm25: f64) -> f64 {
    if !bm25.is_finite() {
        return 0.0;
    }
    let a = bm25.abs();
    a / (1.0 + a)
}

/// Hybrid relevance: `0.7 * semantic + 0.3 * lexical`.
///
/// When `semantic` is `None` it is treated as **0.0**, so the combined score
/// is `0.3 * normalize(lexical)`. Both components are passed through
/// [`normalize_search_score`] first.
pub fn hybrid_search_score(semantic: Option<f64>, lexical: f64) -> f64 {
    let lex = normalize_search_score(lexical);
    let sem = semantic.map(normalize_search_score).unwrap_or(0.0);
    HYBRID_SEMANTIC_WEIGHT * sem + HYBRID_LEXICAL_WEIGHT * lex
}

/// Register `al_turkish_fold`. Propagates registration errors (does not swallow).
///
/// SQLite replaces an existing function of the same name, so re-registering on
/// an already-prepared connection is fine and still returns `Ok`.
pub fn register_turkish_fold_fn(conn: &Connection) -> Result<()> {
    conn.create_scalar_function(
        "al_turkish_fold",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let input: String = ctx.get(0)?;
            Ok::<_, rusqlite::Error>(turkish_fold(&input))
        },
    )
    .context("register al_turkish_fold")?;
    Ok(())
}

/// Escape `%`, `_`, and `\` for SQLite `LIKE … ESCAPE '\'` (Vault #70).
pub fn like_pattern(needle: &str) -> String {
    let mut out = String::with_capacity(needle.len() + 2);
    out.push('%');
    for ch in needle.chars() {
        match ch {
            '\\' | '%' | '_' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.push('%');
    out
}

/// True when this bundled SQLite build exposes FTS5 (`ENABLE_FTS5`).
pub fn fts5_available(conn: &Connection) -> Result<bool> {
    let mut stmt = conn.prepare("PRAGMA compile_options")?;
    let opts = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for opt in opts {
        let opt = opt?;
        if opt == "ENABLE_FTS5" || opt.ends_with("=ENABLE_FTS5") {
            return Ok(true);
        }
    }
    // Fallback: try creating a temp FTS5 table.
    match conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS temp._fts5_probe USING fts5(x); DROP TABLE IF EXISTS temp._fts5_probe;",
    ) {
        Ok(()) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// Upgrade to Vault FTS schema v1: table, triggers, backfill. Idempotent.
pub fn migrate_vault_fts(conn: &Connection) -> Result<()> {
    register_turkish_fold_fn(conn)?;

    let current: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1",
            params![VAULT_FTS_SCHEMA_VERSION_KEY],
            |row| row.get(0),
        )
        .optional()?;
    if current.as_deref() == Some(VAULT_FTS_SCHEMA_VERSION) && fts_table_exists(conn)? {
        return Ok(());
    }

    // Rebuild cleanly so re-entry after a partial failure stays idempotent.
    drop_fts_objects(conn)?;
    create_fts_table(conn)?;
    create_fts_triggers(conn)?;
    backfill_fts(conn)?;

    conn.execute(
        "INSERT INTO settings(key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![VAULT_FTS_SCHEMA_VERSION_KEY, VAULT_FTS_SCHEMA_VERSION],
    )?;
    Ok(())
}

/// Downgrade: drop FTS table + triggers and clear the schema version flag.
pub fn downgrade_vault_fts(conn: &Connection) -> Result<()> {
    drop_fts_objects(conn)?;
    conn.execute(
        "DELETE FROM settings WHERE key = ?1",
        params![VAULT_FTS_SCHEMA_VERSION_KEY],
    )?;
    Ok(())
}

fn fts_table_exists(conn: &Connection) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![FTS_TABLE],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

fn drop_fts_objects(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        DROP TRIGGER IF EXISTS experiences_ai_vault_fts;
        DROP TRIGGER IF EXISTS experiences_au_vault_fts;
        DROP TRIGGER IF EXISTS experiences_ad_vault_fts;
        DROP TRIGGER IF EXISTS project_index_ai_vault_fts;
        DROP TRIGGER IF EXISTS project_index_au_vault_fts;
        DROP TRIGGER IF EXISTS project_index_ad_vault_fts;
        DROP TABLE IF EXISTS vault_fts;
        "#,
    )
    .context("drop vault_fts objects")?;
    Ok(())
}

fn create_fts_table(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE VIRTUAL TABLE vault_fts USING fts5(
          row_kind UNINDEXED,
          row_id UNINDEXED,
          project_id UNINDEXED,
          title,
          tags,
          source,
          adr_record,
          body,
          tokenize = 'unicode61'
        );
        "#,
    )
    .context("create vault_fts")?;
    Ok(())
}

fn create_fts_triggers(conn: &Connection) -> Result<()> {
    // Experiences: only active rows are indexed (archived excluded at FTS layer).
    conn.execute_batch(
        r#"
        CREATE TRIGGER experiences_ai_vault_fts AFTER INSERT ON experiences BEGIN
          INSERT INTO vault_fts(row_kind, row_id, project_id, title, tags, source, adr_record, body)
          SELECT
            'experience',
            NEW.id,
            NEW.project_id,
            al_turkish_fold(COALESCE(NEW.title, '')),
            al_turkish_fold(COALESCE(NEW.tags_json, '')),
            al_turkish_fold(COALESCE(NEW.source, '')),
            al_turkish_fold(COALESCE(NEW.adr_record, '')),
            al_turkish_fold(
              COALESCE(NEW.topic, '') || ' ' || COALESCE(NEW.solution_summary, '')
            )
          WHERE COALESCE(NEW.status, 'active') = 'active';
        END;

        CREATE TRIGGER experiences_ad_vault_fts AFTER DELETE ON experiences BEGIN
          DELETE FROM vault_fts
          WHERE row_kind = 'experience' AND row_id = OLD.id;
        END;

        CREATE TRIGGER experiences_au_vault_fts AFTER UPDATE ON experiences BEGIN
          DELETE FROM vault_fts
          WHERE row_kind = 'experience' AND row_id = OLD.id;
          INSERT INTO vault_fts(row_kind, row_id, project_id, title, tags, source, adr_record, body)
          SELECT
            'experience',
            NEW.id,
            NEW.project_id,
            al_turkish_fold(COALESCE(NEW.title, '')),
            al_turkish_fold(COALESCE(NEW.tags_json, '')),
            al_turkish_fold(COALESCE(NEW.source, '')),
            al_turkish_fold(COALESCE(NEW.adr_record, '')),
            al_turkish_fold(
              COALESCE(NEW.topic, '') || ' ' || COALESCE(NEW.solution_summary, '')
            )
          WHERE COALESCE(NEW.status, 'active') = 'active';
        END;

        CREATE TRIGGER project_index_ai_vault_fts AFTER INSERT ON project_index BEGIN
          INSERT INTO vault_fts(row_kind, row_id, project_id, title, tags, source, adr_record, body)
          SELECT
            'project_page',
            NEW.id,
            NEW.project_id,
            al_turkish_fold(COALESCE(NEW.name, '')),
            '',
            '',
            '',
            al_turkish_fold(
              COALESCE(NEW.file_path, '') || ' ' || COALESCE(NEW.detail, '')
            )
          WHERE NEW.kind = 'node';
        END;

        CREATE TRIGGER project_index_ad_vault_fts AFTER DELETE ON project_index BEGIN
          DELETE FROM vault_fts
          WHERE row_kind = 'project_page' AND row_id = OLD.id;
        END;

        CREATE TRIGGER project_index_au_vault_fts AFTER UPDATE ON project_index BEGIN
          DELETE FROM vault_fts
          WHERE row_kind = 'project_page' AND row_id = OLD.id;
          INSERT INTO vault_fts(row_kind, row_id, project_id, title, tags, source, adr_record, body)
          SELECT
            'project_page',
            NEW.id,
            NEW.project_id,
            al_turkish_fold(COALESCE(NEW.name, '')),
            '',
            '',
            '',
            al_turkish_fold(
              COALESCE(NEW.file_path, '') || ' ' || COALESCE(NEW.detail, '')
            )
          WHERE NEW.kind = 'node';
        END;
        "#,
    )
    .context("create vault_fts triggers")?;
    Ok(())
}

fn backfill_fts(conn: &Connection) -> Result<()> {
    // Experiences (active only).
    conn.execute(
        r#"
        INSERT INTO vault_fts(row_kind, row_id, project_id, title, tags, source, adr_record, body)
        SELECT
          'experience',
          id,
          project_id,
          al_turkish_fold(COALESCE(title, '')),
          al_turkish_fold(COALESCE(tags_json, '')),
          al_turkish_fold(COALESCE(source, '')),
          al_turkish_fold(COALESCE(adr_record, '')),
          al_turkish_fold(COALESCE(topic, '') || ' ' || COALESCE(solution_summary, ''))
        FROM experiences
        WHERE COALESCE(status, 'active') = 'active'
        "#,
        [],
    )
    .context("backfill experiences into vault_fts")?;

    // Project pages (index nodes).
    conn.execute(
        r#"
        INSERT INTO vault_fts(row_kind, row_id, project_id, title, tags, source, adr_record, body)
        SELECT
          'project_page',
          id,
          project_id,
          al_turkish_fold(COALESCE(name, '')),
          '',
          '',
          '',
          al_turkish_fold(COALESCE(file_path, '') || ' ' || COALESCE(detail, ''))
        FROM project_index
        WHERE kind = 'node'
        "#,
        [],
    )
    .context("backfill project_index into vault_fts")?;
    Ok(())
}

/// Build an FTS5 MATCH query from a user string (fold + prefix tokens).
///
/// Each whitespace-separated token is folded and emitted as a **prefix** term
/// (`token*`) so partial queries match longer indexed tokens (closer to the
/// previous LIKE `%needle%` UX for leading/stem hits) without ICU.
pub fn fts_match_query(raw: &str) -> Option<String> {
    let folded = turkish_fold(raw.trim());
    if folded.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    // Split on non-alphanumeric so hyphenated queries (`needle-target`) become
    // the same token stream unicode61 produces at index time.
    for token in folded.split(|c: char| !c.is_alphanumeric()) {
        if token.is_empty() {
            continue;
        }
        parts.push(format!("{token}*"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

/// FTS experience search with hybrid scoring (stored embedding cosine + normalized BM25).
///
/// Returns `(hit, hybrid_score)` pairs. The hybrid score is computed **once** here
/// and must be carried into [`super::experience_governance::sort_scored_experiences_for_search`]
/// — callers must not recompute LIKE/on-the-fly lexical scores.
pub fn search_experiences_fts(
    conn: &Connection,
    needle: &str,
    limit: usize,
    status: Option<&str>,
) -> Result<Vec<(LoungeExperience, f64)>> {
    register_turkish_fold_fn(conn)?;
    let Some(match_q) = fts_match_query(needle) else {
        return Ok(Vec::new());
    };

    let status_filter = status.unwrap_or(EXPERIENCE_STATUS_ACTIVE);
    let fetch_cap = (limit.saturating_mul(4)).clamp(limit, 256);
    let query_emb = lexical_embedding(needle);

    let sql = r#"
        SELECT
          e.id, e.project_id, e.agent_id, e.topic, e.solution_summary, e.adr_record,
          e.outcome, e.related_task_id, e.tags_json, e.created_at, e.embedding, e.payload_json,
          e.status, e.reviewed, e.use_count, e.last_used_at, e.archived_at, e.is_pinned,
          e.updated_at, e.original_content, e.archived_by, e.title, e.title_source, e.source,
          bm25(vault_fts) AS bm25_raw
        FROM vault_fts
        JOIN experiences e ON e.id = vault_fts.row_id
        WHERE vault_fts.row_kind = 'experience'
          AND vault_fts MATCH ?1
          AND COALESCE(e.status, 'active') = ?2
        ORDER BY bm25(vault_fts)
        LIMIT ?3
        "#;

    let mut stmt = conn.prepare(sql)?;
    let mapped = stmt.query_map(params![match_q, status_filter, fetch_cap as i64], |row| {
        let blob: Option<Vec<u8>> = row.get(10)?;
        let bm25: f64 = row.get(24)?;
        let record = super::experience_governance::map_lounge_row(row)?;
        Ok((record, bm25, blob))
    })?;

    let mut scored: Vec<(LoungeExperience, f64)> = Vec::new();
    for row in mapped {
        let (exp, bm25, blob) = row?;
        let lexical = normalize_bm25(bm25);
        let semantic = blob
            .as_deref()
            .and_then(decode_embedding)
            .filter(|emb| !emb.is_empty())
            .and_then(|emb| cosine_similarity(&query_emb, &emb))
            .map(|s| s as f64);
        // No stored embedding → semantic missing (None), not a zero vector guess.
        let hybrid = hybrid_search_score(semantic, lexical);
        scored.push((exp, hybrid));
    }

    super::experience_governance::sort_scored_experiences_for_search(&mut scored, None);
    scored.truncate(limit);
    Ok(scored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ExperienceStore;
    use crate::models::{now_rfc3339, ExperienceOutcome, ExperienceRecord, TitleSource};
    use std::time::Instant;

    const KIND_EXPERIENCE: &str = "experience";

    fn fts_row_count(conn: &Connection, kind: Option<&str>) -> Result<i64> {
        register_turkish_fold_fn(conn)?;
        match kind {
            Some(k) => Ok(conn.query_row(
                "SELECT COUNT(*) FROM vault_fts WHERE row_kind = ?1",
                params![k],
                |row| row.get(0),
            )?),
            None => Ok(conn.query_row("SELECT COUNT(*) FROM vault_fts", [], |row| row.get(0))?),
        }
    }

    fn fts_has_experience(conn: &Connection, id: &str) -> Result<bool> {
        register_turkish_fold_fn(conn)?;
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM vault_fts WHERE row_kind = ?1 AND row_id = ?2",
            params![KIND_EXPERIENCE, id],
            |row| row.get(0),
        )?;
        Ok(n > 0)
    }

    #[test]
    fn bundled_sqlite_exposes_fts5() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(
            fts5_available(&conn).unwrap(),
            "bundled rusqlite must ship ENABLE_FTS5 on every OS"
        );
        conn.execute_batch("CREATE VIRTUAL TABLE t USING fts5(x, tokenize = 'unicode61');")
            .unwrap();
    }

    #[test]
    fn turkish_fold_maps_dotted_dotless_i_and_diacritics() {
        assert_eq!(turkish_fold("İstanbul"), "istanbul");
        assert_eq!(turkish_fold("Istanbul"), "istanbul");
        assert_eq!(turkish_fold("ıstanbul"), "istanbul");
        assert_eq!(turkish_fold("istanbul"), "istanbul");
        assert_eq!(turkish_fold("ŞĞÜÇÖ"), "sguco");
        assert_eq!(turkish_fold("şğüçö"), "sguco");
        // Symmetric: all four I forms fold identically.
        let forms = ["İ", "I", "i", "ı"];
        let folded: Vec<_> = forms.iter().map(|s| turkish_fold(s)).collect();
        assert!(folded.iter().all(|f| f == "i"));
    }

    #[test]
    fn turkish_fold_property_idempotent_and_symmetric_i() {
        let samples = [
            "İstanbul",
            "ışık",
            "IŞIK",
            "Çalışma",
            "GÖRÜŞ",
            "öğün",
            "ASCII already",
            "",
            "a",
            "III",
            "iıİI",
        ];
        for s in samples {
            let once = turkish_fold(s);
            let twice = turkish_fold(&once);
            assert_eq!(once, twice, "fold must be idempotent for {s:?}");
        }

        // Property: any mix of İ/I/i/ı of equal length folds to the same string.
        let mut state: u64 = 0x00F0_1DCA_FEBE_EF42;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let alphabet = ['İ', 'I', 'i', 'ı'];
        for _ in 0..200 {
            let len = (next() % 12) as usize + 1;
            let mut a = String::new();
            let mut b = String::new();
            for _ in 0..len {
                a.push(alphabet[(next() as usize) % 4]);
                b.push(alphabet[(next() as usize) % 4]);
            }
            assert_eq!(turkish_fold(&a), turkish_fold(&b));
            assert_eq!(turkish_fold(&a), "i".repeat(len));
        }
    }

    #[test]
    fn normalize_bm25_maps_to_unit_interval_monotonic_in_relevance() {
        // More-negative BM25 (better) → higher normalized score.
        assert!((normalize_bm25(0.0) - 0.0).abs() < 1e-12);
        assert!(normalize_bm25(-5.63) > normalize_bm25(-0.27));
        assert!(normalize_bm25(-2.0) > normalize_bm25(-0.5));
        assert!((normalize_bm25(-5.63) - (5.63 / 6.63)).abs() < 1e-9);
        assert!((normalize_bm25(-0.27) - (0.27 / 1.27)).abs() < 1e-9);
        assert_eq!(normalize_bm25(f64::NAN), 0.0);
        assert_eq!(normalize_bm25(f64::INFINITY), 0.0);
        let v = normalize_bm25(-10.0);
        assert!((0.0..=1.0).contains(&v));
    }

    #[tokio::test]
    async fn strong_bm25_match_ranks_above_weak() {
        let store = seed_store();
        // Strong: query tokens dominate title/ADR.
        store
            .insert_record(sample_record(
                "strong",
                "unique-bm25-needle unique-bm25-needle",
                "active",
            ))
            .await
            .unwrap();
        // Weak: single token buried in a long unrelated body.
        let mut weak = sample_record(
            "weak",
            "lorem ipsum dolor sit amet consectetur adipiscing elit sed do",
            "active",
        );
        weak.adr_record = "lorem ipsum unique-bm25-needle padding only once".into();
        weak.topic = "unrelated topic padding".into();
        weak.title = "unrelated title padding".into();
        weak.solution_summary = "padding solution body with filler words".into();
        weak.embedding = lexical_embedding(&weak.topic);
        store.insert_record(weak).await.unwrap();

        let conn = store.conn.lock().expect("lock");
        let hits = search_experiences_fts(&conn, "unique-bm25-needle", 10, Some("active")).unwrap();
        assert!(hits.len() >= 2, "expected both hits: {hits:?}");
        assert_eq!(
            hits[0].0.id, "strong",
            "strong BM25 must rank first: {hits:?}"
        );
        assert!(
            hits[0].1 > hits[1].1,
            "hybrid score must reflect BM25 order: strong={} weak={}",
            hits[0].1,
            hits[1].1
        );
    }

    #[test]
    fn hybrid_score_with_and_without_semantic() {
        let lex = 1.0;
        let with = hybrid_search_score(Some(1.0), lex);
        assert!((with - 1.0).abs() < 1e-12);
        let missing = hybrid_search_score(None, lex);
        assert!(
            (missing - HYBRID_LEXICAL_WEIGHT).abs() < 1e-12,
            "missing semantic must behave as 0.0 → 0.3 * lexical, got {missing}"
        );
        let mid = hybrid_search_score(Some(0.5), 0.5);
        assert!((mid - 0.5).abs() < 1e-12);
    }

    fn seed_store() -> ExperienceStore {
        ExperienceStore::memory().unwrap()
    }

    fn sample_record(id: &str, title: &str, status: &str) -> ExperienceRecord {
        ExperienceRecord {
            id: id.into(),
            project_id: "proj".into(),
            agent_id: "agent".into(),
            topic: title.into(),
            solution_summary: format!("solution for {title}"),
            adr_record: format!("adr {title}"),
            outcome: ExperienceOutcome::Success,
            related_task_id: None,
            tags: vec!["tag1".into()],
            created_at: now_rfc3339(),
            embedding: lexical_embedding(title),
            status: status.into(),
            reviewed: true,
            use_count: 0,
            last_used_at: None,
            archived_at: None,
            is_pinned: false,
            updated_at: Some(now_rfc3339()),
            original_content: None,
            archived_by: None,
            title: title.into(),
            title_source: TitleSource::Fallback,
            source: "mcp".into(),
        }
    }

    #[tokio::test]
    async fn migration_backfill_is_idempotent() {
        let store = seed_store();
        store
            .insert_record(sample_record("e1", "NATS subject", "active"))
            .await
            .unwrap();
        store
            .insert_record(sample_record("e2", "archived one", "archived"))
            .await
            .unwrap();

        let conn = store.conn.lock().expect("lock");
        let before = fts_row_count(&conn, Some(KIND_EXPERIENCE)).unwrap();
        assert!(before >= 1);
        assert!(fts_has_experience(&conn, "e1").unwrap());
        assert!(!fts_has_experience(&conn, "e2").unwrap());

        migrate_vault_fts(&conn).unwrap();
        migrate_vault_fts(&conn).unwrap();
        let after = fts_row_count(&conn, Some(KIND_EXPERIENCE)).unwrap();
        assert_eq!(before, after, "re-migrate must not duplicate FTS rows");
    }

    #[tokio::test]
    async fn triggers_sync_insert_update_delete_and_exclude_archived() {
        let store = seed_store();
        store
            .insert_record(sample_record("t1", "Çalışma ışık", "active"))
            .await
            .unwrap();
        {
            let conn = store.conn.lock().expect("lock");
            assert!(fts_has_experience(&conn, "t1").unwrap());
            // Query-time fold: dotted İ matches indexed folded text.
            let hits = search_experiences_fts(&conn, "calisma", 10, Some("active")).unwrap();
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].0.id, "t1");
        }

        // Archive → removed from FTS.
        store.archive_experience("t1".into()).await.unwrap();
        {
            let conn = store.conn.lock().expect("lock");
            assert!(!fts_has_experience(&conn, "t1").unwrap());
            let hits = search_experiences_fts(&conn, "calisma", 10, Some("active")).unwrap();
            assert!(hits.is_empty());
        }

        // Restore → back in FTS.
        store.unarchive_experience("t1".into()).await.unwrap();
        {
            let conn = store.conn.lock().expect("lock");
            assert!(fts_has_experience(&conn, "t1").unwrap());
        }

        // Delete → gone.
        store.delete_record("t1".into()).await.unwrap();
        {
            let conn = store.conn.lock().expect("lock");
            assert!(!fts_has_experience(&conn, "t1").unwrap());
        }
    }

    #[tokio::test]
    async fn title_tags_source_adr_are_searchable() {
        let store = seed_store();
        let mut rec = sample_record("f1", "generic-topic", "active");
        rec.title = "UniquTitleX".into();
        rec.tags = vec!["UniquTagY".into()];
        rec.source = "UniquSourceZ".into();
        rec.adr_record = "UniquAdrW decision".into();
        store.insert_record(rec).await.unwrap();

        let conn = store.conn.lock().expect("lock");
        for needle in ["UniquTitleX", "UniquTagY", "UniquSourceZ", "UniquAdrW"] {
            let hits = search_experiences_fts(&conn, needle, 5, Some("active")).unwrap();
            assert_eq!(hits.len(), 1, "expected hit for {needle}");
            assert_eq!(hits[0].0.id, "f1");
        }
    }

    #[tokio::test]
    async fn turkish_query_matches_indexed_diacritics() {
        let store = seed_store();
        store
            .insert_record(sample_record("tr1", "Öğün şişe", "active"))
            .await
            .unwrap();
        let conn = store.conn.lock().expect("lock");
        // ASCII-folded query finds diacritic document (symmetric fold).
        let hits = search_experiences_fts(&conn, "ogun sise", 5, Some("active")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0.id, "tr1");
        // Dotted capital İ in query matches.
        let hits2 = search_experiences_fts(&conn, "ÖĞÜN", 5, Some("active")).unwrap();
        assert_eq!(hits2.len(), 1);
    }

    #[test]
    fn downgrade_drops_fts_and_triggers() {
        let store = seed_store();
        {
            let conn = store.conn.lock().expect("lock");
            assert!(fts_table_exists(&conn).unwrap());
            downgrade_vault_fts(&conn).unwrap();
            assert!(!fts_table_exists(&conn).unwrap());
            let ver: Option<String> = conn
                .query_row(
                    "SELECT value_json FROM settings WHERE key = ?1",
                    params![VAULT_FTS_SCHEMA_VERSION_KEY],
                    |row| row.get(0),
                )
                .optional()
                .unwrap();
            assert!(ver.is_none());
            let trig: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='trigger' AND name LIKE '%vault_fts'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(trig, 0);
            // Re-upgrade works.
            migrate_vault_fts(&conn).unwrap();
            assert!(fts_table_exists(&conn).unwrap());
        }
    }

    #[tokio::test]
    async fn fts_search_p95_under_50ms_at_10k() {
        let store = seed_store();
        // Bulk insert via SQL for speed; triggers keep FTS in sync.
        {
            let conn = store.conn.lock().expect("lock");
            let tx = conn.unchecked_transaction().unwrap();
            for i in 0..10_000 {
                let title = if i % 500 == 0 {
                    format!("needle-target-{i}")
                } else {
                    format!("doc-padding-{i}-lorem")
                };
                tx.execute(
                    r#"
                    INSERT INTO experiences (
                      id, project_id, agent_id, topic, solution_summary, adr_record,
                      outcome, tags_json, created_at, embedding, payload_json,
                      status, reviewed, title, title_source, source
                    ) VALUES (?1, 'p', 'a', ?2, ?2, ?2, 'success', '[]', ?3, NULL, '{}',
                              'active', 1, ?2, 'fallback', '')
                    "#,
                    params![format!("id-{i}"), title, now_rfc3339()],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }

        let conn = store.conn.lock().expect("lock");
        // Warmup.
        for _ in 0..5 {
            let _ = search_experiences_fts(&conn, "needle-target", 20, Some("active")).unwrap();
        }

        let mut samples = Vec::with_capacity(40);
        for _ in 0..40 {
            let start = Instant::now();
            let hits = search_experiences_fts(&conn, "needle-target", 20, Some("active")).unwrap();
            samples.push(start.elapsed());
            assert!(!hits.is_empty(), "perf fixture must yield hits");
        }
        samples.sort();
        let idx = ((samples.len() as f64) * 0.95).ceil() as usize - 1;
        let p95 = samples[idx.min(samples.len() - 1)];
        assert!(
            p95.as_millis() < 50,
            "p95 FTS search at 10k must be < 50ms, got {}ms",
            p95.as_millis()
        );
    }

    #[tokio::test]
    async fn project_page_indexed_and_searchable() {
        let store = seed_store();
        {
            let conn = store.conn.lock().expect("lock");
            conn.execute(
                r#"
                INSERT INTO project_index (
                  id, project_id, repo_path, kind, name, file_path, line,
                  ref_count, detail, payload_json, indexed_at
                ) VALUES (
                  'n1', 'demo', '/tmp/demo', 'node', 'ÇalışanSembol',
                  '/tmp/demo/ışık.rs', 1, 0, 'detail', '{}', ?1
                )
                "#,
                params![now_rfc3339()],
            )
            .unwrap();
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM vault_fts WHERE row_kind = 'project_page' AND row_id = 'n1'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(n, 1);
            let match_q = fts_match_query("calisan").unwrap();
            let hit: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM vault_fts WHERE row_kind = 'project_page' AND vault_fts MATCH ?1",
                    params![match_q],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(hit >= 1);
        }
    }
}
