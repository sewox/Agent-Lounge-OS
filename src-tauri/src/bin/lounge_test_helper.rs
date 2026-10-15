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
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    // Windows grandchild for M1: inherit stdio and park so killed parents leave
    // open pipes for the probe's reader threads.
    if args.first().map(String::as_str) == Some("hold-stdio") {
        park_holding_stdio();
        return ExitCode::SUCCESS;
    }
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
/// Signal (sibling file only — no process-wide env races in parallel tests):
/// `lounge-fake-cbm-daemon` next to this binary (one line).
///
/// Values:
/// - unset / `off` / `inactive` → `daemon: not running` (exit 1, like upstream)
/// - `active:permanent:PORT` / `active:session:PORT` / `active:permanent:disabled`
/// - `not-responding` → `daemon: not responding …` (exit 1)
/// - `hang` → sleep forever (probe timeout → Unknown)
/// - `hang-pipe-holder` → spawn grandchild holding inherited stdio, then hang
///   (timeout kill must not leave the probe blocked on reader joins)
/// - `empty` → exit 0, no stdout
/// - `garbage` → exit 2, unrecognised text
/// - `exit:N` → exit N with garbage body
/// - `crlf-bom-active:PORT` → UTF-8 BOM + CRLF + mixed-case Active line
/// - `pre-0.11` → usage/help + explicit `unknown command: daemon` (Inactive)
/// - `crash` → exit 127, empty output (Unknown)
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
    String::new()
}

fn park_holding_stdio() {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Leave a child/grandchild holding inherited stdout/stderr, then hang so the
/// probe times out and kills only this PID. Unbounded reader joins would block.
fn spawn_stdio_holder_then_hang() -> Result<ExitCode, String> {
    #[cfg(unix)]
    {
        // fork avoids std::process::Command (clippy disallowed) in this test bin.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(format!(
                "fork hold-stdio: {}",
                std::io::Error::last_os_error()
            ));
        }
        if pid == 0 {
            park_holding_stdio();
            std::process::exit(0);
        }
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    #[cfg(windows)]
    {
        use std::process::{Command, Stdio};
        let exe = env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
        // Test-only self-reexec; PolicyGate does not apply inside lounge-test-helper.
        #[allow(clippy::disallowed_methods)]
        let mut child = Command::new(&exe);
        child
            .arg("hold-stdio")
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());
        let _child = child
            .spawn()
            .map_err(|e| format!("spawn hold-stdio grandchild: {e}"))?;
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err("hang-pipe-holder unsupported on this OS".into())
    }
}

fn print_active(label: &str, port_or_disabled: &str, crlf_bom: bool) -> Result<ExitCode, String> {
    let pid = std::process::id();
    let mut body = format!(
        "daemon: active ({label})\n  pid: {pid}\n  build: 0.11.0 (fakehelper000...)\n  committed clients: 0\n"
    );
    if port_or_disabled.eq_ignore_ascii_case("disabled") {
        body.push_str("  ui: disabled (enable with `daemon start` in a UI build)\n");
    } else {
        let port: u16 = port_or_disabled
            .parse()
            .map_err(|e| format!("bad daemon UI port {port_or_disabled:?}: {e}"))?;
        body.push_str(&format!(
            "  ui: configured at http://127.0.0.1:{port} (readiness not checked; `daemon start --open` verifies it)\n"
        ));
    }
    if crlf_bom {
        // Mixed case + BOM + CRLF for parser robustness tests.
        let mixed = body.replace("daemon: active", "Daemon: Active");
        let crlf = mixed.replace('\n', "\r\n");
        let mut out = Vec::from(b"\xEF\xBB\xBF".as_slice());
        out.extend_from_slice(crlf.as_bytes());
        let mut stdout = std::io::stdout();
        stdout
            .write_all(&out)
            .map_err(|e| format!("write stdout: {e}"))?;
        let _ = stdout.flush();
    } else {
        print!("{body}");
    }
    Ok(ExitCode::SUCCESS)
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
    if spec.eq_ignore_ascii_case("hang") {
        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }
    if spec.eq_ignore_ascii_case("hang-pipe-holder") {
        spawn_stdio_holder_then_hang()?;
    }
    if spec.eq_ignore_ascii_case("empty") {
        return Ok(ExitCode::SUCCESS);
    }
    if spec.eq_ignore_ascii_case("garbage") {
        println!("this is not a daemon status blob");
        return Ok(ExitCode::from(2));
    }
    if spec.eq_ignore_ascii_case("crash") {
        return Ok(ExitCode::from(127));
    }
    if spec.eq_ignore_ascii_case("not-responding") {
        println!("daemon: not responding (endpoint held by pid 12)");
        println!("hint: terminate pid 12, then run daemon start");
        return Ok(ExitCode::from(1));
    }
    if spec.eq_ignore_ascii_case("pre-0.11") {
        println!("codebase-memory-mcp 0.10.8");
        println!();
        println!("Usage:");
        println!("  codebase-memory-mcp              Run MCP server on stdio");
        println!("  codebase-memory-mcp --ui=true    Enable HTTP graph visualization");
        eprintln!("unknown command: daemon");
        return Ok(ExitCode::from(1));
    }
    if let Some(code) = spec.strip_prefix("exit:") {
        let code: u8 = code
            .parse()
            .map_err(|e| format!("bad exit code {code:?}: {e}"))?;
        println!("nope");
        return Ok(ExitCode::from(code));
    }
    if let Some(port) = spec.strip_prefix("crlf-bom-active:") {
        return print_active("permanent", port, true);
    }

    let parts: Vec<&str> = spec.split(':').collect();
    if parts.first().copied() != Some("active") {
        return Err(format!("bad lounge-fake-cbm-daemon marker: {spec}"));
    }
    let kind = parts.get(1).copied().unwrap_or("permanent");
    let permanent = kind.eq_ignore_ascii_case("permanent");
    let label = if permanent {
        "permanent"
    } else {
        "session-managed"
    };
    let port_or_disabled = parts.get(2).copied().unwrap_or("disabled");
    print_active(label, port_or_disabled, false)
}

/// Pollute a sibling cache dir (no process-wide `CBM_CACHE_DIR` env).
fn maybe_pollute_cbm_config(port: u16) -> Result<(), String> {
    let exe = env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let cache = exe
        .parent()
        .ok_or_else(|| "binary has no parent".to_string())?
        .join("lounge-fake-cbm-cache");
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
