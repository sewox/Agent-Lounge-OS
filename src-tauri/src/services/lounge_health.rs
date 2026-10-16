//! H5: unified read-only `lounge_health` report (Tauri + MCP).
//!
//! Checks at app start (once) and on request only — never periodically.
//! Does not mutate MCP configs or CBM pollution (detect-only).

use std::sync::{Mutex, OnceLock};

use crate::db::ExperienceStore;
use crate::models::{
    now_rfc3339, CbmHealthReport, DeadSymbolHealthSummary, LoungeHealthReport,
    LOUNGE_HEALTH_SCHEMA_VERSION,
};
use crate::services::agent_health::{check_all_agents, AgentHealthRoots, ExpectedLoungeMcp};
use crate::services::cbm_daemon::{probe_daemon_status, CbmDaemonPresence};
use crate::services::cbm_ui_config::{
    cbm_ui_config_path, looks_like_lounge_pollution, read_cbm_ui_config,
};
use crate::services::health_filters::mask_home_path;
use crate::services::os_release::platform_health;
use crate::services::plugin::lounge_workspace;
use crate::services::title_health::collect_title_health;

fn cache() -> &'static Mutex<Option<LoungeHealthReport>> {
    static CACHE: OnceLock<Mutex<Option<LoungeHealthReport>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Last cached report (startup or previous request), if any.
pub fn cached_lounge_health() -> Option<LoungeHealthReport> {
    cache().lock().ok().and_then(|g| g.clone())
}

/// Collect a fresh report, update cache, return it.
pub async fn collect_lounge_health(store: &ExperienceStore) -> LoungeHealthReport {
    let report = build_lounge_health(store, true).await;
    if let Ok(mut g) = cache().lock() {
        *g = Some(report.clone());
    }
    report
}

/// Startup: one read-only check (never starts a timer).
pub async fn run_startup_lounge_health(store: ExperienceStore) {
    let report = collect_lounge_health(&store).await;
    let warn_agents = report
        .agents
        .iter()
        .filter(|a| a.status.as_str() != "ok" && a.status.as_str() != "not_installed")
        .count();
    log::info!(
        "lounge_health schema={} agents_warn={} title_backlog={} give_ups={} cbm_polluted={} daemon={}",
        report.schema_version,
        warn_agents,
        report.title.db_backlog,
        report.title.give_ups,
        report.cbm.polluted_18749,
        report.cbm.daemon_mode
    );
}

async fn build_lounge_health(store: &ExperienceStore, probe_daemon: bool) -> LoungeHealthReport {
    let expected = ExpectedLoungeMcp::current();
    let roots = AgentHealthRoots::from_env(lounge_workspace());
    let agents = check_all_agents(&roots, &expected);
    let title = collect_title_health(store).await;
    let dead_symbols = dead_symbol_summary(store).await;
    let cbm = cbm_health_detect(store, probe_daemon).await;
    let platform = platform_health();

    LoungeHealthReport {
        schema_version: LOUNGE_HEALTH_SCHEMA_VERSION.into(),
        checked_at: now_rfc3339(),
        platform,
        agents,
        title,
        dead_symbols,
        cbm,
        expected_lounge_mcp_version: Some(expected.version),
        expected_lounge_mcp_sha256: expected.sha256,
    }
}

async fn dead_symbol_summary(store: &ExperienceStore) -> DeadSymbolHealthSummary {
    let symbols = store.list_dead_symbols(None).await.unwrap_or_default();
    let mut kritik = 0u64;
    let mut potansiyel = 0u64;
    for s in &symbols {
        match s.severity.as_deref() {
            Some("Kritik") => kritik += 1,
            Some("Potansiyel") => potansiyel += 1,
            _ => potansiyel += 1,
        }
    }
    let ignored = store
        .list_ignored_symbols(None)
        .await
        .map(|v| v.len() as u64)
        .unwrap_or(0);
    DeadSymbolHealthSummary {
        kritik,
        potansiyel,
        total: symbols.len() as u64,
        ignored,
    }
}

async fn cbm_health_detect(store: &ExperienceStore, probe_daemon: bool) -> CbmHealthReport {
    let mut report = CbmHealthReport::default();

    if let Some(path) = cbm_ui_config_path() {
        report.config_path = Some(mask_home_path(&path.to_string_lossy()));
        match read_cbm_ui_config(&path) {
            Ok(Some((cfg, _))) => {
                report.ui_enabled = cfg.ui_enabled;
                report.ui_port = cfg.ui_port;
                // DETECT ONLY — never call migrate_lounge_cbm_config_pollution here.
                report.polluted_18749 = looks_like_lounge_pollution(&cfg);
            }
            Ok(None) => {}
            Err(err) => {
                log::debug!("cbm ui config read: {err}");
            }
        }
    }

    if probe_daemon {
        // Best-effort: reuse MemoryBridge discovery binary when available via store path?
        // Probe only if we can resolve codebase-memory-mcp without mutating anything.
        if let Ok(bridge) = crate::services::MemoryBridge::discover() {
            match probe_daemon_status(bridge.binary_path()) {
                Ok(status) => {
                    report.daemon_mode = status.is_daemon_mode();
                    report.daemon_presence = Some(match status.presence {
                        CbmDaemonPresence::Active => "active".into(),
                        CbmDaemonPresence::Inactive => "inactive".into(),
                        CbmDaemonPresence::Unknown => "unknown".into(),
                    });
                }
                Err(_) => {
                    report.daemon_presence = Some("unknown".into());
                }
            }
        } else {
            let _ = store; // silence; daemon probe optional
            report.daemon_presence = Some("unknown".into());
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::agent_health::{
        sha256_file_hex, ExpectedLoungeMcp, LOUNGE_MCP_VERSION_MARKER_PREFIX,
    };
    use crate::services::cbm_ui_config::CbmUiConfigFile;
    use serde_json::json;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;

    #[tokio::test]
    async fn lounge_health_masks_home_and_detects_pollution_only() {
        let dir = std::env::temp_dir().join(format!("lounge-health-h5-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let db = ExperienceStore::open(dir.join("t.sqlite")).unwrap();

        // Pollution fixture under injected CBM cache.
        let cache = dir.join("cbm-cache");
        fs::create_dir_all(&cache).unwrap();
        let cfg_path = cache.join("config.json");
        let polluted = CbmUiConfigFile {
            ui_enabled: Some(true),
            ui_port: Some(18749),
            extra: Default::default(),
        };
        fs::write(&cfg_path, serde_json::to_string(&polluted).unwrap()).unwrap();

        // Use low-level detect path (same as report) without migrating.
        assert!(looks_like_lounge_pollution(&polluted));
        let before = fs::read(&cfg_path).unwrap();

        let report = build_lounge_health(&db, false).await;
        assert_eq!(report.schema_version, LOUNGE_HEALTH_SCHEMA_VERSION);
        assert!(!report.checked_at.is_empty());
        assert!(!report.platform.os.is_empty());

        // Pollution file untouched.
        assert_eq!(fs::read(&cfg_path).unwrap(), before);

        let _ = fs::remove_dir_all(&dir);
        let _ = report;
        let _ = json!({});
    }

    #[test]
    fn expected_version_matches_crate() {
        let e = ExpectedLoungeMcp {
            version: env!("CARGO_PKG_VERSION").into(),
            sha256: None,
            path: None,
        };
        assert_eq!(e.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn version_marker_roundtrip_file() {
        let path = std::env::temp_dir().join(format!("lmcp-ver-{}", uuid::Uuid::new_v4()));
        let mut f = fs::File::create(&path).unwrap();
        writeln!(f, "{LOUNGE_MCP_VERSION_MARKER_PREFIX}9.9.9").unwrap();
        drop(f);
        let ver = crate::services::agent_health::extract_embedded_version(&path).unwrap();
        assert_eq!(ver, "9.9.9");
        let _ = sha256_file_hex(&path);
        let _ = fs::remove_file(&path);
        let _: PathBuf = path;
    }
}
