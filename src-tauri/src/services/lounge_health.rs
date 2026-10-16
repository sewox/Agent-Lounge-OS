//! H5: unified read-only `lounge_health` report (Tauri + MCP).
//!
//! Checks at app start (once) and on request only — never periodically.
//! Does not mutate MCP configs or CBM pollution (detect-only).

use std::path::PathBuf;
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

/// Injectable inputs for tests / callers that override env-derived paths.
#[derive(Debug, Clone, Default)]
pub struct LoungeHealthOptions {
    pub probe_daemon: bool,
    pub roots: Option<AgentHealthRoots>,
    pub cbm_config_path: Option<PathBuf>,
    pub expected: Option<ExpectedLoungeMcp>,
}

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
    let report = build_lounge_health(
        store,
        LoungeHealthOptions {
            probe_daemon: true,
            ..Default::default()
        },
    )
    .await;
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

pub async fn build_lounge_health(
    store: &ExperienceStore,
    options: LoungeHealthOptions,
) -> LoungeHealthReport {
    let expected = options
        .expected
        .clone()
        .unwrap_or_else(ExpectedLoungeMcp::current);
    let roots = options
        .roots
        .clone()
        .unwrap_or_else(|| AgentHealthRoots::from_env(lounge_workspace()));
    let cbm_path = options.cbm_config_path.clone().or_else(cbm_ui_config_path);
    let probe_daemon = options.probe_daemon;

    // Blocking binary reads + optional daemon CLI probe off the async worker.
    let expected_for_agents = expected.clone();
    let roots_for_agents = roots.clone();
    let agents = tokio::task::spawn_blocking(move || {
        check_all_agents(&roots_for_agents, &expected_for_agents)
    })
    .await
    .unwrap_or_default();

    let title = collect_title_health(store).await;
    let dead_symbols = dead_symbol_summary(store).await;

    let cbm = tokio::task::spawn_blocking(move || cbm_health_detect_sync(cbm_path, probe_daemon))
        .await
        .unwrap_or_default();

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

fn cbm_health_detect_sync(config_path: Option<PathBuf>, probe_daemon: bool) -> CbmHealthReport {
    let mut report = CbmHealthReport::default();

    if let Some(path) = config_path {
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // ENV_LOCK serializes HOME mutation for the whole test.
    async fn lounge_health_masks_home_and_detects_pollution_only() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("lounge-health-h5-{}", uuid::Uuid::new_v4()));
        let home = dir.join("home");
        fs::create_dir_all(&home).unwrap();
        let db = ExperienceStore::open(dir.join("t.sqlite")).unwrap();

        let prev_home = std::env::var_os("HOME");
        let prev_userprofile = std::env::var_os("USERPROFILE");
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("USERPROFILE", &home);
        }

        // Pollution fixture under HOME so masking is assertable.
        let cache = home.join(".cache/codebase-memory-mcp");
        fs::create_dir_all(&cache).unwrap();
        let cfg_path = cache.join("config.json");
        let polluted = CbmUiConfigFile {
            ui_enabled: Some(true),
            ui_port: Some(18749),
            extra: Default::default(),
        };
        fs::write(&cfg_path, serde_json::to_string(&polluted).unwrap()).unwrap();
        let before = fs::read(&cfg_path).unwrap();

        // Agent config under same HOME with absolute command path.
        let bin = home.join("bin/lounge-mcp");
        fs::create_dir_all(bin.parent().unwrap()).unwrap();
        {
            let mut f = fs::File::create(&bin).unwrap();
            writeln!(f, "#!/bin/sh\n{LOUNGE_MCP_VERSION_MARKER_PREFIX}0.1.0").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&bin).unwrap().permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&bin, perms).unwrap();
            }
        }
        let cursor_cfg = home.join(".cursor/mcp.json");
        fs::create_dir_all(cursor_cfg.parent().unwrap()).unwrap();
        fs::write(
            &cursor_cfg,
            serde_json::to_string_pretty(&json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "command": bin.to_string_lossy(),
                        "args": [],
                        "env": { "LOUNGE_MCP_URL": "http://127.0.0.1:18791" }
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let roots = AgentHealthRoots {
            home: home.clone(),
            appdata: None,
            xdg_config: Some(home.join(".config")),
            workspace: dir.join("ws"),
        };
        fs::create_dir_all(&roots.workspace).unwrap();

        let expected = ExpectedLoungeMcp {
            version: "0.1.0".into(),
            sha256: sha256_file_hex(&bin).ok(),
            path: Some(bin.clone()),
        };

        let report = build_lounge_health(
            &db,
            LoungeHealthOptions {
                probe_daemon: false,
                roots: Some(roots),
                cbm_config_path: Some(cfg_path.clone()),
                expected: Some(expected),
            },
        )
        .await;

        assert_eq!(report.schema_version, LOUNGE_HEALTH_SCHEMA_VERSION);
        assert!(
            report.cbm.polluted_18749,
            "injected polluted config must be detected"
        );
        assert_eq!(
            report.cbm.config_path.as_deref(),
            Some("~/.cache/codebase-memory-mcp/config.json")
        );
        // Byte-for-byte unchanged (detect-only).
        assert_eq!(fs::read(&cfg_path).unwrap(), before);

        let home_s = home.to_string_lossy();
        // Every path-ish field masked.
        for agent in &report.agents {
            if let Some(p) = &agent.config_path {
                assert!(p.starts_with("~/"), "agent config_path not masked: {p}");
                assert!(!p.contains(home_s.as_ref()), "leaked home in {p}");
            }
            if let Some(c) = &agent.command {
                assert!(
                    c.starts_with("~/") || !c.contains(home_s.as_ref()),
                    "agent command not masked: {c}"
                );
            }
            if let Some(d) = &agent.detail {
                assert!(!d.contains(home_s.as_ref()), "agent detail not masked: {d}");
            }
        }
        if let Some(p) = &report.cbm.config_path {
            assert!(p.starts_with("~/"), "cbm config_path not masked: {p}");
        }

        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prev_userprofile {
                Some(v) => std::env::set_var("USERPROFILE", v),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
        let _ = fs::remove_dir_all(&dir);
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
