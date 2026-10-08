//! Cross-platform test helper for Graph UI port ownership / anti-adoption tests.
//!
//! Modes (same binary, argv-selected):
//! - `tcp-hold --port=N` — bind TCP LISTEN on `127.0.0.1:N` and park until killed.
//! - `tcp-hold --port=0` — bind an ephemeral loopback port (reported in ready line).
//! - `tcp-hold --listen-fd=N` (Unix) / `--listen-stdin` (Windows) — adopt a pre-bound
//!   LISTEN socket from the parent (no rebind; kills reserve→free→rebind races).
//! - `--ui=true --port=N` — fake codebase-memory-mcp Graph UI (`/api/ui-config`, `/rpc`).
//!   Same listen handoff flags are supported.
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

fn parse_port_flag(args: &[String]) -> Result<Option<u16>, String> {
    for arg in args {
        if let Some(rest) = arg.strip_prefix("--port=") {
            return rest
                .parse::<u16>()
                .map(Some)
                .map_err(|e| format!("bad --port=: {e}"));
        }
    }
    for i in 0..args.len() {
        if args[i] == "--port" {
            let v = args
                .get(i + 1)
                .ok_or_else(|| "--port missing value".to_string())?;
            return v
                .parse::<u16>()
                .map(Some)
                .map_err(|e| format!("bad --port: {e}"));
        }
    }
    Ok(None)
}

fn has_listen_stdin(args: &[String]) -> bool {
    args.iter().any(|a| a == "--listen-stdin")
}

#[cfg(unix)]
fn parse_listen_fd(args: &[String]) -> Result<Option<i32>, String> {
    for arg in args {
        if let Some(rest) = arg.strip_prefix("--listen-fd=") {
            return rest
                .parse::<i32>()
                .map(Some)
                .map_err(|e| format!("bad --listen-fd=: {e}"));
        }
    }
    Ok(None)
}

fn has_listen_handoff_request(args: &[String]) -> bool {
    if has_listen_stdin(args) {
        return true;
    }
    #[cfg(unix)]
    {
        if args.iter().any(|a| a.starts_with("--listen-fd=")) {
            return true;
        }
    }
    let _ = args;
    false
}

/// Adopt a parent-handed LISTEN socket, or bind `127.0.0.1:port` (`port=0` → ephemeral).
fn take_or_bind_listener(args: &[String]) -> Result<(tokio::net::TcpListener, u16), String> {
    if has_listen_handoff_request(args) {
        let listener = adopt_inherited_listener(args)?
            .ok_or_else(|| "listen handoff requested but no socket adopted".to_string())?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("local_addr: {e}"))?
            .port();
        return Ok((listener, port));
    }
    let port =
        parse_port_flag(args)?.ok_or_else(|| "missing --port (or listen handoff)".to_string())?;
    let listener = bind_loopback(port)?;
    let bound = listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?
        .port();
    Ok((listener, bound))
}

fn adopt_inherited_listener(args: &[String]) -> Result<Option<tokio::net::TcpListener>, String> {
    #[cfg(unix)]
    {
        if let Some(fd) = parse_listen_fd(args)? {
            // SAFETY: parent cleared CLOEXEC and passed this live LISTEN fd.
            let std_listener = unsafe { std::net::TcpListener::from_raw_fd_checked(fd)? };
            std_listener
                .set_nonblocking(true)
                .map_err(|e| format!("nonblocking: {e}"))?;
            let listener = tokio::net::TcpListener::from_std(std_listener)
                .map_err(|e| format!("from_std: {e}"))?;
            return Ok(Some(listener));
        }
        Ok(None)
    }

    #[cfg(windows)]
    {
        use std::os::windows::io::{FromRawSocket, RawSocket};
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE};

        if !has_listen_stdin(args) {
            return Ok(None);
        }
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return Err(format!(
                "listen-stdin: GetStdHandle(STD_INPUT_HANDLE) failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        // SAFETY: parent passed the LISTEN SOCKET as the child's stdin via
        // STARTF_USESTDHANDLES; we take ownership of that handle.
        let std_listener = unsafe { std::net::TcpListener::from_raw_socket(handle as RawSocket) };
        std_listener
            .set_nonblocking(true)
            .map_err(|e| format!("nonblocking: {e}"))?;
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .map_err(|e| format!("from_std: {e}"))?;
        Ok(Some(listener))
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = args;
        Ok(None)
    }
}

#[cfg(unix)]
trait FromRawFdChecked: Sized {
    unsafe fn from_raw_fd_checked(fd: i32) -> Result<Self, String>;
}

#[cfg(unix)]
impl FromRawFdChecked for std::net::TcpListener {
    unsafe fn from_raw_fd_checked(fd: i32) -> Result<Self, String> {
        use std::os::unix::io::FromRawFd;
        if fd < 0 {
            return Err(format!("listen-fd {fd} invalid"));
        }
        Ok(unsafe { std::net::TcpListener::from_raw_fd(fd) })
    }
}

/// Bind `127.0.0.1:port` for LISTEN (`port=0` → ephemeral).
///
/// On Unix, `SO_REUSEADDR` helps after unrelated bind probes. On Windows, leave
/// the default (SO_REUSEADDR there allows duplicate concurrent binds).
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
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .map_err(|e| format!("runtime: {e}"))?;

    let (listener, port) = rt.block_on(async { take_or_bind_listener(args) })?;

    let line = format!("tcp-hold-ready port={port} pid={}", std::process::id());
    let mut err = std::io::stderr();
    let _ = writeln!(err, "{line}");
    let _ = err.flush();
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();

    rt.block_on(async move {
        loop {
            let _ = listener.accept().await;
        }
    });
    Ok(())
}

fn run_fake_cbm(args: &[String]) -> Result<(), String> {
    let rpc_hits = Arc::new(AtomicU64::new(0));
    let hits = rpc_hits.clone();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .map_err(|e| format!("runtime: {e}"))?;

    let (listener, port) = rt.block_on(async { take_or_bind_listener(args) })?;

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

    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}
