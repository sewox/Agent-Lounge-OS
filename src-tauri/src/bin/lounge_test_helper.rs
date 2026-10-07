//! Cross-platform test helper for Graph UI port ownership / anti-adoption tests.
//!
//! Modes (same binary, argv-selected):
//! - `tcp-hold --port=N` — bind TCP LISTEN on `127.0.0.1:N` and park until killed.
//! - `--ui=true --port=N` — fake codebase-memory-mcp Graph UI (`/api/ui-config`, `/rpc`).
//!
//! Built only with `--features test-helpers` (`required-features` on the [[bin]]).
//! Release / `tauri build` omit this feature, so the helper never ships in installers.
//! Tests stage a copy renamed to `codebase-memory-mcp` for GuardedCommand allowlisting.

use std::env;
use std::io::Write;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("tcp-hold") {
        match run_tcp_hold(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("tcp-hold: {err}");
                ExitCode::FAILURE
            }
        }
    } else {
        match run_fake_cbm(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("fake-cbm: {err}");
                ExitCode::FAILURE
            }
        }
    }
}

fn parse_port_flag(args: &[String]) -> Result<u16, String> {
    for arg in args {
        if let Some(rest) = arg.strip_prefix("--port=") {
            return rest
                .parse::<u16>()
                .map_err(|e| format!("bad --port=: {e}"))
                .and_then(|p| {
                    if p == 0 {
                        Err("port 0 invalid".into())
                    } else {
                        Ok(p)
                    }
                });
        }
    }
    for i in 0..args.len() {
        if args[i] == "--port" {
            let v = args
                .get(i + 1)
                .ok_or_else(|| "--port missing value".to_string())?;
            return v
                .parse::<u16>()
                .map_err(|e| format!("bad --port: {e}"))
                .and_then(|p| {
                    if p == 0 {
                        Err("port 0 invalid".into())
                    } else {
                        Ok(p)
                    }
                });
        }
    }
    Err("missing --port".into())
}

/// Bind `127.0.0.1:port` for LISTEN.
///
/// On Unix, `SO_REUSEADDR` lets the helper re-bind after the parent’s
/// `tcp_bind_available` probe (bind+drop) which can leave the port briefly
/// unusable on macOS. On Windows, leave the default (SO_REUSEADDR there allows
/// duplicate concurrent binds).
fn bind_loopback(port: u16) -> Result<tokio::net::TcpListener, String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let socket = tokio::net::TcpSocket::new_v4().map_err(|e| format!("socket: {e}"))?;
    #[cfg(not(windows))]
    {
        socket
            .set_reuseaddr(true)
            .map_err(|e| format!("reuseaddr: {e}"))?;
    }
    socket.bind(addr).map_err(|e| format!("bind {addr}: {e}"))?;
    socket
        .listen(128)
        .map_err(|e| format!("listen {addr}: {e}"))
}

fn run_tcp_hold(args: &[String]) -> Result<(), String> {
    let port = parse_port_flag(args)?;
    // Multi-thread runtime: same rationale as `run_fake_cbm` — current_thread +
    // CREATE_NO_WINDOW on Windows CI has been observed to leave the LISTEN socket
    // invisible to GetExtendedTcpTable / netstat for the full ownership wait
    // (flake: port_owned_by_lounge_matches_spawned_child_id saw []).
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .map_err(|e| format!("runtime: {e}"))?;

    let listener = rt.block_on(async { bind_loopback(port) })?;

    // Printed only after bind+listen succeed — parent must wait on this line
    // (not a fixed sleep / bare listen_pids poll) before asserting ownership.
    let mut out = std::io::stdout();
    let _ = writeln!(out, "tcp-hold-ready port={port} pid={}", std::process::id());
    let _ = out.flush();

    rt.block_on(async move {
        loop {
            let _ = listener.accept().await;
        }
    });
    Ok(())
}

fn run_fake_cbm(args: &[String]) -> Result<(), String> {
    let port = parse_port_flag(args)?;
    let rpc_hits = Arc::new(AtomicU64::new(0));
    let hits = rpc_hits.clone();

    // Multi-thread runtime: reliable on Windows with CREATE_NO_WINDOW + piped stdin
    // (no dependency on stdin EOF to keep the process alive).
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .map_err(|e| format!("runtime: {e}"))?;

    let listener = rt.block_on(async { bind_loopback(port) })?;

    use axum::routing::{get, post};
    use axum::{Json, Router};

    let app = Router::new()
        .route(
            "/api/ui-config",
            get(|| async { Json(serde_json::json!({"lang": "en", "helper": true})) }),
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

    rt.spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // Self-probe: do not park until the socket actually accepts HTTP.
    let ready = rt.block_on(async {
        let url = format!("http://127.0.0.1:{port}/api/ui-config");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(resp) = reqwest::get(&url).await {
                if resp.status().is_success() {
                    return true;
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    });
    if !ready {
        return Err(format!(
            "fake-cbm self-probe failed on 127.0.0.1:{port}/api/ui-config"
        ));
    }

    eprintln!("fake-cbm-ready port={port} pid={}", std::process::id());
    let _ = std::io::stderr().flush();

    // Stay alive until parent kills us (stdin may be piped or null — ignore it).
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}
