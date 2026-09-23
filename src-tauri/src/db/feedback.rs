//! Kullanıcı geri bildirimi — Security onay / Context Whisper faydalı tıklaması.
//!
//! DecisionGate skorlaması Laya ağırlıklarını yeniden eğitmez; `apply_user_bias`
//! sınıflandırma sonrası küçük bir User Bias düzeltmesi uygular.

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::kernel::decision_engine::{
    apply_user_bias, DecisionResult, SecurityLevel, UserBiasStats, USER_BIAS_APPROVE_THRESHOLD,
};
use crate::models::now_rfc3339;

pub const KIND_SECURITY_APPROVE: &str = "security_approve";
pub const KIND_SECURITY_DENY: &str = "security_deny";
pub const KIND_WHISPER_USEFUL: &str = "whisper_useful";

/// Son N gün içindeki deny'ler "recent" sayılır; varsa bias uygulanmaz.
pub const RECENT_DENY_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeedbackEvent {
    pub id: String,
    pub kind: String,
    /// Güvenlik sınıfı: `Risky` / `Critical` (whisper için boş veya proje etiketi).
    pub alert_type: String,
    /// Araç / konu / özet ipucu — gruplama için.
    pub subject: String,
    pub task_id: Option<String>,
    pub experience_id: Option<String>,
    pub project_id: Option<String>,
    pub created_at: String,
}

pub fn migrate_feedback(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS feedback_events (
            id TEXT PRIMARY KEY,
            kind TEXT NOT NULL,
            alert_type TEXT NOT NULL DEFAULT '',
            subject TEXT NOT NULL DEFAULT '',
            task_id TEXT,
            experience_id TEXT,
            project_id TEXT,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_feedback_kind_alert
            ON feedback_events(kind, alert_type);
        CREATE INDEX IF NOT EXISTS idx_feedback_created
            ON feedback_events(created_at);
        "#,
    )?;
    Ok(())
}

pub fn insert_event(conn: &Connection, event: &FeedbackEvent) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO feedback_events (
            id, kind, alert_type, subject, task_id, experience_id, project_id, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
        params![
            event.id,
            event.kind,
            event.alert_type,
            event.subject,
            event.task_id,
            event.experience_id,
            event.project_id,
            event.created_at,
        ],
    )?;
    Ok(())
}

pub fn record_security_vote(
    conn: &Connection,
    approve: bool,
    alert_type: SecurityLevel,
    task_id: &str,
    subject: &str,
) -> Result<FeedbackEvent> {
    let event = FeedbackEvent {
        id: uuid_v4(),
        kind: if approve {
            KIND_SECURITY_APPROVE.into()
        } else {
            KIND_SECURITY_DENY.into()
        },
        alert_type: alert_type.as_str().into(),
        subject: subject.into(),
        task_id: Some(task_id.into()),
        experience_id: None,
        project_id: None,
        created_at: now_rfc3339(),
    };
    insert_event(conn, &event)?;
    Ok(event)
}

pub fn record_whisper_useful(
    conn: &Connection,
    experience_id: &str,
    project_id: Option<&str>,
    task_id: Option<&str>,
) -> Result<FeedbackEvent> {
    let event = FeedbackEvent {
        id: uuid_v4(),
        kind: KIND_WHISPER_USEFUL.into(),
        alert_type: "whisper".into(),
        subject: experience_id.into(),
        task_id: task_id.map(str::to_string),
        experience_id: Some(experience_id.into()),
        project_id: project_id.map(str::to_string),
        created_at: now_rfc3339(),
    };
    insert_event(conn, &event)?;
    Ok(event)
}

/// Belirli güvenlik sınıfı için User Bias istatistikleri.
pub fn security_bias_stats(conn: &Connection, alert_type: SecurityLevel) -> Result<UserBiasStats> {
    let label = alert_type.as_str();
    let approve_count: u32 = conn
        .query_row(
            r#"
        SELECT COUNT(*) FROM feedback_events
        WHERE kind = ?1 AND alert_type = ?2
        "#,
            params![KIND_SECURITY_APPROVE, label],
            |row| row.get::<_, i64>(0),
        )?
        .try_into()
        .unwrap_or(0);

    let cutoff = recent_deny_cutoff();
    let recent_denies: u32 = conn
        .query_row(
            r#"
        SELECT COUNT(*) FROM feedback_events
        WHERE kind = ?1 AND alert_type = ?2 AND created_at >= ?3
        "#,
            params![KIND_SECURITY_DENY, label, cutoff],
            |row| row.get::<_, i64>(0),
        )?
        .try_into()
        .unwrap_or(0);

    Ok(UserBiasStats {
        alert_type,
        approve_count,
        recent_denies,
    })
}

/// Sınıflandırma sonrası bias uygula (DB okumalı).
pub fn apply_stored_user_bias(conn: &Connection, result: DecisionResult) -> Result<DecisionResult> {
    let level = result.security.value;
    if matches!(level, SecurityLevel::Safe) {
        return Ok(result);
    }
    let stats = security_bias_stats(conn, level)?;
    Ok(apply_user_bias(result, &stats))
}

fn recent_deny_cutoff() -> String {
    let secs = RECENT_DENY_DAYS.saturating_mul(86_400);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_sub(secs as u64))
        .unwrap_or(0);
    // RFC3339 yaklaşık cutoff — lexicographic karşılaştırma için yeterli.
    chrono_like_rfc3339(ts)
}

fn chrono_like_rfc3339(unix_secs: u64) -> String {
    // Basit UTC format (harici chrono bağımlılığı yok).
    let days = unix_secs / 86_400;
    let rem = unix_secs % 86_400;
    let hours = rem / 3600;
    let mins = (rem % 3600) / 60;
    let secs = rem % 60;
    // 1970-01-01 + days → kabaca yıl/ay/gün (leap yok; yalnızca cutoff sıralaması).
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{hours:02}:{mins:02}:{secs:02}Z")
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    // Howard Hinnant algorithms (proleptic Gregorian).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

fn uuid_v4() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("fb-{nanos:x}")
}

/// ExperienceStore üzerinden senkron okuma (DecisionGate / test).
pub fn with_conn_bias(
    store: &crate::db::ExperienceStore,
    result: DecisionResult,
) -> DecisionResult {
    let Ok(conn) = store.conn.lock() else {
        return result;
    };
    apply_stored_user_bias(&conn, result.clone()).unwrap_or(result)
}

pub fn with_conn_record_security(
    store: &crate::db::ExperienceStore,
    approve: bool,
    alert_type: SecurityLevel,
    task_id: &str,
    subject: &str,
) -> Result<FeedbackEvent> {
    let conn = store.conn.lock().expect("experience db lock");
    record_security_vote(&conn, approve, alert_type, task_id, subject)
}

pub fn with_conn_record_whisper(
    store: &crate::db::ExperienceStore,
    experience_id: &str,
    project_id: Option<&str>,
    task_id: Option<&str>,
) -> Result<FeedbackEvent> {
    let conn = store.conn.lock().expect("experience db lock");
    record_whisper_useful(&conn, experience_id, project_id, task_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ExperienceStore;
    use crate::kernel::decision_engine::{RecallHint, RoutingType, Scored};
    use std::collections::HashMap;

    fn decision(level: SecurityLevel) -> DecisionResult {
        let key = level.as_str().to_string();
        DecisionResult {
            message_id: "m1".into(),
            subject: "lounge.task.requested".into(),
            routing: Scored {
                value: RoutingType::Task,
                confidence: 0.9,
                probabilities: HashMap::from([("Task".into(), 0.9)]),
            },
            security: Scored {
                value: level,
                confidence: 0.92,
                probabilities: HashMap::from([(key, 0.92)]),
            },
            knowledge_hit: 0.2,
            elapsed_ms: 2,
            elapsed_us: 2_000,
            device: "cpu".into(),
            recall: RecallHint::default(),
        }
    }

    #[test]
    fn approve_persists_a_row() {
        let store = ExperienceStore::memory().unwrap();
        let event = with_conn_record_security(
            &store,
            true,
            SecurityLevel::Risky,
            "task-1",
            "touch secrets",
        )
        .unwrap();
        assert_eq!(event.kind, KIND_SECURITY_APPROVE);
        assert_eq!(event.alert_type, "Risky");
        assert_eq!(event.task_id.as_deref(), Some("task-1"));

        let conn = store.conn.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM feedback_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn three_risky_approves_lower_next_risky_score() {
        let store = ExperienceStore::memory().unwrap();
        for i in 0..USER_BIAS_APPROVE_THRESHOLD {
            with_conn_record_security(
                &store,
                true,
                SecurityLevel::Risky,
                &format!("t-{i}"),
                "risky tool",
            )
            .unwrap();
        }
        let lowered = with_conn_bias(&store, decision(SecurityLevel::Risky));
        assert_eq!(lowered.security.value, SecurityLevel::Safe);
        assert!(USER_BIAS_APPROVE_THRESHOLD >= 3);
    }

    #[test]
    fn zero_history_does_not_lower_critical() {
        let store = ExperienceStore::memory().unwrap();
        let unchanged = with_conn_bias(&store, decision(SecurityLevel::Critical));
        assert_eq!(unchanged.security.value, SecurityLevel::Critical);
    }

    #[test]
    fn whisper_useful_persists() {
        let store = ExperienceStore::memory().unwrap();
        let event =
            with_conn_record_whisper(&store, "exp-whisper-1", Some("agent-lounge-os"), None)
                .unwrap();
        assert_eq!(event.kind, KIND_WHISPER_USEFUL);
        assert_eq!(event.experience_id.as_deref(), Some("exp-whisper-1"));
        assert_eq!(event.project_id.as_deref(), Some("agent-lounge-os"));

        let conn = store.conn.lock().unwrap();
        let kind: String = conn
            .query_row(
                "SELECT kind FROM feedback_events WHERE experience_id = ?1",
                params!["exp-whisper-1"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kind, KIND_WHISPER_USEFUL);
    }

    #[test]
    fn recent_deny_blocks_bias() {
        let store = ExperienceStore::memory().unwrap();
        for i in 0..3 {
            with_conn_record_security(&store, true, SecurityLevel::Risky, &format!("a-{i}"), "x")
                .unwrap();
        }
        with_conn_record_security(&store, false, SecurityLevel::Risky, "deny-1", "x").unwrap();
        let kept = with_conn_bias(&store, decision(SecurityLevel::Risky));
        assert_eq!(kept.security.value, SecurityLevel::Risky);
    }
}
