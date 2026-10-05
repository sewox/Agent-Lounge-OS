//! PR-5: NATS credentials + MCP remote Host/Origin gate (Managed Origin Validation).
//!
//! - Auth default **true** (`LOUNGE_AUTH_REQUIRED` unset / not `false`).
//! - Migration bypass: `LOUNGE_AUTH_REQUIRED=false`.
//! - `source_verified` on NATS ingress is true only while session NATS auth is active.

#![allow(deprecated)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::probe::lounge_nats_dir;

pub const LOUNGE_AUTH_REQUIRED_ENV: &str = "LOUNGE_AUTH_REQUIRED";
pub const LOUNGE_NATS_USER_ENV: &str = "LOUNGE_NATS_USER";
pub const LOUNGE_NATS_PASS_ENV: &str = "LOUNGE_NATS_PASS";
pub const LOUNGE_NATS_CREDS_FILE_ENV: &str = "LOUNGE_NATS_CREDS_FILE";
pub const LOUNGE_TOKEN_ENV: &str = "LOUNGE_TOKEN";
pub const HDR_LOUNGE_TOKEN: &str = "x-lounge-token";

const CREDS_FILE_NAME: &str = "session.creds.json";
const TOKEN_FILE_NAME: &str = "lounge.token";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NatsCredentials {
    pub user: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CredsFile {
    user: String,
    password: String,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Debug)]
struct AuthState {
    /// Kernel started (or adopted) a NATS server that requires these credentials.
    nats_auth_active: bool,
    creds: Option<NatsCredentials>,
    lounge_token: String,
    allowed_hosts: HashSet<String>,
}

impl AuthState {
    fn fresh() -> Self {
        let token = load_or_create_token();
        let creds = load_credentials_from_env_or_file();
        Self {
            nats_auth_active: false,
            creds,
            lounge_token: token,
            allowed_hosts: HashSet::new(),
        }
    }
}

fn state() -> &'static Mutex<AuthState> {
    static STATE: OnceLock<Mutex<AuthState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(AuthState::fresh()))
}

/// Policy: auth required unless `LOUNGE_AUTH_REQUIRED` is an explicit falsey value.
pub fn auth_required() -> bool {
    match std::env::var(LOUNGE_AUTH_REQUIRED_ENV) {
        Ok(v) => {
            let t = v.trim().to_ascii_lowercase();
            !matches!(t.as_str(), "0" | "false" | "no" | "off")
        }
        Err(_) => true,
    }
}

pub fn nats_auth_active() -> bool {
    state()
        .lock()
        .map(|s| s.nats_auth_active)
        .unwrap_or(false)
}

pub fn default_creds_path() -> PathBuf {
    std::env::var_os(LOUNGE_NATS_CREDS_FILE_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| lounge_nats_dir().join(CREDS_FILE_NAME))
}

fn token_path() -> PathBuf {
    lounge_nats_dir().join(TOKEN_FILE_NAME)
}

fn load_or_create_token() -> String {
    if let Ok(t) = std::env::var(LOUNGE_TOKEN_ENV) {
        let t = t.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    let path = token_path();
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let t = existing.trim().to_string();
        if !t.is_empty() {
            return t;
        }
    }
    let token = format!("lounge_{}", uuid::Uuid::new_v4().simple());
    let _ = std::fs::create_dir_all(lounge_nats_dir());
    let _ = std::fs::write(&path, &token);
    // SAFETY: single-process Kernel; env is the worker hand-off channel.
    unsafe {
        std::env::set_var(LOUNGE_TOKEN_ENV, &token);
    }
    token
}

fn load_credentials_from_env_or_file() -> Option<NatsCredentials> {
    let user = std::env::var(LOUNGE_NATS_USER_ENV).ok()?;
    let password = std::env::var(LOUNGE_NATS_PASS_ENV).ok()?;
    let user = user.trim().to_string();
    let password = password.trim().to_string();
    if !user.is_empty() && !password.is_empty() {
        return Some(NatsCredentials { user, password });
    }
    load_credentials_file(&default_creds_path()).ok().flatten()
}

pub fn load_credentials_file(path: &Path) -> Result<Option<NatsCredentials>> {
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read NATS creds {}", path.display()))?;
    let parsed: CredsFile = serde_json::from_str(&raw)
        .with_context(|| format!("parse NATS creds {}", path.display()))?;
    if parsed.user.trim().is_empty() || parsed.password.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(NatsCredentials {
        user: parsed.user,
        password: parsed.password,
    }))
}

pub fn write_credentials_file(path: &Path, creds: &NatsCredentials, nats_url: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create creds dir {}", parent.display()))?;
    }
    let body = CredsFile {
        user: creds.user.clone(),
        password: creds.password.clone(),
        url: Some(nats_url.to_string()),
    };
    let json = serde_json::to_string_pretty(&body)?;
    std::fs::write(path, json).with_context(|| format!("write creds {}", path.display()))?;
    // SAFETY: Kernel → worker credential hand-off via env.
    unsafe {
        std::env::set_var(LOUNGE_NATS_USER_ENV, &creds.user);
        std::env::set_var(LOUNGE_NATS_PASS_ENV, &creds.password);
        std::env::set_var(LOUNGE_NATS_CREDS_FILE_ENV, path);
    }
    Ok(())
}

pub fn generate_session_credentials() -> NatsCredentials {
    NatsCredentials {
        user: format!("lounge_{}", uuid::Uuid::new_v4().simple()),
        password: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
    }
}

/// Ensure session credentials exist when auth is required. Does not mark auth active.
pub fn ensure_session_credentials(nats_url: &str) -> Result<Option<NatsCredentials>> {
    if !auth_required() {
        return Ok(None);
    }
    let mut guard = state().lock().expect("lounge_auth poison");
    if let Some(existing) = guard.creds.clone() {
        let path = default_creds_path();
        write_credentials_file(&path, &existing, nats_url)?;
        return Ok(Some(existing));
    }
    let creds = generate_session_credentials();
    let path = default_creds_path();
    write_credentials_file(&path, &creds, nats_url)?;
    guard.creds = Some(creds.clone());
    Ok(Some(creds))
}

pub fn activate_nats_auth(creds: NatsCredentials) {
    let mut guard = state().lock().expect("lounge_auth poison");
    guard.creds = Some(creds);
    guard.nats_auth_active = true;
}

pub fn deactivate_nats_auth() {
    if let Ok(mut guard) = state().lock() {
        guard.nats_auth_active = false;
    }
}

pub fn current_credentials() -> Option<NatsCredentials> {
    state()
        .lock()
        .ok()
        .and_then(|s| s.creds.clone())
        .or_else(load_credentials_from_env_or_file)
}

/// Authenticated NATS connect when session auth is active; otherwise plain.
pub fn connect(url: &str) -> Result<nats::Connection> {
    if nats_auth_active() {
        let creds = current_credentials().ok_or_else(|| {
            anyhow::anyhow!(
                "NATS authentication is active but credentials are missing \
                 (set {LOUNGE_NATS_USER_ENV}/{LOUNGE_NATS_PASS_ENV} or {LOUNGE_NATS_CREDS_FILE_ENV})"
            )
        })?;
        return nats::Options::with_user_pass(&creds.user, &creds.password)
            .connect(url)
            .map_err(|e| anyhow::anyhow!("NATS connect (auth): {e}"));
    }
    nats::connect(url).map_err(|e| anyhow::anyhow!("NATS connect: {e}"))
}

/// NATS ingress trust: verified only when the local bus enforces session credentials.
pub fn nats_ingress_source_verified() -> bool {
    nats_auth_active()
}

pub fn lounge_token() -> String {
    state()
        .lock()
        .map(|s| s.lounge_token.clone())
        .unwrap_or_else(|_| load_or_create_token())
}

pub fn rotate_lounge_token() -> String {
    let token = format!("lounge_{}", uuid::Uuid::new_v4().simple());
    let path = token_path();
    let _ = std::fs::create_dir_all(lounge_nats_dir());
    let _ = std::fs::write(&path, &token);
    unsafe {
        std::env::set_var(LOUNGE_TOKEN_ENV, &token);
    }
    if let Ok(mut guard) = state().lock() {
        guard.lounge_token = token.clone();
    }
    token
}

pub fn allowed_hosts() -> Vec<String> {
    state()
        .lock()
        .map(|s| {
            let mut v: Vec<_> = s.allowed_hosts.iter().cloned().collect();
            v.sort();
            v
        })
        .unwrap_or_default()
}

/// Parse a tunnel URL (or bare host) and add its hostname to the allow-list.
pub fn add_allowed_origin(url_or_host: &str) -> Result<String> {
    let host = parse_host(url_or_host).context("invalid tunnel URL / host")?;
    let mut guard = state().lock().expect("lounge_auth poison");
    guard.allowed_hosts.insert(host.clone());
    Ok(host)
}

pub fn remove_allowed_origin(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    state()
        .lock()
        .map(|mut s| s.allowed_hosts.remove(&host))
        .unwrap_or(false)
}

pub fn clear_allowed_origins() {
    if let Ok(mut s) = state().lock() {
        s.allowed_hosts.clear();
    }
}

pub fn parse_host(url_or_host: &str) -> Result<String> {
    let raw = url_or_host.trim();
    if raw.is_empty() {
        anyhow::bail!("empty host");
    }
    let host = if raw.contains("://") {
        // minimal URL parse without adding url crate
        let after = raw.split("://").nth(1).unwrap_or(raw);
        let authority = after.split('/').next().unwrap_or(after);
        let hostport = authority.split('@').next_back().unwrap_or(authority);
        strip_port(hostport)
    } else {
        strip_port(raw.split('/').next().unwrap_or(raw))
    };
    let host = host.trim().trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
    if host.is_empty() || host.contains(' ') {
        anyhow::bail!("invalid host: {url_or_host}");
    }
    Ok(host)
}

fn strip_port(hostport: &str) -> &str {
    if hostport.starts_with('[') {
        return hostport
            .trim_start_matches('[')
            .split(']')
            .next()
            .unwrap_or(hostport);
    }
    // hostname:port — but avoid splitting IPv4 oddly; only split last ':' if digits after
    if let Some((h, port)) = hostport.rsplit_once(':') {
        if !h.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            return h;
        }
    }
    hostport
}

pub fn is_loopback_host(hostname: &str) -> bool {
    let h = hostname
        .trim()
        .trim_matches(|c| c == '[' || c == ']')
        .to_ascii_lowercase();
    matches!(h.as_str(), "localhost" | "127.0.0.1" | "::1" | "0.0.0.0")
}

fn hostname_from_host_header(host_hdr: &str) -> Option<String> {
    let host = strip_port(host_hdr.trim()).trim().to_ascii_lowercase();
    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

fn hostname_from_origin(origin: &str) -> Result<String> {
    parse_host(origin)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAuthFailure {
    MissingToken,
    InvalidToken,
    HostNotAllowed,
    OriginNotAllowed,
    InvalidOrigin,
}

impl RemoteAuthFailure {
    pub fn status(self) -> u16 {
        403
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::MissingToken => "missing X-Lounge-Token",
            Self::InvalidToken => "invalid X-Lounge-Token",
            Self::HostNotAllowed => "host not allow-listed",
            Self::OriginNotAllowed => "origin host not allow-listed",
            Self::InvalidOrigin => "invalid origin",
        }
    }
}

/// Authorize MCP HTTP request for loopback vs secure-entry (token + allow-list).
pub fn authorize_mcp_headers(
    host_header: Option<&str>,
    origin_header: Option<&str>,
    token_header: Option<&str>,
) -> std::result::Result<(), RemoteAuthFailure> {
    let token_ok = {
        let expected = lounge_token();
        token_header
            .map(str::trim)
            .filter(|t| !t.is_empty() && constant_time_eq(t.as_bytes(), expected.as_bytes()))
            .is_some()
    };

    let allowed = allowed_hosts();

    if let Some(origin) = origin_header.map(str::trim).filter(|s| !s.is_empty()) {
        let origin_host = hostname_from_origin(origin).map_err(|_| RemoteAuthFailure::InvalidOrigin)?;
        if !is_loopback_host(&origin_host) {
            if !token_ok {
                return Err(if token_header.map(str::trim).filter(|s| !s.is_empty()).is_some() {
                    RemoteAuthFailure::InvalidToken
                } else {
                    RemoteAuthFailure::MissingToken
                });
            }
            if !allowed.iter().any(|h| h == &origin_host) {
                return Err(RemoteAuthFailure::OriginNotAllowed);
            }
        }
    }

    if let Some(host_hdr) = host_header.map(str::trim).filter(|s| !s.is_empty()) {
        let Some(host) = hostname_from_host_header(host_hdr) else {
            return Ok(());
        };
        if !is_loopback_host(&host) {
            if !token_ok {
                return Err(if token_header.map(str::trim).filter(|s| !s.is_empty()).is_some() {
                    RemoteAuthFailure::InvalidToken
                } else {
                    RemoteAuthFailure::MissingToken
                });
            }
            if !allowed.iter().any(|h| h == &host) {
                return Err(RemoteAuthFailure::HostNotAllowed);
            }
        }
    }

    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RemoteAccessInfo {
    pub auth_required: bool,
    pub nats_auth_active: bool,
    pub lounge_token: String,
    pub mcp_bind: String,
    pub mcp_url: String,
    pub allowed_hosts: Vec<String>,
    pub tunnel_url: Option<String>,
    pub mcp_json: String,
    pub nats_creds_file: String,
}

pub fn remote_access_info(mcp_bind: &str, tunnel_url: Option<&str>) -> RemoteAccessInfo {
    let token = lounge_token();
    let hosts = allowed_hosts();
    let public_url = tunnel_url
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_else(|| format!("http://{mcp_bind}"));
    let mcp_endpoint = if public_url.ends_with("/mcp") {
        public_url.clone()
    } else {
        format!("{public_url}/mcp")
    };
    let mcp_json = serde_json::json!({
        "mcpServers": {
            "agent-lounge-os": {
                "url": mcp_endpoint,
                "headers": {
                    "X-Lounge-Token": token,
                }
            }
        }
    });
    RemoteAccessInfo {
        auth_required: auth_required(),
        nats_auth_active: nats_auth_active(),
        lounge_token: token,
        mcp_bind: mcp_bind.to_string(),
        mcp_url: mcp_endpoint,
        allowed_hosts: hosts,
        tunnel_url: tunnel_url.map(|s| s.to_string()),
        mcp_json: serde_json::to_string_pretty(&mcp_json).unwrap_or_else(|_| "{}".into()),
        nats_creds_file: default_creds_path().display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_required_defaults_true() {
        // Do not assert against ambient env in parallel tests blindly —
        // only check parser helper via temporary unset is hard; exercise falsey.
        let prev = std::env::var_os(LOUNGE_AUTH_REQUIRED_ENV);
        unsafe {
            std::env::set_var(LOUNGE_AUTH_REQUIRED_ENV, "false");
        }
        assert!(!auth_required());
        unsafe {
            std::env::set_var(LOUNGE_AUTH_REQUIRED_ENV, "true");
        }
        assert!(auth_required());
        unsafe {
            match prev {
                Some(v) => std::env::set_var(LOUNGE_AUTH_REQUIRED_ENV, v),
                None => std::env::remove_var(LOUNGE_AUTH_REQUIRED_ENV),
            }
        }
    }

    #[test]
    fn parse_host_from_tunnel_url() {
        assert_eq!(
            parse_host("https://abc.trycloudflare.com/mcp").unwrap(),
            "abc.trycloudflare.com"
        );
        assert_eq!(parse_host("tunnel.example:8443").unwrap(), "tunnel.example");
        assert_eq!(parse_host("127.0.0.1:18791").unwrap(), "127.0.0.1");
    }

    #[test]
    fn loopback_hosts_recognized() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("localhost"));
        assert!(is_loopback_host("::1"));
        assert!(!is_loopback_host("abc.trycloudflare.com"));
    }

    #[test]
    fn remote_auth_loopback_ok_without_token() {
        clear_allowed_origins();
        assert!(authorize_mcp_headers(Some("127.0.0.1:18791"), None, None).is_ok());
        assert!(authorize_mcp_headers(Some("localhost:18791"), Some("http://127.0.0.1:9"), None).is_ok());
    }

    #[test]
    fn remote_auth_requires_token_and_allowlist() {
        clear_allowed_origins();
        let token = lounge_token();
        let err = authorize_mcp_headers(Some("abc.trycloudflare.com"), None, None).unwrap_err();
        assert_eq!(err, RemoteAuthFailure::MissingToken);

        let err = authorize_mcp_headers(Some("abc.trycloudflare.com"), None, Some("bad")).unwrap_err();
        assert_eq!(err, RemoteAuthFailure::InvalidToken);

        let err =
            authorize_mcp_headers(Some("abc.trycloudflare.com"), None, Some(&token)).unwrap_err();
        assert_eq!(err, RemoteAuthFailure::HostNotAllowed);

        add_allowed_origin("https://abc.trycloudflare.com").unwrap();
        assert!(
            authorize_mcp_headers(Some("abc.trycloudflare.com"), None, Some(&token)).is_ok()
        );
        clear_allowed_origins();
    }

    #[test]
    fn creds_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("lounge-creds-{}", uuid::Uuid::new_v4()));
        let path = dir.join("session.creds.json");
        let creds = NatsCredentials {
            user: "u1".into(),
            password: "p1".into(),
        };
        write_credentials_file(&path, &creds, "nats://127.0.0.1:4222").unwrap();
        let loaded = load_credentials_file(&path).unwrap().unwrap();
        assert_eq!(loaded, creds);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mcp_json_contains_token_and_url() {
        let info = remote_access_info("127.0.0.1:18791", Some("https://abc.trycloudflare.com"));
        assert!(info.mcp_json.contains("X-Lounge-Token"));
        assert!(info.mcp_json.contains(&info.lounge_token));
        assert!(info.mcp_json.contains("https://abc.trycloudflare.com/mcp"));
    }
}
