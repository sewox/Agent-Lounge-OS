//! Hourly auto-archive of stale experiences (TTL from settings).

use std::time::Duration;

use crate::db::{ExperienceStore, SystemClock};

/// Spawn a background loop that archives unused experiences once per hour.
pub fn spawn_auto_archive(store: ExperienceStore) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(3600)).await;
            let ttl = store.experience_ttl_days().await.unwrap_or(90);
            match store.auto_archive_stale(ttl, &SystemClock).await {
                Ok(n) if n > 0 => log::info!("auto-archive: archived {n} stale experience(s)"),
                Ok(_) => {}
                Err(err) => log::warn!("auto-archive failed: {err}"),
            }
        }
    });
}
