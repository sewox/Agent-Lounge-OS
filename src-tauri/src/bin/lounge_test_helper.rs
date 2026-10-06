//! Cross-platform test helper for Graph UI port ownership / anti-adoption tests.
//!
//! Modes (same binary, argv-selected):
//! - `tcp-hold --port=N` — bind TCP LISTEN on `127.0.0.1:N` and park until killed.
//! - `--ui=true --port=N` — fake codebase-memory-mcp Graph UI (`/api/ui-config`, `/rpc`).
//!
//! Production builds may include this binary; it is only invoked from tests. Spawn paths
//! copy/rename it to `codebase-memory-mcp` so GuardedCommand allowlisting matches.

use std::env;
use std::io::{Read, Write};
use std::net::TcpListener;
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

fn run_tcp_hold(args: &[String]) -> Result<(), String> {
    let port = parse_port_flag(args)?;
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|e| format!("bind 127.0.0.1:{port}: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("set_nonblocking: {e}"))?;
    // Signal readiness on stdout (tests may ignore).
    let mut out = std::io::stdout();
    let _ = writeln!(out, "tcp-hold-ready port={port} pid={}", std::process::id());
    let _ = out.flush();
    // Keep the listener alive; accept loop drains spurious connections.
    loop {
        let _ = listener.accept();
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn run_fake_cbm(args: &[String]) -> Result<(), String> {
    let port = parse_port_flag(args)?;
    let rpc_hits = Arc::new(AtomicU64::new(0));
    let hits = rpc_hits.clone();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;

    rt.block_on(async move {
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

        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| format!("bind 127.0.0.1:{port}: {e}"))?;
        eprintln!(
            "fake-cbm-ready port={port} pid={} rpc_hits_path=stderr",
            std::process::id()
        );

        // Serve until stdin closes (parent keeps Stdio::piped open) or process killed.
        let serve = axum::serve(listener, app);
        tokio::select! {
            res = serve => {
                res.map_err(|e| format!("serve: {e}"))?;
            }
            _ = stdin_closed() => {}
        }
        Ok::<(), String>(())
    })?;

    let _ = rpc_hits;
    Ok(())
}

async fn stdin_closed() {
    tokio::task::spawn_blocking(|| {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 64];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(_) => break,
            }
        }
    })
    .await
    .ok();
}
