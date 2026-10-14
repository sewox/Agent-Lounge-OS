//! Cross-platform test helper for Graph UI port ownership / anti-adoption tests.
//!
//! Modes (same binary, argv-selected):
//! - `tcp-hold --port=N` — bind TCP LISTEN on `127.0.0.1:N` and park until killed.
//! - `tcp-hold --port=0` — bind an ephemeral loopback port (reported in ready line).
//! - `tcp-hold --listen-fd=N` (Unix) — adopt a pre-bound LISTEN fd from the parent
//!   (CLOEXEC cleared only in the child's `pre_exec`).
//! - `tcp-hold --reuse-bind --port=N` (Windows) — SO_REUSEADDR bind while parent
//!   still holds (parent converted exclusive→reuseaddr immediately before spawn);
//!   parent drops after `listen-adopted`.
//! - `daemon status` — fake upstream v0.11.0 coordination-daemon probe.
//!   Controlled by `LOUNGE_FAKE_CBM_DAEMON` (see `run_daemon_status`).
//! - `--ui=true --port=N` — fake codebase-memory-mcp Graph UI (`/api/ui-config`, `/rpc`).
//!   Same listen handoff flags are supported. When `CBM_CACHE_DIR` is set, also
//!   writes `config.json` (`ui_enabled`/`ui_port`) so tests can assert Lounge does
//!   **not** reach this path in daemon mode.
//!
//! Built only with `--features test-helpers` (`required-features` on the [[bin]]).
//! Release / `tauri build` omit this feature, so the helper never ships in installers.
//! Tests stage a copy renamed to `codebase-memory-mcp` for GuardedCommand allowlisting.

use std::env;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
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
    } else if args.first().map(String::as_str) == Some("daemon") {
        match run_daemon_ctl(&args[1..]) {
            Ok(code) => code,
            Err(err) => {
                eprintln!("fake-daemon: {err}");
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

/// Fake `daemon <status|start|stop>` — only `status` is implemented for Lounge probes.
///
/// Signal (first match wins), so parallel tests do not race on process env:
/// 1. Sibling file `lounge-fake-cbm-daemon` next to this binary (one line)
/// 2. Else `LOUNGE_FAKE_CBM_DAEMON` env
///
/// Values:
/// - unset / `off` / `inactive` → `daemon: not running` (exit 1, like upstream)
/// - `active:permanent:PORT` → active permanent + UI on PORT
/// - `active:session:PORT` → active session-managed + UI on PORT
/// - `active:permanent:disabled` → active, UI disabled
fn run_daemon_ctl(args: &[String]) -> Result<ExitCode, String> {
    let sub = args.first().map(String::as_str).unwrap_or("");
    if sub != "status" {
        return Err(format!(
            "fake daemon only supports status (got {sub:?}); start/stop not implemented"
        ));
    }
    run_daemon_status()
}

fn fake_daemon_spec() -> String {
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let marker = dir.join("lounge-fake-cbm-daemon");
            if let Ok(raw) = std::fs::read_to_string(&marker) {
                return raw.trim().to_string();
            }
        }
    }
    env::var("LOUNGE_FAKE_CBM_DAEMON").unwrap_or_default()
}

fn run_daemon_status() -> Result<ExitCode, String> {
    let spec = fake_daemon_spec();
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("off") || spec.eq_ignore_ascii_case("inactive")
    {
        println!("daemon: not running");
        println!(
            "hint: `codebase-memory-mcp daemon start` keeps a daemon warm so CLI commands and hooks skip the per-command startup cost."
        );
        return Ok(ExitCode::from(1));
    }

    let parts: Vec<&str> = spec.split(':').collect();
    if parts.first().copied() != Some("active") {
        return Err(format!("bad LOUNGE_FAKE_CBM_DAEMON: {spec}"));
    }
    let kind = parts.get(1).copied().unwrap_or("permanent");
    let permanent = kind.eq_ignore_ascii_case("permanent");
    let label = if permanent {
        "permanent"
    } else {
        "session-managed"
    };
    let pid = std::process::id();
    println!("daemon: active ({label})");
    println!("  pid: {pid}");
    println!("  build: 0.11.0 (fakehelper000...)");
    println!("  committed clients: 0");

    let port_or_disabled = parts.get(2).copied().unwrap_or("disabled");
    if port_or_disabled.eq_ignore_ascii_case("disabled") {
        println!("  ui: disabled (enable with `daemon start` in a UI build)");
    } else {
        let port: u16 = port_or_disabled
            .parse()
            .map_err(|e| format!("bad daemon UI port {port_or_disabled:?}: {e}"))?;
        println!(
            "  ui: configured at http://127.0.0.1:{port} (readiness not checked; `daemon start --open` verifies it)"
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn maybe_pollute_cbm_config(port: u16) -> Result<(), String> {
    let Some(cache) = env::var_os("CBM_CACHE_DIR").map(PathBuf::from) else {
        return Ok(());
    };
    if cache.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(&cache).map_err(|e| format!("mkdir {}: {e}", cache.display()))?;
    let path = cache.join("config.json");
    let body = format!("{{\n    \"ui_enabled\": true,\n    \"ui_port\": {port}\n}}\n");
    std::fs::write(&path, body).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(())
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

fn has_reuse_bind(args: &[String]) -> bool {
    args.iter().any(|a| a == "--reuse-bind")
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

#[cfg(unix)]
fn has_listen_fd_request(args: &[String]) -> bool {
    args.iter().any(|a| a.starts_with("--listen-fd="))
}

fn emit_listen_adopted(port: u16) {
    let line = format!("listen-adopted port={port} pid={}", std::process::id());
    let mut err = std::io::stderr();
    let _ = writeln!(err, "{line}");
    let _ = err.flush();
}

/// Adopt / reuse-bind / bind. Emits `listen-adopted` when a handoff path is used.
fn take_or_bind_listener(args: &[String]) -> Result<(tokio::net::TcpListener, u16), String> {
    #[cfg(unix)]
    if has_listen_fd_request(args) {
        let listener = adopt_listen_fd(args)?
            .ok_or_else(|| "listen-fd requested but not adopted".to_string())?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("local_addr: {e}"))?
            .port();
        emit_listen_adopted(port);
        return Ok((listener, port));
    }

    let port =
        parse_port_flag(args)?.ok_or_else(|| "missing --port (or listen handoff)".to_string())?;
    let reuse = has_reuse_bind(args);
    let listener = bind_loopback(port, reuse)?;
    let bound = listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?
        .port();
    if reuse {
        emit_listen_adopted(bound);
    }
    Ok((listener, bound))
}

#[cfg(unix)]
fn adopt_listen_fd(args: &[String]) -> Result<Option<tokio::net::TcpListener>, String> {
    if let Some(fd) = parse_listen_fd(args)? {
        // SAFETY: parent passed this live LISTEN fd; CLOEXEC was cleared only
        // in this child's pre_exec, so concurrent sibling spawns cannot inherit it.
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

/// Bind `127.0.0.1:port` (`port=0` → ephemeral).
///
/// `reuse_bind` enables SO_REUSEADDR so Windows can bind while the parent still
/// holds the (reuseaddr-converted) reserved listener.
fn bind_loopback(port: u16, reuse_bind: bool) -> Result<tokio::net::TcpListener, String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let socket = tokio::net::TcpSocket::new_v4().map_err(|e| format!("socket: {e}"))?;
    if reuse_bind {
        socket
            .set_reuseaddr(true)
            .map_err(|e| format!("reuseaddr: {e}"))?;
    } else {
        #[cfg(not(windows))]
        {
            socket
                .set_reuseaddr(true)
                .map_err(|e| format!("reuseaddr: {e}"))?;
        }
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

    let (listener, port) = {
        let _enter = rt.enter();
        take_or_bind_listener(args)?
    };

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

    let (listener, port) = {
        let _enter = rt.enter();
        take_or_bind_listener(args)?
    };

    // Mirror upstream `--ui=true --port=N` persistence into shared config.json.
    maybe_pollute_cbm_config(port)?;

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
