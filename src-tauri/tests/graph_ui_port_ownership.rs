//! Graph UI port ownership + anti-adoption — real child PID, real spawn path.
//!
//! These integration tests must pass on Linux, macOS, and Windows CI without
//! `#[ignore]`, skip, hollow cfg gates, or soft early returns.

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use app_lib::services::memory_bridge::{
    MemoryBridge, MemoryBridgeConfig, ToolTransport, TransportMode,
};
use app_lib::services::{
    enable_graph_ui_headless, listen_pids, port_owned_by_lounge, probe_ui_config,
    spawn_tcp_hold_child, stage_codebase_memory_mcp_double, tcp_bind_available,
    wait_until_port_owned, GraphUiPortMode, GraphUiState,
};
use axum::routing::{get, post};
use axum::{Json, Router};

fn helper_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lounge-test-helper"))
}

/// Hold a foreign slot + two free successors so Auto retry still has a port if
/// one free port is briefly unusable after release (macOS TIME_WAIT).
struct HeldBand {
    foreign_port: u16,
    band_end: u16,
    /// Keeps free band ports reserved until enable.
    hold_free: Vec<TcpListener>,
}

fn reserve_held_band() -> HeldBand {
    for _ in 0..300 {
        let hold_foreign = match TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(_) => continue,
        };
        let foreign_port = hold_foreign.local_addr().expect("addr").port();
        // Need foreign+1 and foreign+2 free and holdable.
        if foreign_port >= u16::MAX - 2 {
            continue;
        }
        let mut hold_free = Vec::with_capacity(2);
        let mut ok = true;
        for offset in 1u16..=2 {
            match TcpListener::bind(("127.0.0.1", foreign_port + offset)) {
                Ok(l) => hold_free.push(l),
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        let band_end = foreign_port + 2;
        // Release foreign slot for in-process axum; keep free ports held.
        drop(hold_foreign);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !tcp_bind_available(foreign_port) {
            assert!(
                std::time::Instant::now() < deadline,
                "foreign port {foreign_port} did not free after drop"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        return HeldBand {
            foreign_port,
            band_end,
            hold_free,
        };
    }
    panic!("could not reserve held contiguous port band");
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
    for _ in 0..80 {
        if probe_ui_config(port).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        probe_ui_config(port).await,
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

    let hold = TcpListener::bind("127.0.0.1:0").expect("ephemeral");
    let port = hold.local_addr().expect("addr").port();
    drop(hold);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !tcp_bind_available(port) {
        assert!(
            std::time::Instant::now() < deadline,
            "ephemeral port {port} must free before tcp-hold spawn"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

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
    let held = reserve_held_band();
    let band_start = held.foreign_port;
    let band_end = held.band_end;
    let foreign = spawn_foreign_cbm_on(band_start).await;
    assert!(
        !tcp_bind_available(band_start),
        "foreign must occupy band start"
    );
    // Release reserved free ports immediately before enable.
    drop(held.hold_free);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        let any_free = (band_start + 1..=band_end).any(tcp_bind_available);
        if any_free {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "band {band_start}..={band_end} must free a successor after releasing holds"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

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
        .unwrap_or_else(|e| panic!("enable_graph_ui_headless: {e}"));

    assert!(
        live > band_start && live <= band_end,
        "Lounge must spawn on a free band port after foreign {band_start}, got {live}"
    );
    assert_eq!(bridge.http_port(), live);

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

    assert_eq!(
        bridge.select_transport().await,
        ToolTransport::HttpRpc,
        "owned live port should select HTTP"
    );
    bridge.set_http_port(foreign.port);
    bridge.set_owned_ui_pid(None);
    assert_eq!(bridge.select_transport().await, ToolTransport::Cli);
    let _ = bridge
        .run_tool(&["list_projects", "--format", "json"])
        .await;
    assert_eq!(
        foreign.rpc_hits.load(Ordering::SeqCst),
        0,
        "foreign /rpc must receive 0 hits"
    );

    state.kill_spawned_child();
    bridge.set_owned_ui_pid(None);
    let _ = std::fs::remove_dir_all(&scratch);
}
