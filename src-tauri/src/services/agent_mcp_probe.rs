//! HU4: on-demand loopback-only probe for URL-only agent MCP entries.
//!
//! Never takes a URL from the UI. Never follows redirects. Never logs path,
//! query, headers, or tokens — only scheme + host + port.
//!
//! SSRF guards: `.no_proxy()`, http(s) only, and `localhost` pinned to
//! loopback addresses (never free DNS).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::services::agent_health::{lookup_agent_mcp_url, AgentHealthRoots};
use crate::services::plugin::lounge_workspace;

pub const PROBE_OUTCOME_REACHABLE: &str = "reachable";
pub const PROBE_OUTCOME_UNREACHABLE: &str = "unreachable";
pub const PROBE_OUTCOME_NOT_PROBED_REMOTE: &str = "not_probed_remote";
pub const PROBE_OUTCOME_CONFIG_MISSING: &str = "config_missing";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMcpProbeResult {
    /// `reachable` | `unreachable` | `not_probed_remote` | `config_missing`
    pub outcome: String,
    pub elapsed_ms: u64,
}

/// Safe log target: scheme + host + port only (no path/query/userinfo).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SafeUrlAuthority {
    pub scheme: String,
    pub host: String,
    pub port: u16,
}

impl SafeUrlAuthority {
    pub fn from_url(url: &Url) -> Option<Self> {
        let host = url.host_str()?.to_string();
        let port = url.port_or_known_default()?;
        Some(Self {
            scheme: url.scheme().to_string(),
            host,
            port,
        })
    }

    pub fn log_label(&self) -> String {
        format!("{}://{}:{}", self.scheme, self.host, self.port)
    }
}

fn remote_result() -> AgentMcpProbeResult {
    AgentMcpProbeResult {
        outcome: PROBE_OUTCOME_NOT_PROBED_REMOTE.into(),
        elapsed_ms: 0,
    }
}

/// Literal loopback IP, or the name `localhost` (pinned later — never free DNS).
pub fn is_allowed_probe_host(host: &str) -> bool {
    let h = host.trim().trim_matches(|c| c == '[' || c == ']');
    if h.eq_ignore_ascii_case("localhost") {
        return true;
    }
    match h.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_loopback(),
        Ok(IpAddr::V6(v6)) => v6.is_loopback(),
        Err(_) => false,
    }
}

/// Parse URL and decide whether probing is allowed. Does not connect.
pub fn classify_probe_url(raw_url: &str) -> Result<(Url, SafeUrlAuthority), AgentMcpProbeResult> {
    let parsed = Url::parse(raw_url.trim()).map_err(|_| remote_result())?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(remote_result());
    }
    let auth = SafeUrlAuthority::from_url(&parsed).ok_or_else(remote_result)?;
    if !is_allowed_probe_host(&auth.host) {
        return Err(remote_result());
    }
    Ok((parsed, auth))
}

/// Build a reqwest client that never uses env/system proxies and pins
/// `localhost` to loopback IPs (127.0.0.1 and ::1). Port in SocketAddr is
/// ignored by reqwest — the URL port is always used.
pub fn build_probe_client() -> Result<reqwest::Client, reqwest::Error> {
    let loopback_v4 = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let loopback_v6 = SocketAddr::from((Ipv6Addr::LOCALHOST, 0));
    reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        // Pin `localhost` — never free DNS / hosts-file hijack.
        .resolve_to_addrs("localhost", &[loopback_v4, loopback_v6])
        .build()
}

/// Re-read URL from config on disk (never from UI) and probe if loopback.
pub async fn probe_agent_mcp_url(
    client: &str,
    config_path_id: &str,
) -> Result<AgentMcpProbeResult, String> {
    let roots = AgentHealthRoots::from_env(lounge_workspace());
    probe_agent_mcp_url_with(client, config_path_id, &roots, true).await
}

/// Injectable variant for tests (`do_connect=false` skips the HTTP call after classification).
pub async fn probe_agent_mcp_url_with(
    client: &str,
    config_path_id: &str,
    roots: &AgentHealthRoots,
    do_connect: bool,
) -> Result<AgentMcpProbeResult, String> {
    let url = match lookup_agent_mcp_url(client, config_path_id, roots)? {
        Some(u) => u,
        None => {
            return Ok(AgentMcpProbeResult {
                outcome: PROBE_OUTCOME_CONFIG_MISSING.into(),
                elapsed_ms: 0,
            });
        }
    };

    let (parsed, auth) = match classify_probe_url(&url) {
        Ok(v) => v,
        Err(remote) => {
            log::info!(
                "agent_mcp_probe outcome={} (rejected; no connect)",
                remote.outcome
            );
            return Ok(remote);
        }
    };

    log::info!(
        "agent_mcp_probe target={} connect={}",
        auth.log_label(),
        do_connect
    );

    if !do_connect {
        return Ok(AgentMcpProbeResult {
            outcome: PROBE_OUTCOME_REACHABLE.into(),
            elapsed_ms: 0,
        });
    }

    Ok(perform_loopback_probe(&parsed, &auth).await)
}

async fn perform_loopback_probe(url: &Url, auth: &SafeUrlAuthority) -> AgentMcpProbeResult {
    let started = Instant::now();
    let client = match build_probe_client() {
        Ok(c) => c,
        Err(_) => {
            log::warn!(
                "agent_mcp_probe client_build_failed target={}",
                auth.log_label()
            );
            return AgentMcpProbeResult {
                outcome: PROBE_OUTCOME_UNREACHABLE.into(),
                elapsed_ms: started.elapsed().as_millis() as u64,
            };
        }
    };

    // Single request; no redirects; never log response body or headers.
    let result = client.get(url.clone()).send().await;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(resp) => {
            // Any TCP+HTTP response counts as reachable (even 4xx/5xx).
            let _ = resp.status();
            log::info!(
                "agent_mcp_probe outcome={} target={} ms={}",
                PROBE_OUTCOME_REACHABLE,
                auth.log_label(),
                elapsed_ms
            );
            AgentMcpProbeResult {
                outcome: PROBE_OUTCOME_REACHABLE.into(),
                elapsed_ms,
            }
        }
        Err(_) => {
            log::info!(
                "agent_mcp_probe outcome={} target={} ms={}",
                PROBE_OUTCOME_UNREACHABLE,
                auth.log_label(),
                elapsed_ms
            );
            AgentMcpProbeResult {
                outcome: PROBE_OUTCOME_UNREACHABLE.into(),
                elapsed_ms,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::agent_health::AgentHealthRoots;
    use crate::services::health_filters::mask_home_path_with;
    use serde_json::json;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, Once, OnceLock};
    use std::thread;

    /// Thread-safe log line capture for asserting emitted probe logs.
    fn captured_logs() -> &'static Mutex<Vec<String>> {
        static LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
        LOGS.get_or_init(|| Mutex::new(Vec::new()))
    }

    struct CaptureLogger;

    impl log::Log for CaptureLogger {
        fn enabled(&self, metadata: &log::Metadata) -> bool {
            metadata.target().contains("agent_mcp_probe")
                || metadata
                    .target()
                    .contains("services::agent_mcp_probe")
        }

        fn log(&self, record: &log::Record) {
            if self.enabled(record.metadata()) {
                if let Ok(mut g) = captured_logs().lock() {
                    g.push(format!("{}", record.args()));
                }
            }
        }

        fn flush(&self) {}
    }

    fn ensure_log_capture() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = log::set_boxed_logger(Box::new(CaptureLogger));
            log::set_max_level(log::LevelFilter::Info);
        });
    }

    fn clear_captured_logs() {
        ensure_log_capture();
        if let Ok(mut g) = captured_logs().lock() {
            g.clear();
        }
    }

    fn take_captured_logs() -> Vec<String> {
        ensure_log_capture();
        captured_logs()
            .lock()
            .map(|g| g.clone())
            .unwrap_or_default()
    }

    fn temp_roots(tag: &str) -> (AgentHealthRoots, std::path::PathBuf) {
        let base =
            std::env::temp_dir().join(format!("agent-mcp-probe-{}-{}", tag, uuid::Uuid::new_v4()));
        let home = base.join("home");
        fs::create_dir_all(&home).unwrap();
        let roots = AgentHealthRoots {
            home: home.clone(),
            appdata: None,
            xdg_config: Some(home.join(".config")),
            workspace: base.join("ws"),
            path_dirs: Vec::new(),
        };
        fs::create_dir_all(&roots.workspace).unwrap();
        (roots, base)
    }

    fn write_url_only_config(roots: &AgentHealthRoots, url: &str) -> String {
        let cfg = roots.home.join(".cursor/mcp.json");
        fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        fs::write(
            &cfg,
            serde_json::to_string_pretty(&json!({
                "mcpServers": {
                    "agent-lounge-os": { "url": url }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let homes = roots.mask_homes();
        mask_home_path_with(&cfg.to_string_lossy(), &homes)
    }

    #[test]
    fn loopback_hosts_accepted_including_windows_localhost() {
        assert!(is_allowed_probe_host("127.0.0.1"));
        assert!(is_allowed_probe_host("::1"));
        assert!(is_allowed_probe_host("[::1]"));
        assert!(is_allowed_probe_host("localhost"));
        assert!(is_allowed_probe_host("LOCALHOST"));
        assert!(is_allowed_probe_host("127.0.0.2"));
        assert!(!is_allowed_probe_host("example.com"));
        assert!(!is_allowed_probe_host("8.8.8.8"));
        assert!(!is_allowed_probe_host("192.168.1.1"));
        assert!(!is_allowed_probe_host("10.0.0.1"));
    }

    #[test]
    fn classify_rejects_non_http_schemes() {
        let err = classify_probe_url("ftp://127.0.0.1/mcp").unwrap_err();
        assert_eq!(err.outcome, PROBE_OUTCOME_NOT_PROBED_REMOTE);
        let err = classify_probe_url("file:///etc/passwd").unwrap_err();
        assert_eq!(err.outcome, PROBE_OUTCOME_NOT_PROBED_REMOTE);
        let err = classify_probe_url("ws://127.0.0.1:9/mcp").unwrap_err();
        assert_eq!(err.outcome, PROBE_OUTCOME_NOT_PROBED_REMOTE);
    }

    #[test]
    fn classify_rejects_remote_without_connect() {
        let err = classify_probe_url("https://evil.example/mcp?token=secret").unwrap_err();
        assert_eq!(err.outcome, PROBE_OUTCOME_NOT_PROBED_REMOTE);
        let ok = classify_probe_url("http://127.0.0.1:18791/mcp").unwrap();
        assert_eq!(ok.1.host, "127.0.0.1");
        assert_eq!(ok.1.port, 18791);
        assert_eq!(ok.1.scheme, "http");
        let local = classify_probe_url("http://localhost:18791/path?q=1#frag").unwrap();
        assert!(is_allowed_probe_host(&local.1.host));
        let label = local.1.log_label();
        assert!(!label.contains("path"));
        assert!(!label.contains("q=1"));
        assert!(!label.contains("token"));
    }

    #[test]
    fn safe_authority_strips_userinfo_path_query() {
        let url = Url::parse("http://user:token@127.0.0.1:18791/mcp?x=1").unwrap();
        let auth = SafeUrlAuthority::from_url(&url).unwrap();
        let label = auth.log_label();
        assert_eq!(label, "http://127.0.0.1:18791");
        assert!(!label.contains("user"));
        assert!(!label.contains("token"));
        assert!(!label.contains("mcp"));
        assert!(!label.contains("x=1"));
    }

    #[test]
    fn build_probe_client_sets_no_proxy() {
        // Builder succeeds with HTTP_PROXY set — client must still build (no_proxy).
        let prev = std::env::var_os("HTTP_PROXY");
        // SAFETY: test-only env mutation; restored below.
        unsafe {
            std::env::set_var("HTTP_PROXY", "http://127.0.0.1:9");
            std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:9");
            std::env::set_var("ALL_PROXY", "http://127.0.0.1:9");
        }
        let client = build_probe_client();
        unsafe {
            match prev {
                Some(v) => std::env::set_var("HTTP_PROXY", v),
                None => std::env::remove_var("HTTP_PROXY"),
            }
            std::env::remove_var("HTTPS_PROXY");
            std::env::remove_var("ALL_PROXY");
        }
        assert!(client.is_ok(), "no_proxy client must build with HTTP_PROXY set");
    }

    #[tokio::test]
    async fn http_proxy_env_is_not_used_for_loopback_probe() {
        // Bind a real loopback server; with HTTP_PROXY pointing at a dead port,
        // a proxied client would fail. no_proxy must still reach 127.0.0.1.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_bg = hits.clone();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                hits_bg.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            }
        });

        let prev_http = std::env::var_os("HTTP_PROXY");
        let prev_https = std::env::var_os("HTTPS_PROXY");
        let prev_all = std::env::var_os("ALL_PROXY");
        // Point proxies at an unused loopback port — proxied traffic would fail.
        // SAFETY: test-only env mutation; restored below.
        unsafe {
            std::env::set_var("HTTP_PROXY", "http://127.0.0.1:1");
            std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:1");
            std::env::set_var("ALL_PROXY", "http://127.0.0.1:1");
        }

        let (roots, base) = temp_roots("proxy");
        let id = write_url_only_config(
            &roots,
            &format!("http://127.0.0.1:{}/mcp?token=sekrit", addr.port()),
        );
        let result = probe_agent_mcp_url_with("cursor", &id, &roots, true)
            .await
            .unwrap();

        // Restore env before asserts so later tests are clean.
        unsafe {
            match prev_http {
                Some(v) => std::env::set_var("HTTP_PROXY", v),
                None => std::env::remove_var("HTTP_PROXY"),
            }
            match prev_https {
                Some(v) => std::env::set_var("HTTPS_PROXY", v),
                None => std::env::remove_var("HTTPS_PROXY"),
            }
            match prev_all {
                Some(v) => std::env::set_var("ALL_PROXY", v),
                None => std::env::remove_var("ALL_PROXY"),
            }
        }

        assert_eq!(result.outcome, PROBE_OUTCOME_REACHABLE);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let _ = fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn never_connects_to_non_loopback() {
        // No std mutex held across await (clippy await_holding_lock).
        let (roots, base) = temp_roots("remote");
        let id = write_url_only_config(
            &roots,
            "https://api.example.com/mcp?token=supersecret",
        );
        let result = probe_agent_mcp_url_with("cursor", &id, &roots, true)
            .await
            .unwrap();
        assert_eq!(result.outcome, PROBE_OUTCOME_NOT_PROBED_REMOTE);
        assert_eq!(result.elapsed_ms, 0);
        let _ = fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn localhost_is_pinned_and_reachable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_bg = hits.clone();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                hits_bg.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\n\r\n");
            }
        });

        let (roots, base) = temp_roots("localhost-pin");
        // Host string is localhost — client must pin to 127.0.0.1, not free DNS.
        let id = write_url_only_config(&roots, &format!("http://localhost:{}/mcp", addr.port()));
        let result = probe_agent_mcp_url_with("cursor", &id, &roots, true)
            .await
            .unwrap();
        assert_eq!(result.outcome, PROBE_OUTCOME_REACHABLE);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let _ = fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn redirect_is_not_followed() {
        let redirect_hits = Arc::new(AtomicUsize::new(0));
        let target_hits = Arc::new(AtomicUsize::new(0));

        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        let target_addr = target.local_addr().unwrap();
        let th = target_hits.clone();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = target.accept() {
                th.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
            }
        });

        let source = TcpListener::bind("127.0.0.1:0").unwrap();
        let source_addr = source.local_addr().unwrap();
        let rh = redirect_hits.clone();
        let loc = format!("http://127.0.0.1:{}/secret", target_addr.port());
        thread::spawn(move || {
            if let Ok((mut stream, _)) = source.accept() {
                rh.fetch_add(1, Ordering::SeqCst);
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 302 Found\r\nLocation: {loc}\r\nContent-Length: 0\r\n\r\n"
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let (roots, base) = temp_roots("redirect");
        let id = write_url_only_config(
            &roots,
            &format!("http://127.0.0.1:{}/start", source_addr.port()),
        );
        let result = probe_agent_mcp_url_with("cursor", &id, &roots, true)
            .await
            .unwrap();
        // 302 is still a TCP+HTTP response → reachable; redirect must not be followed.
        assert_eq!(result.outcome, PROBE_OUTCOME_REACHABLE);
        assert_eq!(redirect_hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            target_hits.load(Ordering::SeqCst),
            0,
            "redirect target must never be contacted"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn hanging_server_hits_total_timeout() {
        // Accept then never respond — exercises total timeout (~3s), not refused port.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                // Hold the connection open without writing a response.
                thread::sleep(Duration::from_secs(10));
                drop(stream);
            }
        });

        let (roots, base) = temp_roots("hang");
        let id = write_url_only_config(&roots, &format!("http://127.0.0.1:{}/hang", addr.port()));
        let started = Instant::now();
        let result = probe_agent_mcp_url_with("cursor", &id, &roots, true)
            .await
            .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(result.outcome, PROBE_OUTCOME_UNREACHABLE);
        assert!(
            elapsed >= Duration::from_millis(2500),
            "expected ~3s total timeout, got {elapsed:?}"
        );
        assert!(
            elapsed <= Duration::from_secs(6),
            "timeout budget exceeded: {elapsed:?}"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn exactly_one_request_reaches_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let hits_bg = hits.clone();
        thread::spawn(move || {
            // Accept at most a few connections; count HTTP request starts.
            for _ in 0..3 {
                if let Ok((mut stream, _)) = listener.accept() {
                    hits_bg.fetch_add(1, Ordering::SeqCst);
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf);
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                }
            }
        });

        let (roots, base) = temp_roots("one-req");
        let id = write_url_only_config(
            &roots,
            &format!("http://127.0.0.1:{}/once?token=abc", addr.port()),
        );
        let result = probe_agent_mcp_url_with("cursor", &id, &roots, true)
            .await
            .unwrap();
        assert_eq!(result.outcome, PROBE_OUTCOME_REACHABLE);
        // Brief wait for any illicit follow-up.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        let _ = fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn reachable_response_is_reachable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
            }
        });
        let (roots, base) = temp_roots("ok");
        let id = write_url_only_config(&roots, &format!("http://127.0.0.1:{}/", addr.port()));
        let result = probe_agent_mcp_url_with("cursor", &id, &roots, true)
            .await
            .unwrap();
        assert_eq!(result.outcome, PROBE_OUTCOME_REACHABLE);
        let _ = fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn emitted_logs_contain_no_path_query_or_token() {
        clear_captured_logs();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            }
        });
        let (roots, base) = temp_roots("logs");
        let secret_path = format!(
            "http://127.0.0.1:{}/v1/mcp?authorization=Bearer%20sekrit&x=1",
            addr.port()
        );
        let id = write_url_only_config(&roots, &secret_path);
        let _ = probe_agent_mcp_url_with("cursor", &id, &roots, true)
            .await
            .unwrap();
        let lines = take_captured_logs();
        assert!(
            !lines.is_empty(),
            "expected probe log lines to be captured"
        );
        let joined = lines.join("\n");
        assert!(
            !joined.contains("/v1/mcp"),
            "path leaked in logs: {joined}"
        );
        assert!(
            !joined.contains("authorization"),
            "query key leaked: {joined}"
        );
        assert!(!joined.contains("sekrit"), "token leaked: {joined}");
        assert!(!joined.contains("Bearer"), "token leaked: {joined}");
        assert!(
            joined.contains(&format!("http://127.0.0.1:{}", addr.port())),
            "expected scheme+host+port in logs: {joined}"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn missing_config_returns_config_missing() {
        let (roots, base) = temp_roots("missing");
        let result = probe_agent_mcp_url_with("cursor", "~/no-such.json", &roots, true)
            .await
            .unwrap();
        assert_eq!(result.outcome, PROBE_OUTCOME_CONFIG_MISSING);
        let _ = fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn command_plus_url_entry_is_not_probed_as_url_only() {
        let (roots, base) = temp_roots("not-url-only");
        let cfg = roots.home.join(".cursor/mcp.json");
        fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        fs::write(
            &cfg,
            serde_json::to_string_pretty(&json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "command": "lounge-mcp",
                        "url": "http://127.0.0.1:18791/mcp",
                        "env": { "LOUNGE_MCP_URL": "http://127.0.0.1:18791" }
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let homes = roots.mask_homes();
        let id = mask_home_path_with(&cfg.to_string_lossy(), &homes);
        let result = probe_agent_mcp_url_with("cursor", &id, &roots, true)
            .await
            .unwrap();
        assert_eq!(result.outcome, PROBE_OUTCOME_CONFIG_MISSING);
        let _ = fs::remove_dir_all(&base);
    }
}
