//! PR-Graph-2: shared coordination daemon mode — never mutate port/config.
//!
//! Runs on Linux, macOS, and Windows CI without `#[ignore]`, skip, or soft gates.
//! Uses the staged `lounge-test-helper` fake for both `daemon status` and UI HTTP.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use app_lib::services::memory_bridge::{MemoryBridge, MemoryBridgeConfig, TransportMode};
use app_lib::services::{
    enable_graph_ui_headless, graph_ui_status, port_owned_by_lounge, probe_daemon_status,
    probe_ui_config, stage_codebase_memory_mcp_double, wait_until, CbmDaemonStatus,
    GraphUiPortMode, GraphUiState,
};
use axum::routing::get;
use axum::{Json, Router};

fn helper_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lounge-test-helper"))
}

fn write_config(path: &PathBuf, enabled: bool, port: u16) -> String {
    let body = format!(
        "{{\n    \"ui_enabled\": {},\n    \"ui_port\": {}\n}}\n",
        if enabled { "true" } else { "false" },
        port
    );
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("mkdir config parent");
    }
    fs::write(path, &body).expect("write config");
    body
}

async fn serve_shared_ui(listener: std::net::TcpListener) -> u16 {
    let port = listener.local_addr().expect("addr").port();
    listener.set_nonblocking(true).expect("nonblocking");
    let tokio_listener = tokio::net::TcpListener::from_std(listener).expect("from_std");
    let app = Router::new().route(
        "/api/ui-config",
        get(|| async { Json(serde_json::json!({"lang": "en", "shared": true})) }),
    );
    tokio::spawn(async move {
        let _ = axum::serve(tokio_listener, app).await;
    });
    let up = wait_until(
        Duration::from_secs(5),
        Duration::from_millis(25),
        || async { probe_ui_config(port).await },
    )
    .await;
    assert!(up, "shared UI must answer /api/ui-config on {port}");
    port
}

/// Daemon mode + live shared UI: enable must not spawn and must leave config untouched.
#[tokio::test]
async fn daemon_mode_enable_opens_existing_without_touching_config() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");
    let cache = scratch.join("cbm-cache");
    fs::create_dir_all(&cache).expect("cache");
    let config_path = cache.join("config.json");

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind shared");
    let shared_port = serve_shared_ui(listener).await;
    let original = write_config(&config_path, true, shared_port);

    // If Lounge wrongly spawns `--ui=true --port=N`, the fake writes CBM_CACHE_DIR.
    std::env::set_var("CBM_CACHE_DIR", &cache);

    let bridge = MemoryBridge::with_config(
        &binary,
        MemoryBridgeConfig {
            http_port: 18749,
            transport: TransportMode::ForceCli,
            ..MemoryBridgeConfig::default()
        },
    );
    let state = GraphUiState::new();
    state.set_port_mode(GraphUiPortMode::Auto);
    state.set_cbm_config_path_override(Some(config_path.clone()));
    state.set_daemon_status_override(Some(CbmDaemonStatus {
        active: true,
        permanent: true,
        daemon_pid: Some(4242),
        ui_port: Some(shared_port),
        ui_disabled: false,
        raw_summary: format!("daemon: active (permanent) ui={shared_port}"),
    }));

    let status = graph_ui_status(&state, &bridge, None).await;
    assert!(status.daemon_mode, "status must report daemon_mode");
    assert!(status.ui_available, "shared UI must be available");
    assert!(!status.owned_by_lounge);
    assert!(
        !status.port_conflict,
        "daemon mode must not be an error state"
    );
    assert_eq!(status.port, shared_port);
    assert_eq!(
        status.message_key.as_deref(),
        Some("graphDaemonOpenExisting")
    );

    let live = enable_graph_ui_headless(&state, &bridge, None, 18749..=18759)
        .await
        .expect("daemon enable returns existing port");
    assert_eq!(live, shared_port);
    assert!(
        state.spawned_child_pid().is_none(),
        "daemon mode must never spawn a Lounge child"
    );

    let after = fs::read_to_string(&config_path).expect("read config after");
    assert_eq!(
        after, original,
        "shared config.json must be byte-identical after enable"
    );
    assert!(
        after.contains(&format!("\"ui_port\": {shared_port}")),
        "ui_port must remain {shared_port}"
    );

    std::env::remove_var("CBM_CACHE_DIR");
    let _ = fs::remove_dir_all(&scratch);
}

/// Daemon active but UI disabled: no spawn, config untouched, clear non-error info.
#[tokio::test]
async fn daemon_mode_without_ui_does_not_spawn_or_mutate() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");
    let cache = scratch.join("cbm-cache");
    fs::create_dir_all(&cache).expect("cache");
    let config_path = cache.join("config.json");
    let original = write_config(&config_path, false, 9749);

    std::env::set_var("CBM_CACHE_DIR", &cache);

    let bridge = MemoryBridge::with_config(
        &binary,
        MemoryBridgeConfig {
            http_port: 18749,
            transport: TransportMode::ForceCli,
            ..MemoryBridgeConfig::default()
        },
    );
    let state = GraphUiState::new();
    state.set_cbm_config_path_override(Some(config_path.clone()));
    state.set_daemon_status_override(Some(CbmDaemonStatus {
        active: true,
        permanent: true,
        daemon_pid: Some(7),
        ui_port: None,
        ui_disabled: true,
        raw_summary: "daemon: active ui disabled".into(),
    }));

    let status = graph_ui_status(&state, &bridge, None).await;
    assert!(status.daemon_mode);
    assert!(!status.ui_available);
    assert!(!status.port_conflict);
    assert_eq!(status.message_key.as_deref(), Some("graphDaemonSharedNoUi"));

    let err = enable_graph_ui_headless(&state, &bridge, None, 18749..=18759)
        .await
        .expect_err("enable must refuse spawn when shared UI is down");
    let msg = format!("{err}");
    assert!(
        msg.contains("graphDaemonSharedNoUi"),
        "expected i18n key, got {msg}"
    );
    assert!(state.spawned_child_pid().is_none());
    let after = fs::read_to_string(&config_path).expect("config");
    assert_eq!(after, original);

    std::env::remove_var("CBM_CACHE_DIR");
    let _ = fs::remove_dir_all(&scratch);
}

/// Per-session (daemon inactive): existing spawn path still owns the child port.
#[tokio::test]
async fn per_session_mode_still_spawns_and_owns() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");
    let cache = scratch.join("cbm-cache");
    fs::create_dir_all(&cache).expect("cache");
    let config_path = cache.join("config.json");
    write_config(&config_path, false, 9749);
    std::env::set_var("CBM_CACHE_DIR", &cache);

    // Stage a reserved listener for handoff — never free-then-rebind.
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind lounge port");
    let lounge_port = listener.local_addr().expect("addr").port();

    let bridge = MemoryBridge::with_config(
        &binary,
        MemoryBridgeConfig {
            http_port: lounge_port,
            transport: TransportMode::Auto,
            ..MemoryBridgeConfig::default()
        },
    );
    let state = GraphUiState::new();
    state.set_port_mode(GraphUiPortMode::User);
    state.set_cbm_config_path_override(Some(config_path.clone()));
    state.set_daemon_status_override(Some(CbmDaemonStatus::inactive()));
    state.stage_listen_handoff(listener);

    let live = enable_graph_ui_headless(&state, &bridge, None, lounge_port..=lounge_port)
        .await
        .expect("per-session enable");
    assert_eq!(live, lounge_port);
    let child = state.spawned_child_pid().expect("child spawned");
    assert!(
        port_owned_by_lounge(live, Some(child)),
        "per-session child must own the listen port"
    );

    let status = graph_ui_status(&state, &bridge, None).await;
    assert!(!status.daemon_mode);
    assert!(status.ui_available);
    assert!(status.owned_by_lounge);

    state.kill_spawned_child();
    std::env::remove_var("CBM_CACHE_DIR");
    let _ = fs::remove_dir_all(&scratch);
}

/// Fake helper `daemon status` argv path (sibling marker file) matches upstream format.
#[tokio::test]
async fn fake_helper_daemon_status_argv_roundtrip() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");
    let marker = scratch.join("lounge-fake-cbm-daemon");
    fs::write(&marker, "active:permanent:9749").expect("write marker");

    let status = probe_daemon_status(&binary).expect("probe");
    assert!(status.active);
    assert!(status.permanent);
    assert_eq!(status.ui_port, Some(9749));

    fs::write(&marker, "inactive").expect("write inactive");
    let off = probe_daemon_status(&binary).expect("probe off");
    assert!(!off.active);

    let _ = fs::remove_dir_all(&scratch);
}
