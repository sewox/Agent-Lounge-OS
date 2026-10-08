//! Graph UI port ownership + anti-adoption — real child PID, real spawn path.
//!
//! These integration tests must pass on Linux, macOS, and Windows CI without
//! `#[ignore]`, skip, hollow cfg gates, or soft early returns.
//!
//! Port allocation rule: never reserve → free → rebind. Keep the `std`
//! `TcpListener` alive and hand it over (`from_std` for in-process axum,
//! `--listen-fd` on Unix, or Windows `--reuse-bind` while parent still holds).
//! Parent reservations use a plain exclusive bind (no SO_REUSEADDR).

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use app_lib::services::memory_bridge::{
    MemoryBridge, MemoryBridgeConfig, ToolTransport, TransportMode,
};
use app_lib::services::{
    classify_port_status, enable_graph_ui_headless, listen_pids, port_owned_by_lounge,
    probe_ui_config, spawn_tcp_hold_ephemeral, spawn_tcp_hold_on_std_listener,
    stage_codebase_memory_mcp_double, std_listener_to_tokio, wait_tcp_hold_ephemeral_ready,
    wait_tcp_hold_ready, wait_until_port_not_owned, wait_until_port_owned, GraphUiPortMode,
    GraphUiState,
};
use axum::routing::{get, post};
use axum::{Json, Router};

fn helper_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lounge-test-helper"))
}

/// Contiguous band: foreign slot + two successor slots, all still bound.
struct HeldBand {
    foreign_port: u16,
    band_end: u16,
    foreign: TcpListener,
    /// Successor ports still reserved (hand over or stage; never free-then-rebind).
    successors: Vec<TcpListener>,
}

/// Exclusive loopback bind — must fail if another socket is already listening.
fn bind_loopback_exclusive(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port))
}

fn reserve_held_band() -> HeldBand {
    for _ in 0..300 {
        // Plain exclusive bind in the parent. Child handoff uses SO_REUSEADDR
        // only on Windows (`--reuse-bind`); never reserve with reuseaddr here.
        let foreign = match bind_loopback_exclusive(0) {
            Ok(l) => l,
            Err(_) => continue,
        };
        let foreign_port = foreign.local_addr().expect("addr").port();
        if foreign_port >= u16::MAX - 2 {
            continue;
        }
        let mut successors = Vec::with_capacity(2);
        let mut ok = true;
        for offset in 1u16..=2 {
            match bind_loopback_exclusive(foreign_port + offset) {
                Ok(l) => successors.push(l),
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        return HeldBand {
            foreign_port,
            band_end: foreign_port + 2,
            foreign,
            successors,
        };
    }
    panic!("could not reserve held contiguous port band");
}

/// B1: exclusive reservation must fail when the port is already listened on
/// (Linux, macOS, Windows — SO_REUSEADDR parent reserve would wrongly succeed
/// on Windows and create a new port race).
#[test]
fn exclusive_port_reservation_rejects_busy_port() {
    let held = bind_loopback_exclusive(0).expect("bind ephemeral exclusive listener");
    let port = held.local_addr().expect("addr").port();
    let err = bind_loopback_exclusive(port)
        .expect_err("exclusive TcpListener::bind must fail on a port that is already listening");
    assert!(
        matches!(
            err.kind(),
            std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied
        ),
        "expected AddrInUse (or PermissionDenied), got {:?}: {err}",
        err.kind()
    );
    drop(held);
}

struct ForeignCbm {
    port: u16,
    rpc_hits: Arc<AtomicU32>,
    _task: tokio::task::JoinHandle<()>,
}

async fn spawn_foreign_cbm_on_listener(listener: TcpListener) -> ForeignCbm {
    let port = listener.local_addr().expect("foreign addr").port();
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
    let tokio_listener =
        std_listener_to_tokio(listener).unwrap_or_else(|e| panic!("foreign from_std :{port}: {e}"));
    let task = tokio::spawn(async move {
        let _ = axum::serve(tokio_listener, app).await;
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
/// Child binds ephemeral `--port=0` (no parent reserve/free/rebind).
#[tokio::test]
async fn port_owned_by_lounge_matches_spawned_child_id() {
    let (binary, _scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage helper");
    let parent_pid = std::process::id();

    let mut child = spawn_tcp_hold_ephemeral(&binary).expect("spawn ephemeral tcp-hold");
    let child_pid = child.id();
    assert_ne!(child_pid, parent_pid, "child must be a distinct process");

    let port = wait_tcp_hold_ephemeral_ready(&mut child, Duration::from_secs(5))
        .unwrap_or_else(|e| panic!("ephemeral tcp-hold ready: {e}"));

    assert!(
        wait_until_port_owned(port, child_pid, Duration::from_secs(5)),
        "after tcp-hold-ready, listen_pids({port}) must include child.id()={child_pid}; saw {:?}",
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

/// S2: Auto remap to a successor already owned by Lounge → ui_available=true.
/// Preferred = tcp-hold child via listen handoff; owned UI = in-process from_std.
#[tokio::test]
async fn classify_auto_owned_successor_reports_ui_available() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage helper");
    let mut held = reserve_held_band();
    let preferred = held.foreign_port;
    let band_end = held.band_end;
    assert!(
        !held.successors.is_empty(),
        "successor[0] reserved for owned UI"
    );
    let owned_listener = held.successors.remove(0);
    let owned_port = owned_listener.local_addr().expect("owned addr").port();
    // Keep the spare successor reserved so nothing steals band_end mid-test.
    let _spare = held.successors;

    let (mut foreign, foreign_port, handoff_guard) =
        spawn_tcp_hold_on_std_listener(&binary, held.foreign).expect("foreign handoff");
    assert_eq!(foreign_port, preferred);
    wait_tcp_hold_ready(&mut foreign, preferred, Duration::from_secs(5))
        .unwrap_or_else(|e| panic!("foreign ready: {e}"));
    drop(handoff_guard);
    assert_ne!(foreign.id(), std::process::id());

    let app = Router::new().route(
        "/api/ui-config",
        get(|| async { Json(serde_json::json!({"lang": "en", "lounge": true})) }),
    );
    let tokio_listener = std_listener_to_tokio(owned_listener)
        .unwrap_or_else(|e| panic!("owned from_std :{owned_port}: {e}"));
    let _owned_task = tokio::spawn(async move {
        let _ = axum::serve(tokio_listener, app).await;
    });
    for _ in 0..80 {
        if probe_ui_config(owned_port).await {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(probe_ui_config(owned_port).await, "owned ui-config up");

    let child_pid = std::process::id();
    assert!(
        wait_until_port_owned(owned_port, child_pid, Duration::from_secs(5)),
        "test process must own {owned_port}; pids={:?}",
        listen_pids(owned_port)
    );
    // After SO_REUSEADDR handoff, Windows may briefly still list this process on
    // preferred until the closed parent descriptor leaves the TCP table.
    let parent_off_preferred =
        wait_until_port_not_owned(preferred, child_pid, Duration::from_secs(5));
    assert!(
        parent_off_preferred,
        "parent must leave preferred {preferred} after handoff drop; pids={:?}",
        listen_pids(preferred)
    );
    assert!(!port_owned_by_lounge(preferred, Some(child_pid)));

    let classified = classify_port_status(
        preferred,
        Some(child_pid),
        GraphUiPortMode::Auto,
        preferred..=band_end,
    )
    .await;

    assert_eq!(classified.port, owned_port);
    assert!(
        classified.ui_available,
        "owned remapped successor must surface ui_available"
    );
    assert!(!classified.port_conflict);
    assert_eq!(classified.remap_from_port, Some(preferred));
    assert_eq!(classified.message_key.as_deref(), Some("graphAutoPortInfo"));

    let _ = foreign.kill();
    let _ = foreign.wait();
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Acceptance: foreign ui-config on first band port is not adopted; Lounge spawns next.
/// Free successors are staged as listen handoffs — never free-then-rebind.
#[tokio::test]
async fn enable_graph_ui_skips_foreign_band_port_owns_child() {
    let (binary, scratch) = stage_codebase_memory_mcp_double(&helper_bin()).expect("stage helper");
    let held = reserve_held_band();
    let band_start = held.foreign_port;
    let band_end = held.band_end;
    let foreign = spawn_foreign_cbm_on_listener(held.foreign).await;

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
    // Isolate shared CBM config.json away from the real user cache (S1).
    let cbm_cfg = scratch.join("cbm-cache").join("config.json");
    std::fs::create_dir_all(cbm_cfg.parent().unwrap()).expect("cbm cache dir");
    state.set_cbm_config_path_override(Some(cbm_cfg));

    for successor in held.successors {
        state.stage_listen_handoff(successor);
    }

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
