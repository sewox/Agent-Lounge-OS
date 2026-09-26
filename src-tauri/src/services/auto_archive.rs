//! Hourly auto-archive of stale experiences (TTL from settings).
//! First tick runs after ~60s so newly opened sessions catch stale rows sooner.

use std::time::Duration;

use crate::db::{ExperienceStore, SystemClock};

const FIRST_TICK: Duration = Duration::from_secs(60);
const HOURLY: Duration = Duration::from_secs(3600);

/// Spawn a background loop that archives unused experiences once after ~60s, then hourly.
pub fn spawn_auto_archive(store: ExperienceStore) {
    tauri::async_runtime::spawn(async move {
        let mut first = true;
        loop {
            let delay = if first { FIRST_TICK } else { HOURLY };
            first = false;
            tokio::time::sleep(delay).await;
            let ttl = store.experience_ttl_days().await.unwrap_or(90);
            match store.auto_archive_stale(ttl, &SystemClock).await {
                Ok(n) if n > 0 => log::info!("auto-archive: archived {n} stale experience(s)"),
                Ok(_) => {}
                Err(err) => log::warn!("auto-archive failed: {err}"),
            }
        }
    });
}
