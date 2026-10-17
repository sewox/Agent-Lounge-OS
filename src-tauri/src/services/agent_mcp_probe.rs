//! HU4: on-demand loopback-only probe for URL-only agent MCP entries.
//!
//! Never takes a URL from the UI. Never follows redirects. Never logs path,
//! query, headers, or tokens — only scheme + host + port.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::services::agent_health::{lookup_agent_mcp_url, AgentHealthRoots};
use crate::services::plugin::lounge_workspace;

pub const PROBE_OUTCOME_REACHABLE: &str = "reachable";
pub const PROBE_OUTCOME_UNREACHABLE: &str = "unreachable";
pub const PROBE_OUTCOME_NOT_PROBED_REMOTE: &str = "not_probed_remote";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const TOTAL_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentMcpProbeResult {
    /// `reachable` | `unreachable` | `not_probed_remote`
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

/// True when the URL host is loopback (127.0.0.1, ::1, or localhost).
/// Host string check only — no DNS. Windows localhost→::1 resolution is
/// irrelevant because we never connect to a non-loopback host string.
pub fn is_loopback_host(host: &str) -> bool {
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
    let parsed = Url::parse(raw_url.trim()).map_err(|_| AgentMcpProbeResult {
        outcome: PROBE_OUTCOME_NOT_PROBED_REMOTE.into(),
        elapsed_ms: 0,
    })?;
    let auth = SafeUrlAuthority::from_url(&parsed).ok_or_else(|| AgentMcpProbeResult {
        outcome: PROBE_OUTCOME_NOT_PROBED_REMOTE.into(),
        elapsed_ms: 0,
    })?;
    if !is_loopback_host(&auth.host) {
        return Err(AgentMcpProbeResult {
            outcome: PROBE_OUTCOME_NOT_PROBED_REMOTE.into(),
            elapsed_ms: 0,
        });
    }
    Ok((parsed, auth))
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
                outcome: PROBE_OUTCOME_UNREACHABLE.into(),
                elapsed_ms: 0,
            });
        }
    };

    let (parsed, auth) = match classify_probe_url(&url) {
        Ok(v) => v,
        Err(remote) => {
            log::info!(
                "agent_mcp_probe outcome={} (non-loopback; no connect)",
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
    let client = match reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
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
    use std::sync::{Mutex, OnceLock};

    fn log_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
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

    #[test]
    fn loopback_hosts_accepted_including_windows_localhost() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("[::1]"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("LOCALHOST"));
        // 127.0.0.2 is still IPv4 loopback per RFC
        assert!(is_loopback_host("127.0.0.2"));
        assert!(!is_loopback_host("example.com"));
        assert!(!is_loopback_host("8.8.8.8"));
        assert!(!is_loopback_host("192.168.1.1"));
        assert!(!is_loopback_host("10.0.0.1"));
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
        assert!(is_loopback_host(&local.1.host));
        // Safe label must not include path/query
        let label = local.1.log_label();
        assert!(!label.contains("path"));
        assert!(!label.contains("q=1"));
        assert!(!label.contains("frag"));
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

    #[tokio::test]
    async fn never_connects_to_non_loopback() {
        let _g = log_lock().lock().unwrap();
        let (roots, base) = temp_roots("remote");
        let cfg = roots.home.join(".cursor/mcp.json");
        fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        fs::write(
            &cfg,
            serde_json::to_string_pretty(&json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "url": "https://api.example.com/mcp?token=supersecret"
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
        assert_eq!(result.outcome, PROBE_OUTCOME_NOT_PROBED_REMOTE);
        assert_eq!(result.elapsed_ms, 0);
        let _ = fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn loopback_url_classified_before_connect_flag() {
        let (roots, base) = temp_roots("loop");
        let cfg = roots.home.join(".cursor/mcp.json");
        fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        fs::write(
            &cfg,
            serde_json::to_string_pretty(&json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "url": "http://127.0.0.1:18791/mcp"
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let homes = roots.mask_homes();
        let id = mask_home_path_with(&cfg.to_string_lossy(), &homes);
        // do_connect=false: prove we accept loopback without needing a live server.
        let result = probe_agent_mcp_url_with("cursor", &id, &roots, false)
            .await
            .unwrap();
        assert_eq!(result.outcome, PROBE_OUTCOME_REACHABLE);
        let _ = fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn windows_style_localhost_host_is_loopback() {
        let (roots, base) = temp_roots("win-local");
        let cfg = roots.home.join(".cursor/mcp.json");
        fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        // On Windows, localhost often resolves to ::1 — we still treat the
        // host *string* as loopback and would connect (here: dry-run).
        fs::write(
            &cfg,
            serde_json::to_string_pretty(&json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "url": "http://localhost:18791/mcp"
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let homes = roots.mask_homes();
        let id = mask_home_path_with(&cfg.to_string_lossy(), &homes);
        let result = probe_agent_mcp_url_with("cursor", &id, &roots, false)
            .await
            .unwrap();
        assert_eq!(result.outcome, PROBE_OUTCOME_REACHABLE);
        let _ = fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn connect_timeout_yields_unreachable() {
        // Bind nothing on a high loopback port; expect unreachable within total timeout.
        let (roots, base) = temp_roots("timeout");
        let cfg = roots.home.join(".cursor/mcp.json");
        fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        // Port 1 is privileged / almost never listening for HTTP on loopback in CI.
        fs::write(
            &cfg,
            serde_json::to_string_pretty(&json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "url": "http://127.0.0.1:1/mcp"
                    }
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let homes = roots.mask_homes();
        let id = mask_home_path_with(&cfg.to_string_lossy(), &homes);
        let started = Instant::now();
        let result = probe_agent_mcp_url_with("cursor", &id, &roots, true)
            .await
            .unwrap();
        let elapsed = started.elapsed();
        assert_eq!(result.outcome, PROBE_OUTCOME_UNREACHABLE);
        assert!(
            elapsed <= Duration::from_secs(5),
            "probe must respect timeouts, took {elapsed:?}"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn log_label_never_includes_secret_material() {
        let url = Url::parse("https://127.0.0.1:443/v1/mcp?authorization=Bearer%20sekrit&path=/a")
            .unwrap();
        let auth = SafeUrlAuthority::from_url(&url).unwrap();
        let label = auth.log_label();
        assert!(!label.to_lowercase().contains("bearer"));
        assert!(!label.contains("sekrit"));
        assert!(!label.contains("authorization"));
        assert!(!label.contains("/v1"));
        assert_eq!(label, "https://127.0.0.1:443");
    }
}
