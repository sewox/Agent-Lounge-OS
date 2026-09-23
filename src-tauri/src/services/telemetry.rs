//! Ajan Verimlilik Raporu (Agent Efficiency Report).
//!
//! DecisionGate `lounge.telemetry.decision` / latency UI'dan ayrıdır.
//! Kaynaklar: `experiences` (failure), `telemetry_whisper_hits` (cross-project
//! fısıltı), `telemetry_dead_snapshots` + mevcut ölü sembol sayısı.

use anyhow::{Context, Result};
use chrono::{Duration, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::ExperienceStore;
use crate::kernel::decision_engine::DecisionResult;
use crate::models::{now_rfc3339, SystemPromptAddon};

const WHISPER_TABLE: &str = "telemetry_whisper_hits";
const DEAD_TABLE: &str = "telemetry_dead_snapshots";

/// Haftalık (son 7 gün) ve/veya proje kapsamı.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct EfficiencyReportQuery {
    /// `true` → son 7 gün penceresi.
    #[serde(default = "default_weekly")]
    pub weekly: bool,
    /// Boş / None → tüm projeler.
    #[serde(default)]
    pub project_id: Option<String>,
}

fn default_weekly() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct AgentFailureRow {
    pub agent_id: String,
    /// `experiences.outcome = failure` satır sayısı (uydurma "bug found" değil).
    pub failure_count: u64,
    pub metric_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct DeadCleanupStats {
    pub remaining: u64,
    pub cleaned: Option<u64>,
    /// `cleaned / (cleaned + remaining)` — yalnızca önce/sonra snapshot varken.
    pub rate: Option<f64>,
    pub previous_count: Option<u64>,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub struct AgentEfficiencyReport {
    pub generated_at: String,
    pub scope_label: String,
    pub weekly: bool,
    pub project_id: Option<String>,
    pub window_start: Option<String>,
    pub window_end: String,
    pub failures_by_agent: Vec<AgentFailureRow>,
    pub total_failures: u64,
    /// Kaç fısıltı yayınında en az bir çapraz-proje tecrübe enjekte edildi.
    pub cross_project_whisper_events: u64,
    /// Enjekte edilen çapraz-proje tecrübe satırı sayısı.
    pub cross_project_experience_hits: u64,
    pub dead: DeadCleanupStats,
    pub markdown: String,
    pub source_notes: Vec<String>,
}

pub fn migrate_telemetry(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS telemetry_whisper_hits (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            target_project_id TEXT NOT NULL,
            experience_id TEXT NOT NULL,
            experience_project_id TEXT NOT NULL,
            cross_project INTEGER NOT NULL DEFAULT 0,
            injected_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_telemetry_whisper_at
            ON telemetry_whisper_hits(injected_at);
        CREATE INDEX IF NOT EXISTS idx_telemetry_whisper_project
            ON telemetry_whisper_hits(target_project_id);

        CREATE TABLE IF NOT EXISTS telemetry_dead_snapshots (
            id TEXT PRIMARY KEY,
            project_id TEXT NOT NULL,
            dead_count INTEGER NOT NULL,
            recorded_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_telemetry_dead_project
            ON telemetry_dead_snapshots(project_id, recorded_at);
        "#,
    )?;
    let _ = (WHISPER_TABLE, DEAD_TABLE);
    Ok(())
}

fn with_conn<T>(store: &ExperienceStore, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let conn = store.conn.lock().expect("experience db lock");
    migrate_telemetry(&conn)?;
    f(&conn)
}

/// `inject_knowledge_hit` sonrası: enjekte edilen tecrübeleri hafif sayaç tablosuna yazar.
pub async fn record_whisper_injection(
    store: &ExperienceStore,
    result: &DecisionResult,
    addon: &SystemPromptAddon,
) -> Result<u64> {
    let task_id = addon.task_id.clone();
    let target_project = result.recall.project_id.clone();
    let injected_at = now_rfc3339();
    let rows: Vec<(String, String, i64)> = addon
        .context
        .experiences
        .iter()
        .map(|hit| {
            let cross = if !target_project.is_empty() && hit.project_id != target_project {
                1
            } else {
                0
            };
            (hit.id.clone(), hit.project_id.clone(), cross)
        })
        .collect();

    let conn = store.conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        migrate_telemetry(&conn)?;
        let mut written = 0u64;
        for (experience_id, experience_project_id, cross) in rows {
            let id = Uuid::new_v4().to_string();
            conn.execute(
                r#"
                INSERT INTO telemetry_whisper_hits (
                    id, task_id, target_project_id, experience_id,
                    experience_project_id, cross_project, injected_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
                params![
                    id,
                    task_id,
                    target_project,
                    experience_id,
                    experience_project_id,
                    cross,
                    injected_at,
                ],
            )?;
            written += 1;
        }
        Ok(written)
    })
    .await
    .context("telemetry whisper join")?
}

/// Index / dead list sonrası ölü sembol sayısı snapshot'ı.
pub async fn record_dead_snapshot(
    store: &ExperienceStore,
    project_id: impl Into<String>,
    dead_count: u64,
) -> Result<()> {
    let project_id = project_id.into();
    if project_id.trim().is_empty() {
        return Ok(());
    }
    let recorded_at = now_rfc3339();
    let conn = store.conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        migrate_telemetry(&conn)?;
        let id = Uuid::new_v4().to_string();
        conn.execute(
            r#"
            INSERT INTO telemetry_dead_snapshots (id, project_id, dead_count, recorded_at)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![id, project_id, dead_count as i64, recorded_at],
        )?;
        Ok(())
    })
    .await
    .context("telemetry dead snapshot join")?
}

pub async fn build_agent_efficiency_report(
    store: &ExperienceStore,
    query: EfficiencyReportQuery,
) -> Result<AgentEfficiencyReport> {
    let conn = store.conn.clone();
    tokio::task::spawn_blocking(move || {
        let conn = conn.lock().expect("experience db lock");
        migrate_telemetry(&conn)?;
        build_report_blocking(&conn, &query)
    })
    .await
    .context("efficiency report join")?
}

fn build_report_blocking(
    conn: &Connection,
    query: &EfficiencyReportQuery,
) -> Result<AgentEfficiencyReport> {
    let now = Utc::now();
    let window_end = now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let window_start = if query.weekly {
        Some((now - Duration::days(7)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
    } else {
        None
    };
    let project = query
        .project_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let failures = failures_by_agent(conn, project, window_start.as_deref())?;
    let total_failures: u64 = failures.iter().map(|r| r.failure_count).sum();

    let (whisper_events, whisper_hits) =
        cross_project_counts(conn, project, window_start.as_deref())?;

    let dead = dead_cleanup_stats(conn, project)?;

    let scope_label = match (query.weekly, project) {
        (true, Some(p)) => format!("haftalık + proje ({p})"),
        (true, None) => "haftalık (son 7 gün)".into(),
        (false, Some(p)) => format!("proje ({p})"),
        (false, None) => "tüm zamanlar".into(),
    };

    let mut source_notes = vec![
        "Hata/failure: experiences.outcome = 'failure' (ajan bazlı grup). 'Bug found' sayacı yok."
            .into(),
        "Cross-Project: telemetry_whisper_hits (fısıltı enjeksiyonunda kaydedilir).".into(),
        "Ölü sembol: mevcut project_index dead/broken + telemetry_dead_snapshots önce/sonra."
            .into(),
    ];
    if whisper_hits == 0 && whisper_events == 0 {
        source_notes.push(
            "Henüz fısıltı kaydı yok — DecisionGate MATCH sonrası enjeksiyonlar biriktikçe dolar."
                .into(),
        );
    }
    if dead.rate.is_none() {
        source_notes.push(
            "Temizleme oranı yok: önceki snapshot yok veya sayı düşmemiş (uydurma % yok).".into(),
        );
    }

    let mut report = AgentEfficiencyReport {
        generated_at: window_end.clone(),
        scope_label,
        weekly: query.weekly,
        project_id: project.map(str::to_string),
        window_start: window_start.clone(),
        window_end,
        failures_by_agent: failures,
        total_failures,
        cross_project_whisper_events: whisper_events,
        cross_project_experience_hits: whisper_hits,
        dead,
        markdown: String::new(),
        source_notes,
    };
    report.markdown = render_markdown(&report);
    Ok(report)
}

fn failures_by_agent(
    conn: &Connection,
    project: Option<&str>,
    since: Option<&str>,
) -> Result<Vec<AgentFailureRow>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT agent_id, COUNT(*) AS n
        FROM experiences
        WHERE lower(outcome) = 'failure'
          AND (?1 IS NULL OR project_id = ?1)
          AND (?2 IS NULL OR created_at >= ?2)
        GROUP BY agent_id
        ORDER BY n DESC, agent_id ASC
        "#,
    )?;
    let rows = stmt.query_map(params![project, since], |row| {
        Ok(AgentFailureRow {
            agent_id: row.get::<_, String>(0)?,
            failure_count: row.get::<_, i64>(1)? as u64,
            metric_label: "hata/failure sayısı".into(),
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn cross_project_counts(
    conn: &Connection,
    project: Option<&str>,
    since: Option<&str>,
) -> Result<(u64, u64)> {
    let hits: i64 = conn.query_row(
        r#"
        SELECT COUNT(*)
        FROM telemetry_whisper_hits
        WHERE cross_project = 1
          AND (?1 IS NULL OR target_project_id = ?1)
          AND (?2 IS NULL OR injected_at >= ?2)
        "#,
        params![project, since],
        |row| row.get(0),
    )?;
    let events: i64 = conn.query_row(
        r#"
        SELECT COUNT(DISTINCT task_id)
        FROM telemetry_whisper_hits
        WHERE cross_project = 1
          AND (?1 IS NULL OR target_project_id = ?1)
          AND (?2 IS NULL OR injected_at >= ?2)
        "#,
        params![project, since],
        |row| row.get(0),
    )?;
    Ok((events as u64, hits as u64))
}

fn current_dead_count(conn: &Connection, project: Option<&str>) -> Result<u64> {
    let count: i64 = if let Some(pid) = project {
        conn.query_row(
            r#"
            SELECT COUNT(*) FROM project_index
            WHERE kind IN ('dead', 'broken') AND project_id = ?1
            "#,
            params![pid],
            |row| row.get(0),
        )?
    } else {
        conn.query_row(
            r#"
            SELECT COUNT(*) FROM project_index
            WHERE kind IN ('dead', 'broken')
            "#,
            [],
            |row| row.get(0),
        )?
    };
    Ok(count as u64)
}

fn previous_dead_peak(conn: &Connection, project: Option<&str>) -> Result<Option<u64>> {
    let peak: Option<i64> = if let Some(pid) = project {
        conn.query_row(
            r#"
            SELECT MAX(dead_count) FROM telemetry_dead_snapshots
            WHERE project_id = ?1
            "#,
            params![pid],
            |row| row.get(0),
        )?
    } else {
        conn.query_row(
            r#"
            SELECT MAX(dead_count) FROM telemetry_dead_snapshots
            "#,
            [],
            |row| row.get(0),
        )?
    };
    Ok(peak.map(|n| n as u64))
}

fn dead_cleanup_stats(conn: &Connection, project: Option<&str>) -> Result<DeadCleanupStats> {
    let remaining = current_dead_count(conn, project)?;
    let peak = previous_dead_peak(conn, project)?;
    match peak {
        Some(prev) if prev > remaining => {
            let cleaned = prev - remaining;
            let denom = cleaned + remaining;
            let rate = if denom > 0 {
                Some(cleaned as f64 / denom as f64)
            } else {
                None
            };
            Ok(DeadCleanupStats {
                remaining,
                cleaned: Some(cleaned),
                rate,
                previous_count: Some(prev),
                note: "Peak snapshot − mevcut: cleaned / (cleaned + remaining).".into(),
            })
        }
        Some(prev) => Ok(DeadCleanupStats {
            remaining,
            cleaned: Some(0),
            rate: None,
            previous_count: Some(prev),
            note: "Snapshot peak mevcut sayıdan yüksek değil; oran uydurulmadı.".into(),
        }),
        None => Ok(DeadCleanupStats {
            remaining,
            cleaned: None,
            rate: None,
            previous_count: None,
            note: "Yalnızca mevcut ölü sembol sayısı (önceki snapshot yok).".into(),
        }),
    }
}

pub fn render_markdown(report: &AgentEfficiencyReport) -> String {
    let mut md = String::new();
    md.push_str("# Agent Efficiency Report / Ajan Verimlilik Raporu\n\n");
    md.push_str(&format!("- Üretilme: {}\n", report.generated_at));
    md.push_str(&format!("- Kapsam: {}\n", report.scope_label));
    if let Some(start) = &report.window_start {
        md.push_str(&format!("- Pencere: {start} → {}\n", report.window_end));
    }
    if let Some(pid) = &report.project_id {
        md.push_str(&format!("- Proje: `{pid}`\n"));
    }
    md.push('\n');

    md.push_str("## 1. Ajan başına hata / failure\n\n");
    md.push_str("Metrik: `experiences.outcome = failure` (uydurma bug sayacı değil).\n\n");
    if report.failures_by_agent.is_empty() {
        md.push_str("_Kayıt yok._\n\n");
    } else {
        md.push_str("| Ajan | Hata/failure |\n| --- | ---: |\n");
        for row in &report.failures_by_agent {
            md.push_str(&format!("| {} | {} |\n", row.agent_id, row.failure_count));
        }
        md.push_str(&format!("\n**Toplam:** {}\n\n", report.total_failures));
    }

    md.push_str("## 2. Cross-Project Experience kullanımı\n\n");
    md.push_str(&format!(
        "- Fısıltı olayları (en az 1 çapraz-proje tecrübe): **{}**\n",
        report.cross_project_whisper_events
    ));
    md.push_str(&format!(
        "- Enjekte edilen çapraz-proje tecrübe satırı: **{}**\n\n",
        report.cross_project_experience_hits
    ));

    md.push_str("## 3. Ölü sembol (Dead Symbol) temizleme\n\n");
    md.push_str(&format!(
        "- Kalan (mevcut): **{}**\n",
        report.dead.remaining
    ));
    match report.dead.cleaned {
        Some(c) => md.push_str(&format!("- Temizlenen (önceki − mevcut): **{c}**\n")),
        None => md.push_str("- Temizlenen: _hesaplanamadı (önceki snapshot yok)_\n"),
    }
    match report.dead.rate {
        Some(r) => md.push_str(&format!(
            "- Oran cleaned/(cleaned+remaining): **{:.1}%**\n",
            r * 100.0
        )),
        None => md.push_str("- Oran: _yok (sahte % üretilmedi)_\n"),
    }
    md.push_str(&format!("- Not: {}\n\n", report.dead.note));

    md.push_str("## Kaynak notları\n\n");
    for note in &report.source_notes {
        md.push_str(&format!("- {note}\n"));
    }
    md.push('\n');
    md.push_str("---\n_Agent Lounge OS · Markdown dışa aktarım (PDF için yazdırın)._\n");
    md
}

/// PDF yok: Markdown yeterli; tarayıcıda yazdır → PDF.
pub fn export_format_note() -> &'static str {
    "markdown"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::decision_engine::{
        DecisionResult, RecallHint, RoutingType, Scored, SecurityLevel,
    };
    use crate::models::{
        ExperienceContext, ExperienceHit, ExperienceOutcome, ExperienceRecord, LoungeTask,
    };
    use std::collections::HashMap;

    fn scored<T: Clone>(value: T) -> Scored<T> {
        Scored {
            value: value.clone(),
            confidence: 0.9,
            probabilities: HashMap::new(),
        }
    }

    fn seed_failure(
        store: &ExperienceStore,
        agent: &str,
        project: &str,
        outcome: ExperienceOutcome,
    ) {
        let task = LoungeTask::new(agent, project, "fixture failure task");
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            store
                .insert_record(ExperienceRecord::from_task(
                    &task,
                    "fixture adr",
                    "fixture solution",
                    outcome,
                    vec![],
                ))
                .await
                .unwrap();
        });
    }

    #[test]
    fn aggregates_failures_by_agent_from_fixtures() {
        let store = ExperienceStore::memory().unwrap();
        seed_failure(&store, "cursor", "lounge", ExperienceOutcome::Failure);
        seed_failure(&store, "cursor", "lounge", ExperienceOutcome::Failure);
        seed_failure(&store, "claude", "lounge", ExperienceOutcome::Failure);
        seed_failure(&store, "cursor", "other", ExperienceOutcome::Success);

        let report = with_conn(&store, |conn| {
            build_report_blocking(
                conn,
                &EfficiencyReportQuery {
                    weekly: false,
                    project_id: Some("lounge".into()),
                },
            )
        })
        .unwrap();

        assert_eq!(report.total_failures, 3);
        assert_eq!(report.failures_by_agent.len(), 2);
        assert_eq!(report.failures_by_agent[0].agent_id, "cursor");
        assert_eq!(report.failures_by_agent[0].failure_count, 2);
        assert_eq!(
            report.failures_by_agent[0].metric_label,
            "hata/failure sayısı"
        );
        assert!(report.markdown.contains("cursor"));
        assert!(report.markdown.contains("hata / failure"));
    }

    #[test]
    fn counts_cross_project_whisper_hits() {
        let store = ExperienceStore::memory().unwrap();
        let result = DecisionResult {
            message_id: "m1".into(),
            subject: crate::models::TASK_REQUESTED.into(),
            routing: scored(RoutingType::Task),
            security: scored(SecurityLevel::Safe),
            knowledge_hit: 0.9,
            elapsed_ms: 1,
            elapsed_us: 1_000,
            device: "cpu".into(),
            recall: RecallHint {
                query: "nats".into(),
                project_id: "lounge".into(),
                source_agent: "cursor".into(),
                target_agent: None,
                ast_refs: vec![],
            },
        };
        let addon = SystemPromptAddon::from_context(
            "m1",
            "cursor",
            0.9,
            ExperienceContext {
                experiences: vec![
                    ExperienceHit {
                        id: "e-cross".into(),
                        project_id: "sister-os".into(),
                        agent_id: "claude".into(),
                        topic: "nats".into(),
                        solution_summary: "spawn_blocking".into(),
                        adr_record: "adr".into(),
                        score: 0.8,
                        source: "sqlite".into(),
                    },
                    ExperienceHit {
                        id: "e-same".into(),
                        project_id: "lounge".into(),
                        agent_id: "cursor".into(),
                        topic: "local".into(),
                        solution_summary: "local".into(),
                        adr_record: "adr".into(),
                        score: 0.5,
                        source: "sqlite".into(),
                    },
                ],
                ..Default::default()
            },
        );

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let n = record_whisper_injection(&store, &result, &addon)
                .await
                .unwrap();
            assert_eq!(n, 2);
        });

        let report = with_conn(&store, |conn| {
            build_report_blocking(
                conn,
                &EfficiencyReportQuery {
                    weekly: true,
                    project_id: Some("lounge".into()),
                },
            )
        })
        .unwrap();
        assert_eq!(report.cross_project_experience_hits, 1);
        assert_eq!(report.cross_project_whisper_events, 1);
    }

    #[test]
    fn dead_rate_only_when_previous_snapshot_higher() {
        let store = ExperienceStore::memory().unwrap();
        with_conn(&store, |conn| {
            migrate_telemetry(conn)?;
            for (i, name) in ["a", "b", "c"].iter().enumerate() {
                conn.execute(
                    r#"
                    INSERT INTO project_index (
                        id, project_id, repo_path, kind, name, file_path, line,
                        target, ref_count, detail, payload_json, indexed_at
                    ) VALUES (?1, 'lounge', '/tmp', 'dead', ?2, NULL, NULL, NULL, 0, NULL, '{}', ?3)
                    "#,
                    params![format!("d{i}"), name, now_rfc3339()],
                )?;
            }
            conn.execute(
                r#"
                INSERT INTO telemetry_dead_snapshots (id, project_id, dead_count, recorded_at)
                VALUES ('snap-old', 'lounge', 10, '2026-01-01T00:00:00.000Z')
                "#,
                [],
            )?;
            Ok(())
        })
        .unwrap();

        let report = with_conn(&store, |conn| {
            build_report_blocking(
                conn,
                &EfficiencyReportQuery {
                    weekly: false,
                    project_id: Some("lounge".into()),
                },
            )
        })
        .unwrap();

        assert_eq!(report.dead.remaining, 3);
        assert_eq!(report.dead.previous_count, Some(10));
        assert_eq!(report.dead.cleaned, Some(7));
        let rate = report.dead.rate.expect("rate");
        assert!((rate - (7.0 / 10.0)).abs() < 1e-9);
        assert!(report.markdown.contains("Temizlenen"));
    }

    #[test]
    fn no_fake_rate_without_history() {
        let store = ExperienceStore::memory().unwrap();
        let report = with_conn(&store, |conn| {
            build_report_blocking(
                conn,
                &EfficiencyReportQuery {
                    weekly: true,
                    project_id: None,
                },
            )
        })
        .unwrap();
        assert!(report.dead.rate.is_none());
        assert!(report.dead.cleaned.is_none() || report.dead.cleaned == Some(0));
        assert!(report.markdown.contains("sahte % üretilmedi") || report.markdown.contains("yok"));
    }
}
