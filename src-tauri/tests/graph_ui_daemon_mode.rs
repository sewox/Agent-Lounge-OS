//! PR-Graph-2: shared coordination daemon — fail-closed Active/Inactive/Unknown.
//!
//! Runs on Linux, macOS, and Windows CI without `#[ignore]`, skip, or soft gates.
//! End-to-end paths drive the staged fake helper via its sibling marker file
//! (no in-process override, no process-wide `CBM_CACHE_DIR` races).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use app_lib::services::memory_bridge::{MemoryBridge, MemoryBridgeConfig, TransportMode};
use app_lib::services::{
    classify_daemon_probe_output, enable_graph_ui_headless, fake_helper_pollution_config,
    graph_ui_status, parse_daemon_status_output, port_owned_by_lounge, probe_daemon_status,
    probe_ui_config, stage_codebase_memory_mcp_double, strip_utf8_bom, wait_until,
    CbmDaemonPresence, CbmDaemonStatus, GraphUiPortMode, GraphUiState,
};
use axum::routing::get;
use axum::{Json, Router};

fn helper_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lounge-test-helper"))
}

fn write_marker(scratch: &Path, spec: &str) {
    fs::write(scratch.join("lounge-fake-cbm-daemon"), spec).expect("write marker");
}

fn write_config(path: &Path, enabled: bool, port: u16) -> String {
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

fn bridge_for(binary: &Path, port: u16) -> MemoryBridge {
    MemoryBridge::with_config(
        binary,
        MemoryBridgeConfig {
            http_port: port,
            transport: TransportMode::ForceCli,
            ..MemoryBridgeConfig::default()
        },
    )
}

// --- Parser / classify robustness (pure) ------------------------------------

#[test]
fn parse_accepts_bom_crlf_case_and_hosts() {
    let bom_crlf =
        "\u{FEFF}Daemon: Active (Permanent)\r\n  ui: configured at http://localhost:9749\r\n";
    let s = parse_daemon_status_output(bom_crlf);
    assert_eq!(s.presence, CbmDaemonPresence::Active);
    assert_eq!(s.ui_port, Some(9749));

    let v6 = "daemon: active (session-managed)\n  ui: configured at http://[::1]:18750\n";
    assert_eq!(parse_daemon_status_output(v6).ui_port, Some(18750));

    let nr = "DAEMON: NOT RUNNING\n";
    assert_eq!(
        parse_daemon_status_output(nr).presence,
        CbmDaemonPresence::Inactive
    );

    let resp = "daemon: not responding (endpoint held by pid 9)\n";
    assert_eq!(
        parse_daemon_status_output(resp).presence,
        CbmDaemonPresence::Unknown
    );
    assert!(strip_utf8_bom("\u{FEFF}x").starts_with('x'));
}

#[test]
fn classify_timeout_shaped_reasons() {
    assert_eq!(
        classify_daemon_probe_output("", "", Some(0))
            .reason
            .as_deref(),
        Some("empty_output")
    );
    assert_eq!(
        classify_daemon_probe_output("nope", "", Some(2))
            .reason
            .as_deref(),
        Some("nonzero_exit")
    );
    let pre = classify_daemon_probe_output(
        "Usage:\n  codebase-memory-mcp --ui=true\n",
        "unknown command: daemon\n",
        Some(1),
    );
    assert_eq!(pre.presence, CbmDaemonPresence::Inactive);
    assert_eq!(pre.reason.as_deref(), Some("pre_0_11_unknown_subcommand"));
}

#[test]
fn classify_old_binary_strictness_negatives() {
    // Reworded "active" + daemon-start hint must NOT be Inactive via loose match.
    let reworded = classify_daemon_probe_output(
        "shared coordination service is up (permanent)\nhint: `codebase-memory-mcp daemon start` keeps a daemon warm\n",
        "",
        Some(0),
    );
    assert_eq!(reworded.presence, CbmDaemonPresence::Unknown);

    // Stray "unrecognized" on stderr with exit 0 → Unknown (not old-binary).
    let stray =
        classify_daemon_probe_output("ok\n", "warning: unrecognized locale setting\n", Some(0));
    assert_eq!(stray.presence, CbmDaemonPresence::Unknown);

    // Real upstream "not running" (+ hint) stays Inactive.
    let not_running = classify_daemon_probe_output(
        "daemon: not running\nhint: `codebase-memory-mcp daemon start` keeps a daemon warm\n",
        "",
        Some(1),
    );
    assert_eq!(not_running.presence, CbmDaemonPresence::Inactive);
    assert_eq!(not_running.reason.as_deref(), Some("not_running"));

    // Genuine old-binary unknown-subcommand → Inactive.
    let old = classify_daemon_probe_output(
        "codebase-memory-mcp 0.10.8\n",
        "error: unrecognized subcommand 'daemon'\n",
        Some(2),
    );
    assert_eq!(old.presence, CbmDaemonPresence::Inactive);
    assert_eq!(old.reason.as_deref(), Some("pre_0_11_unknown_subcommand"));
}

// --- Real probe via fake helper marker (no in-process override) -------------

#[tokio::test]
async fn probe_marker_timeout_is_unknown() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");
    write_marker(&scratch, "hang");
    let status = probe_daemon_status(&binary).expect("probe");
    assert_eq!(status.presence, CbmDaemonPresence::Unknown);
    assert_eq!(status.reason.as_deref(), Some("timeout"));
    assert!(status.blocks_spawn());
    let _ = fs::remove_dir_all(&scratch);
}

/// M1: grandchild holding pipes after kill must not hang the probe (bounded joins).
#[tokio::test]
async fn probe_timeout_with_pipe_holder_does_not_hang() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");
    write_marker(&scratch, "hang-pipe-holder");
    let started = std::time::Instant::now();
    let status = probe_daemon_status(&binary).expect("probe");
    let elapsed = started.elapsed();
    assert_eq!(status.presence, CbmDaemonPresence::Unknown);
    assert_eq!(status.reason.as_deref(), Some("timeout"));
    assert!(
        elapsed < Duration::from_secs(8),
        "bounded reader joins must finish promptly, took {elapsed:?}"
    );
    let _ = fs::remove_dir_all(&scratch);
}

#[tokio::test]
async fn probe_marker_nonzero_garbage_empty_crash_not_responding() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");

    write_marker(&scratch, "garbage");
    let g = probe_daemon_status(&binary).expect("garbage");
    assert_eq!(g.presence, CbmDaemonPresence::Unknown);
    assert_eq!(g.reason.as_deref(), Some("nonzero_exit"));

    write_marker(&scratch, "empty");
    let e = probe_daemon_status(&binary).expect("empty");
    assert_eq!(e.presence, CbmDaemonPresence::Unknown);
    assert_eq!(e.reason.as_deref(), Some("empty_output"));

    write_marker(&scratch, "crash");
    let c = probe_daemon_status(&binary).expect("crash");
    assert_eq!(c.presence, CbmDaemonPresence::Unknown);

    write_marker(&scratch, "not-responding");
    let n = probe_daemon_status(&binary).expect("not-responding");
    assert_eq!(n.presence, CbmDaemonPresence::Unknown);
    assert_eq!(n.reason.as_deref(), Some("not_responding"));

    write_marker(&scratch, "exit:3");
    let x = probe_daemon_status(&binary).expect("exit:3");
    assert_eq!(x.presence, CbmDaemonPresence::Unknown);

    let _ = fs::remove_dir_all(&scratch);
}

#[tokio::test]
async fn probe_marker_crlf_bom_active_and_pre_0_11() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");

    write_marker(&scratch, "crlf-bom-active:9749");
    let a = probe_daemon_status(&binary).expect("crlf-bom");
    assert_eq!(a.presence, CbmDaemonPresence::Active);
    assert_eq!(a.ui_port, Some(9749));

    write_marker(&scratch, "pre-0.11");
    let p = probe_daemon_status(&binary).expect("pre-0.11");
    assert_eq!(p.presence, CbmDaemonPresence::Inactive);
    assert_eq!(p.reason.as_deref(), Some("pre_0_11_unknown_subcommand"));

    let _ = fs::remove_dir_all(&scratch);
}

#[tokio::test]
async fn probe_binary_fails_to_start_is_unknown() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let scratch = std::env::temp_dir().join(format!(
        "lounge-cbm-spawnfail-{}-{}",
        std::process::id(),
        nanos
    ));
    fs::create_dir_all(&scratch).expect("mkdir");
    let binary = scratch.join(if cfg!(windows) {
        "codebase-memory-mcp.exe"
    } else {
        "codebase-memory-mcp"
    });
    // Empty non-executable file → spawn failure on Unix; on Windows may also fail.
    fs::write(&binary, []).expect("write empty");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&binary).unwrap().permissions();
        perms.set_mode(0o644); // not executable
        fs::set_permissions(&binary, perms).unwrap();
    }
    let status = probe_daemon_status(&binary).expect("probe");
    assert_eq!(status.presence, CbmDaemonPresence::Unknown);
    assert!(
        status.reason.as_deref() == Some("spawn_failed")
            || status.reason.as_deref() == Some("gate_error")
            || status.reason.as_deref() == Some("wait_error"),
        "expected spawn/gate/wait unknown, got {:?}",
        status.reason
    );
    let _ = fs::remove_dir_all(&scratch);
}

// --- E2E Active / Inactive / Unknown via marker (all 3 OSes) ----------------

/// Active: enable must not spawn; sibling pollution config must stay absent/untouched.
#[tokio::test]
async fn e2e_active_opens_existing_without_touching_config() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");
    let config_path = scratch.join("watched-config.json");
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind shared");
    let shared_port = serve_shared_ui(listener).await;
    let original = write_config(&config_path, true, shared_port);
    write_marker(&scratch, &format!("active:permanent:{shared_port}"));

    let bridge = bridge_for(&binary, 18749);
    let state = GraphUiState::new();
    state.set_port_mode(GraphUiPortMode::Auto);
    state.set_cbm_config_path_override(Some(config_path.clone()));
    // No in-process override — real `daemon status` via marker.

    let status = graph_ui_status(&state, &bridge, None).await;
    assert_eq!(status.daemon_presence, CbmDaemonPresence::Active);
    assert!(status.daemon_mode);
    assert!(status.ui_available);
    assert!(!status.port_conflict);
    assert_eq!(status.port, shared_port);
    assert_eq!(
        status.message_key.as_deref(),
        Some("graphDaemonOpenExisting")
    );

    let preferred_before = bridge.http_port();
    let live = enable_graph_ui_headless(&state, &bridge, None, 18749..=18759)
        .await
        .expect("active enable");
    assert_eq!(live, shared_port);
    assert!(state.spawned_child_pid().is_none());
    assert_eq!(
        bridge.http_port(),
        preferred_before,
        "daemon enable must not permanently set_http_port"
    );
    assert_eq!(fs::read_to_string(&config_path).unwrap(), original);
    // Fake helper would write sibling pollution dir if spawn happened.
    let pollution = fake_helper_pollution_config(&binary).expect("pollution path");
    assert!(
        !pollution.is_file(),
        "spawn pollution config must not appear: {}",
        pollution.display()
    );

    let _ = fs::remove_dir_all(&scratch);
}

/// Inactive: per-session spawn still owns the child port.
#[tokio::test]
async fn e2e_inactive_still_spawns_and_owns() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");
    write_marker(&scratch, "inactive");
    let config_path = scratch.join("watched-config.json");
    write_config(&config_path, false, 9749);

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
    state.set_cbm_config_path_override(Some(config_path));
    state.stage_listen_handoff(listener);

    let status = graph_ui_status(&state, &bridge, None).await;
    assert_eq!(status.daemon_presence, CbmDaemonPresence::Inactive);
    assert!(!status.daemon_mode);

    let live = enable_graph_ui_headless(&state, &bridge, None, lounge_port..=lounge_port)
        .await
        .expect("inactive enable");
    assert_eq!(live, lounge_port);
    let child = state.spawned_child_pid().expect("child spawned");
    assert!(port_owned_by_lounge(live, Some(child)));

    state.kill_spawned_child();
    let _ = fs::remove_dir_all(&scratch);
}

/// Unknown: never spawn; dedicated info key; config untouched.
#[tokio::test]
async fn e2e_unknown_never_spawns() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");
    write_marker(&scratch, "not-responding");
    let config_path = scratch.join("watched-config.json");
    let original = write_config(&config_path, false, 9749);

    let bridge = bridge_for(&binary, 18749);
    let state = GraphUiState::new();
    state.set_cbm_config_path_override(Some(config_path.clone()));

    let status = graph_ui_status(&state, &bridge, None).await;
    assert_eq!(status.daemon_presence, CbmDaemonPresence::Unknown);
    assert!(status.daemon_mode);
    assert!(!status.ui_available);
    assert!(!status.port_conflict);
    assert_eq!(
        status.message_key.as_deref(),
        Some("graphDaemonProbeUnknown")
    );

    let err = enable_graph_ui_headless(&state, &bridge, None, 18749..=18759)
        .await
        .expect_err("unknown must refuse spawn");
    assert!(
        format!("{err}").contains("graphDaemonProbeUnknown"),
        "got {err}"
    );
    assert!(state.spawned_child_pid().is_none());
    assert_eq!(fs::read_to_string(&config_path).unwrap(), original);
    let pollution = fake_helper_pollution_config(&binary).expect("pollution path");
    assert!(!pollution.is_file());

    let _ = fs::remove_dir_all(&scratch);
}

/// Status + enable share the brief daemon probe cache (one marker flip mid-flight).
#[tokio::test]
async fn status_and_enable_share_daemon_probe_cache() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");
    write_marker(&scratch, "not-responding");

    let _bridge = bridge_for(&binary, 18749);
    let state = GraphUiState::new();

    let first = state.probe_cbm_daemon_async(&binary).await;
    assert_eq!(first.presence, CbmDaemonPresence::Unknown);

    // Flip marker to Active — cached Unknown must still win within TTL.
    write_marker(&scratch, "active:permanent:9749");
    let cached = state.probe_cbm_daemon_async(&binary).await;
    assert_eq!(
        cached.presence,
        CbmDaemonPresence::Unknown,
        "enable/status must share the brief cache"
    );

    state.clear_daemon_probe_cache();
    let fresh = state.probe_cbm_daemon_async(&binary).await;
    assert_eq!(fresh.presence, CbmDaemonPresence::Active);

    let _ = fs::remove_dir_all(&scratch);
}

/// M5: stale Inactive cache must not spawn — fresh recheck right before spawn.
#[tokio::test]
async fn enable_rechecks_daemon_fresh_before_spawn() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");
    write_marker(&scratch, "inactive");
    let config_path = scratch.join("watched-config.json");
    let original = write_config(&config_path, false, 9749);

    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind shared");
    let shared_port = serve_shared_ui(listener).await;

    let bridge = bridge_for(&binary, 18749);
    let state = GraphUiState::new();
    state.set_port_mode(GraphUiPortMode::Auto);
    state.set_cbm_config_path_override(Some(config_path.clone()));

    // Prime the brief cache as Inactive.
    let cached = state.probe_cbm_daemon_async(&binary).await;
    assert_eq!(cached.presence, CbmDaemonPresence::Inactive);

    // Flip to Active with a live UI — enable must recheck (bypass cache) and refuse spawn.
    write_marker(&scratch, &format!("active:permanent:{shared_port}"));
    let preferred_before = bridge.http_port();
    let live = enable_graph_ui_headless(&state, &bridge, None, 18749..=18759)
        .await
        .expect("fresh Active should open existing UI");
    assert_eq!(live, shared_port);
    assert!(state.spawned_child_pid().is_none());
    assert_eq!(bridge.http_port(), preferred_before);
    assert_eq!(fs::read_to_string(&config_path).unwrap(), original);
    let pollution = fake_helper_pollution_config(&binary).expect("pollution path");
    assert!(
        !pollution.is_file(),
        "must not spawn on stale Inactive cache"
    );

    let _ = fs::remove_dir_all(&scratch);
}

/// Override path still used by ownership tests (Active helper).
#[tokio::test]
async fn override_active_without_ui_refuses_spawn() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage");
    let bridge = bridge_for(&binary, 18749);
    let state = GraphUiState::new();
    state.set_daemon_status_override(Some(CbmDaemonStatus::active_for_tests(None, true)));

    let status = graph_ui_status(&state, &bridge, None).await;
    assert_eq!(status.daemon_presence, CbmDaemonPresence::Active);
    assert_eq!(status.message_key.as_deref(), Some("graphDaemonSharedNoUi"));

    let err = enable_graph_ui_headless(&state, &bridge, None, 18749..=18759)
        .await
        .expect_err("no ui");
    assert!(format!("{err}").contains("graphDaemonSharedNoUi"));
    let _ = fs::remove_dir_all(&scratch);
}
