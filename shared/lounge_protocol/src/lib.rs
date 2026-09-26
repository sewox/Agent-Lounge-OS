//! Lounge bus zarfı — NATS `lounge.>` üzerindeki her mesaj bu struct ile taşınır.

pub mod schema;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use schema::{validate as validate_schema, SchemaKind};

pub const WILDCARD: &str = "lounge.>";
pub const UI_EVENT: &str = "nats-event";
pub const BUS_CONNECTED: &str = "lounge.bus.connected";
pub const BUS_HEARTBEAT: &str = "lounge.bus.heartbeat";
pub const BUS_PROBE: &str = "lounge.bus.probe";

pub const WORKERS_REGISTER: &str = "lounge.workers.register";
pub const WORKERS_UNREGISTER: &str = "lounge.workers.unregister";
pub const WORKERS_HEARTBEAT: &str = "lounge.workers.heartbeat";
pub const AGENT_HEARTBEAT: &str = "lounge.agent.heartbeat";
pub const AGENT_STATUS: &str = "lounge.agent.status";
/// Per-bot görev kutusu öneki — tam konu: `lounge.tasks.<bot_id>`.
pub const TASKS_INBOX_PREFIX: &str = "lounge.tasks.";

/// `lounge.tasks.<bot_id>` — yalnızca `[a-z0-9_-]` kabul eder.
pub fn worker_tasks_subject(bot_id: &str) -> Option<String> {
    let id = bot_id.trim().to_ascii_lowercase();
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        || !id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(format!("{TASKS_INBOX_PREFIX}{id}"))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LoungeMessage {
    pub id: String,
    #[serde(rename = "type")]
    pub msg_type: String,
    pub subject: String,
    pub source_agent: String,
    #[serde(default)]
    pub target_agent: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub payload: serde_json::Value,
    #[serde(default)]
    pub payload_bytes: usize,
}

impl LoungeMessage {
    pub fn new(
        subject: impl Into<String>,
        source_agent: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        let subject = subject.into();
        let bytes = serde_json::to_vec(&payload).map(|v| v.len()).unwrap_or(0);
        Self {
            id: Uuid::new_v4().to_string(),
            msg_type: type_from_subject(&subject),
            subject,
            source_agent: source_agent.into(),
            target_agent: Some("bus".into()),
            created_at: now_rfc3339(),
            payload,
            payload_bytes: bytes,
        }
    }

    /// Ham NATS gövdesini zarfa çevirir; zaten `LoungeMessage` ise olduğu gibi kullanır.
    pub fn from_nats(subject: impl Into<String>, data: &[u8]) -> Self {
        let subject = subject.into();
        if let Ok(mut envelope) = serde_json::from_slice::<LoungeMessage>(data) {
            if envelope.id.trim().is_empty() {
                envelope.id = Uuid::new_v4().to_string();
            }
            if envelope.subject.trim().is_empty() {
                envelope.subject = subject;
            }
            envelope.payload_bytes = data.len();
            return envelope;
        }

        let payload: serde_json::Value = serde_json::from_slice(data).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(data).into_owned())
        });
        let source = string_field(&payload, &["source_agent", "agent", "from"])
            .unwrap_or_else(|| "nats".into());
        let target = string_field(&payload, &["target_agent", "to"]).or_else(|| {
            payload
                .pointer("/task/target_agent")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
        let created = string_field(&payload, &["created_at"]).unwrap_or_else(now_rfc3339);
        let msg_type =
            string_field(&payload, &["type"]).unwrap_or_else(|| type_from_subject(&subject));
        let id = string_field(&payload, &["id"]).unwrap_or_else(|| Uuid::new_v4().to_string());

        Self {
            id,
            msg_type,
            subject,
            source_agent: source,
            target_agent: target,
            created_at: created,
            payload,
            payload_bytes: data.len(),
        }
    }

    pub fn state(&self) -> &'static str {
        if self.subject.ends_with(".failed") {
            "error"
        } else if self.subject.ends_with(".requested") {
            "queued"
        } else if self.subject.contains("heartbeat") || self.subject.contains("probe") {
            "ok"
        } else {
            "ok"
        }
    }

    pub fn payload_label(&self) -> String {
        format!("{:.1}kb", self.payload_bytes as f32 / 1024.0)
    }

    pub fn clock(&self) -> String {
        if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&self.created_at) {
            return parsed.format("%H:%M:%S%.3f").to_string();
        }
        Utc::now().format("%H:%M:%S%.3f").to_string()
    }
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn type_from_subject(subject: &str) -> String {
    let mut parts = subject.split('.');
    parts.next();
    parts.next().unwrap_or("log").to_string()
}

fn string_field(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_raw_task_payload() {
        let raw = serde_json::json!({
            "id": "11111111-1111-4111-8111-111111111111",
            "type": "task",
            "source_agent": "cursor",
            "target_agent": "lounge-kernel",
            "project_id": "agent-lounge-os",
            "summary": "index dispatcher",
            "created_at": "2026-09-18T12:00:00.000Z"
        });
        let bytes = serde_json::to_vec(&raw).unwrap();
        let msg = LoungeMessage::from_nats("lounge.task.requested", &bytes);
        assert_eq!(msg.source_agent, "cursor");
        assert_eq!(msg.target_agent.as_deref(), Some("lounge-kernel"));
        assert_eq!(msg.msg_type, "task");
        assert_eq!(msg.subject, "lounge.task.requested");
        assert_eq!(msg.state(), "queued");
    }

    #[test]
    fn ui_event_is_nats_event() {
        assert_eq!(UI_EVENT, "nats-event");
        assert_eq!(WILDCARD, "lounge.>");
    }

    #[test]
    fn roundtrips_envelope() {
        let original = LoungeMessage::new(
            BUS_HEARTBEAT,
            "lounge-bus",
            serde_json::json!({ "tick": 1 }),
        );
        let bytes = serde_json::to_vec(&original).unwrap();
        let parsed = LoungeMessage::from_nats(BUS_HEARTBEAT, &bytes);
        assert_eq!(parsed.id, original.id);
        assert_eq!(parsed.source_agent, "lounge-bus");
        assert_eq!(parsed.subject, BUS_HEARTBEAT);
    }

    #[test]
    fn worker_tasks_subject_sanitizes() {
        assert_eq!(
            worker_tasks_subject("grok-tester").as_deref(),
            Some("lounge.tasks.grok-tester")
        );
        assert_eq!(
            worker_tasks_subject("Grok_Tester").as_deref(),
            Some("lounge.tasks.grok_tester")
        );
        assert!(worker_tasks_subject("bad.bot").is_none());
        assert!(worker_tasks_subject("").is_none());
    }
}
