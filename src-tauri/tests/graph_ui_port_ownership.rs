//! Graph UI port ownership + anti-adoption — real child PID, real spawn path.
//!
//! These integration tests must pass on Linux, macOS, and Windows CI without
//! `#[ignore]`, skip, hollow cfg gates, or soft early returns.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use app_lib::services::memory_bridge::{MemoryBridge, MemoryBridgeConfig, TransportMode};
use app_lib::services::{
    enable_graph_ui_headless, listen_pids, port_owned_by_lounge, spawn_tcp_hold_child,
    stage_codebase_memory_mcp_double, tcp_bind_available, wait_until_port_owned, GraphUiPortMode,
    GraphUiState,
};
use axum::routing::{get, post};
use axum::{Json, Router};

fn helper_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lounge-test-helper"))
}

/// Pick two consecutive free loopback ports for an injectable test band.
fn reserve_contiguous_port_pair() -> (u16, u16) {
    for _ in 0..200 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
        let first = listener.local_addr().expect("addr").port();
        if first == u16::MAX {
            continue;
        }
        let second = first + 1;
        if !tcp_bind_available(second) {
            continue;
        }
        drop(listener);
        // Brief window; re-check both free before returning.
        if tcp_bind_available(first) && tcp_bind_available(second) {
            return (first, second);
        }
    }
    panic!("could not reserve contiguous free port pair");
}

struct ForeignCbm {
    port: u16,
    rpc_hits: Arc<AtomicU32>,
    _task: tokio::task::JoinHandle<()>,
}

async fn spawn_foreign_cbm_on(port: u16) -> ForeignCbm {
    let rpc_hits = Arc::new(AtomicU32::new(0));
    let hits = rpc_hits.clone();
    let app = Router::new()
        .route(
            "/api/ui-config",
            get(|| async { Json(serde_json::json!({"lang": "en", "foreign": true})) }),
        )
        .route(
            "/rpc",
            post(move |_body: String| {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": {
                            "content": [{ "type": "text", "text": "{\"projects\":[]}" }]
                        }
                    }))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap_or_else(|e| panic!("foreign bind :{port}: {e}"));
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    // Wait until ui-config answers.
    for _ in 0..40 {
        if app_lib::services::probe_ui_config(port).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        app_lib::services::probe_ui_config(port).await,
        "foreign ui-config must be up on {port}"
    );
    ForeignCbm {
        port,
        rpc_hits,
        _task: task,
    }
}

/// Acceptance: listen_pids / ownership match a Lounge-spawned child.id() — not parent.
#[tokio::test]
async fn port_owned_by_lounge_matches_spawned_child_id() {
    let (binary, _scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage helper");

    let hold = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral");
    let port = hold.local_addr().expect("addr").port();
    drop(hold);

    let mut child = spawn_tcp_hold_child(&binary, port).expect("spawn tcp-hold child");
    let child_pid = child.id();
    let parent_pid = std::process::id();
    assert_ne!(child_pid, parent_pid, "child must be a distinct process");

    assert!(
        wait_until_port_owned(port, child_pid, Duration::from_secs(5)),
        "listen_pids({port}) must include spawned child.id()={child_pid}; saw {:?}",
        listen_pids(port)
    );
    assert!(
        port_owned_by_lounge(port, Some(child_pid)),
        "owned for child.id()"
    );
    assert!(
        !port_owned_by_lounge(port, Some(parent_pid)),
        "parent PID must not own child's listen port"
    );

    let _ = child.kill();
    let _ = child.wait();
    // Dead/exited child: ownership must clear.
    for _ in 0..40 {
        if !port_owned_by_lounge(port, Some(child_pid)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !port_owned_by_lounge(port, Some(child_pid)),
        "dead/exited child.id()={child_pid} must not own port {port}"
    );
}

/// Acceptance: foreign ui-config on first band port is not adopted; Lounge spawns next.
#[tokio::test]
async fn enable_graph_ui_skips_foreign_band_port_owns_child() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage helper");
    let (band_start, band_end) = reserve_contiguous_port_pair();
    let foreign = spawn_foreign_cbm_on(band_start).await;
    assert!(
        !tcp_bind_available(band_start),
        "foreign must occupy band start"
    );
    assert!(
        tcp_bind_available(band_end),
        "band end must stay free for Lounge spawn"
    );

    let bridge = MemoryBridge::with_config(
        &binary,
        MemoryBridgeConfig {
            http_port: band_start,
            transport: TransportMode::Auto,
            ..MemoryBridgeConfig::default()
        },
    );
    let state = GraphUiState::new();
    state.set_port_mode(GraphUiPortMode::Auto);

    let live = enable_graph_ui_headless(&state, &bridge, None, band_start..=band_end)
        .await
        .expect("enable_graph_ui_headless");

    assert_eq!(
        live, band_end,
        "Lounge must spawn on next free band port (not foreign {band_start})"
    );
    assert_eq!(bridge.http_port(), band_end);

    let child_pid = state
        .spawned_child_pid()
        .expect("Lounge must track spawned child");
    assert!(
        wait_until_port_owned(live, child_pid, Duration::from_secs(5)),
        "ownership must match child.id()={child_pid} on live port {live}; pids={:?}",
        listen_pids(live)
    );
    assert!(port_owned_by_lounge(live, Some(child_pid)));
    assert!(!port_owned_by_lounge(
        foreign.port,
        state.spawned_child_pid()
    ));

    // MemoryBridge must not HTTP /rpc to the foreign instance.
    assert_eq!(
        bridge.select_transport().await,
        app_lib::services::memory_bridge::ToolTransport::HttpRpc,
        "owned live port should select HTTP"
    );
    // Point at foreign briefly with wrong ownership → CLI + zero hits.
    bridge.set_http_port(foreign.port);
    bridge.set_owned_ui_pid(None);
    assert_eq!(
        bridge.select_transport().await,
        app_lib::services::memory_bridge::ToolTransport::Cli
    );
    let _ = bridge
        .run_tool(&["list_projects", "--format", "json"])
        .await;
    assert_eq!(
        foreign.rpc_hits.load(Ordering::SeqCst),
        0,
        "foreign /rpc must receive 0 hits"
    );

    // Cleanup Lounge child.
    state.kill_spawned_child();
    bridge.set_owned_ui_pid(None);
    let _ = std::fs::remove_dir_all(&scratch);
}
