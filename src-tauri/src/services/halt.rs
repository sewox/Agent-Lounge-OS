//! Global Halt v1 — refuse new dispatch / internal background work while active.
//!
//! Does **not** cancel in-flight or pending tasks, and does **not** stop external
//! agent sessions (Cursor, Antigravity, Claude Desktop, Grok Bot). Resume is an
//! explicit UI action. See `docs/control.md`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::db::ExperienceStore;
use crate::models::now_rfc3339;

pub const HALT_SETTING_KEY: &str = "halt_state";
pub const HALT_CHANGED_EVENT: &str = "lounge://halt-changed";

/// Who initiated Halt/Resume. v1: only the local UI (`ui`).
pub const HALT_BY_UI: &str = "ui";

/// Structured MCP / dispatch error code (documented in docs/mcp.md).
pub const HALTED_ERROR_CODE: &str = "halted";

/// Operator-facing message: temporary pause; retry later; in-flight untouched.
pub const HALTED_ERROR_MESSAGE: &str = "Agent Lounge OS is temporarily halted. \
New dispatch and write tools are refused; this is a temporary pause — retry after Resume. \
Halt v1 does not cancel in-flight or pending tasks and does not stop external agent sessions \
(Cursor, Antigravity, Claude Desktop, Grok Bot).";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HaltState {
    pub halted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

impl HaltState {
    pub fn active(reason: Option<String>, by: &str) -> Self {
        Self {
            halted: true,
            reason: reason.and_then(|r| {
                let t = r.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            }),
            by: Some(by.to_string()),
            at: Some(now_rfc3339()),
        }
    }

    pub fn cleared(by: &str) -> Self {
        Self {
            halted: false,
            reason: None,
            by: Some(by.to_string()),
            at: Some(now_rfc3339()),
        }
    }
}

/// Structured payload for MCP `halted` tool errors (retryable temporary pause).
pub fn halted_error_payload() -> serde_json::Value {
    serde_json::json!({
        "error": HALTED_ERROR_CODE,
        "code": HALTED_ERROR_CODE,
        "message": HALTED_ERROR_MESSAGE,
        "retryable": true,
        "temporary_pause": true,
        "cancels_inflight": false,
        "stops_external_sessions": false,
        "note": "Retry after the local operator resumes from the Agent Lounge OS UI."
    })
}

pub async fn load_halt_state(store: &ExperienceStore) -> Result<HaltState> {
    match store.get_setting(HALT_SETTING_KEY.into()).await? {
        Some(raw) => {
            let parsed: HaltState =
                serde_json::from_str(&raw).context("halt_state JSON parse failed")?;
            Ok(parsed)
        }
        None => Ok(HaltState::default()),
    }
}

/// Fail-closed: any storage or parse error is treated as halted (and logged).
pub async fn is_halted(store: &ExperienceStore) -> bool {
    halted_from_load_result(load_halt_state(store).await)
}

/// Map a [`load_halt_state`] result to the gate boolean (fail-closed on `Err`).
pub fn halted_from_load_result(result: Result<HaltState>) -> bool {
    match result {
        Ok(state) => state.halted,
        Err(err) => {
            log::error!("halt_state unreadable — fail-closed as halted: {err:#}");
            true
        }
    }
}

async fn persist(store: &ExperienceStore, state: &HaltState) -> Result<()> {
    let raw = serde_json::to_string(state)?;
    store
        .set_setting(HALT_SETTING_KEY.into(), raw)
        .await
        .context("halt_state persist")
}

/// Halt from the local UI only. `by` must be [`HALT_BY_UI`].
pub async fn halt(store: &ExperienceStore, reason: Option<String>, by: &str) -> Result<HaltState> {
    if by != HALT_BY_UI {
        anyhow::bail!("halt may only be triggered by the local UI");
    }
    let state = HaltState::active(reason, by);
    persist(store, &state).await?;
    let reason_for_journal = state.reason.clone();
    let actor = by.to_string();
    if let Err(err) = store.append_journal_entry(crate::db::JournalAppendInput::new(
        actor,
        crate::db::ACTION_CONTROL_HALTED,
        serde_json::json!({ "reason": reason_for_journal }).to_string(),
    )) {
        log::warn!("event_journal control.halted append failed: {err:#}");
    }
    log::info!(
        "halt engaged by={} at={} reason_set={}",
        by,
        state.at.as_deref().unwrap_or("?"),
        state.reason.is_some()
    );
    Ok(state)
}

/// Resume from the local UI only.
pub async fn resume(store: &ExperienceStore, by: &str) -> Result<HaltState> {
    if by != HALT_BY_UI {
        anyhow::bail!("resume may only be triggered by the local UI");
    }
    let state = HaltState::cleared(by);
    persist(store, &state).await?;
    let actor = by.to_string();
    if let Err(err) = store.append_journal_entry(crate::db::JournalAppendInput::new(
        actor,
        crate::db::ACTION_CONTROL_RESUMED,
        "{}",
    )) {
        log::warn!("event_journal control.resumed append failed: {err:#}");
    }
    log::info!(
        "halt resumed by={} at={}",
        by,
        state.at.as_deref().unwrap_or("?")
    );
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::ExperienceStore;

    #[tokio::test]
    async fn halt_state_persists_across_reload() {
        let dir = std::env::temp_dir().join(format!("lounge-halt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lounge.sqlite");
        {
            let store = ExperienceStore::open(&path).unwrap();
            assert!(!is_halted(&store).await);
            let state = halt(&store, Some("maintenance".into()), HALT_BY_UI)
                .await
                .unwrap();
            assert!(state.halted);
            assert_eq!(state.reason.as_deref(), Some("maintenance"));
            assert_eq!(state.by.as_deref(), Some(HALT_BY_UI));
            assert!(state.at.is_some());
        }
        {
            let store = ExperienceStore::open(&path).unwrap();
            let loaded = load_halt_state(&store).await.unwrap();
            assert!(loaded.halted);
            assert_eq!(loaded.reason.as_deref(), Some("maintenance"));
            resume(&store, HALT_BY_UI).await.unwrap();
        }
        {
            let store = ExperienceStore::open(&path).unwrap();
            assert!(!is_halted(&store).await);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn halt_rejects_non_ui_actor() {
        let store = ExperienceStore::memory().unwrap();
        let err = halt(&store, None, "mcp-user")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("local UI"), "{err}");
        let err = resume(&store, "deep-link").await.unwrap_err().to_string();
        assert!(err.contains("local UI"), "{err}");
    }

    #[tokio::test]
    async fn empty_reason_stored_as_none() {
        let store = ExperienceStore::memory().unwrap();
        let state = halt(&store, Some("   ".into()), HALT_BY_UI).await.unwrap();
        assert!(state.halted);
        assert!(state.reason.is_none());
    }

    #[tokio::test]
    async fn halt_and_resume_append_event_journal() {
        use crate::db::{JournalQueryFilter, ACTION_CONTROL_HALTED, ACTION_CONTROL_RESUMED};

        let store = ExperienceStore::memory().unwrap();
        halt(&store, Some("journal-check".into()), HALT_BY_UI)
            .await
            .unwrap();
        resume(&store, HALT_BY_UI).await.unwrap();

        let entries = store
            .query_journal(&JournalQueryFilter {
                limit: Some(10),
                ..Default::default()
            })
            .unwrap();
        let actions: Vec<&str> = entries.iter().map(|e| e.action_type.as_str()).collect();
        assert!(actions.contains(&ACTION_CONTROL_HALTED), "{actions:?}");
        assert!(actions.contains(&ACTION_CONTROL_RESUMED), "{actions:?}");
        assert!(store.verify_journal_chain().unwrap().valid);
    }

    #[test]
    fn halted_error_payload_is_retryable() {
        let v = halted_error_payload();
        assert_eq!(v["code"], HALTED_ERROR_CODE);
        assert_eq!(v["retryable"], true);
        assert_eq!(v["cancels_inflight"], false);
        assert_eq!(v["stops_external_sessions"], false);
        let msg = v["message"].as_str().unwrap();
        assert!(msg.contains("temporary pause") || msg.contains("temporarily halted"));
        assert!(msg.contains("does not stop external"));
    }

    #[test]
    fn load_error_fails_closed_as_halted() {
        assert!(halted_from_load_result(Err(anyhow::anyhow!(
            "sqlite locked"
        ))));
        assert!(!halted_from_load_result(Ok(HaltState::default())));
        assert!(halted_from_load_result(Ok(HaltState::active(
            None, HALT_BY_UI
        ))));
    }

    #[tokio::test]
    async fn corrupt_halt_state_fails_closed() {
        let store = ExperienceStore::memory().unwrap();
        store
            .set_setting(HALT_SETTING_KEY.into(), "{not-json".into())
            .await
            .unwrap();
        assert!(
            load_halt_state(&store).await.is_err(),
            "corrupt JSON must not parse"
        );
        assert!(
            is_halted(&store).await,
            "corrupt halt_state must fail closed (halted)"
        );
    }

    #[tokio::test]
    async fn empty_unreadable_blob_fails_closed() {
        let store = ExperienceStore::memory().unwrap();
        // Valid JSON of the wrong shape — parse error → fail closed.
        store
            .set_setting(HALT_SETTING_KEY.into(), "\"halted\"".into())
            .await
            .unwrap();
        assert!(load_halt_state(&store).await.is_err());
        assert!(is_halted(&store).await);
    }
}
