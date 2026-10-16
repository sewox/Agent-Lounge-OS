//! H3: read-only lounge-mcp entry health per desktop/CLI client.
//!
//! Never writes MCP configs. Checks only at caller request (startup + on-demand).
//! When several configs exist for one client, every path is reported (no "best" mask).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::probe::find_executable;
use crate::models::{AgentHealthEntry, AgentMcpStatus};
use crate::services::autodiscover::{
    find_antigravity, find_claude_cli, find_claude_desktop, find_cursor_app, find_grok_bot,
};
use crate::services::health_filters::mask_home_path;

/// Canonical MCP server key for Agent Lounge OS.
pub const LOUNGE_MCP_SERVER_KEY: &str = "agent-lounge-os";
/// Embedded version marker scanned from lounge-mcp binaries (tests + release).
pub const LOUNGE_MCP_VERSION_MARKER_PREFIX: &str = "LOUNGE_MCP_VERSION=";

/// Env / path roots for injectable tests (temp HOME / APPDATA).
#[derive(Debug, Clone)]
pub struct AgentHealthRoots {
    pub home: PathBuf,
    pub appdata: Option<PathBuf>,
    pub xdg_config: Option<PathBuf>,
    pub workspace: PathBuf,
}

impl AgentHealthRoots {
    pub fn from_env(workspace: PathBuf) -> Self {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
        let xdg_config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        Self {
            home,
            appdata,
            xdg_config,
            workspace,
        }
    }

    fn os_config_dir(&self, app: &str) -> PathBuf {
        if cfg!(target_os = "macos") {
            return self.home.join("Library/Application Support").join(app);
        }
        if cfg!(target_os = "windows") {
            if let Some(appdata) = &self.appdata {
                return appdata.join(app);
            }
            return self.home.join("AppData/Roaming").join(app);
        }
        if let Some(xdg) = &self.xdg_config {
            return xdg.join(app);
        }
        self.home.join(".config").join(app)
    }
}

#[derive(Debug, Clone)]
pub struct ExpectedLoungeMcp {
    pub version: String,
    pub sha256: Option<String>,
    pub path: Option<PathBuf>,
}

impl ExpectedLoungeMcp {
    pub fn current() -> Self {
        let version = env!("CARGO_PKG_VERSION").to_string();
        let path = resolve_expected_lounge_mcp_path();
        let sha256 = path.as_ref().and_then(|p| sha256_file_hex(p).ok());
        Self {
            version,
            sha256,
            path,
        }
    }
}

fn resolve_expected_lounge_mcp_path() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "lounge-mcp.exe"
    } else {
        "lounge-mcp"
    };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    find_executable("lounge-mcp")
}

/// Scan all first-class clients under `roots` (one entry per config path used).
pub fn check_all_agents(
    roots: &AgentHealthRoots,
    expected: &ExpectedLoungeMcp,
) -> Vec<AgentHealthEntry> {
    let mut out = Vec::new();
    out.extend(check_client_entries(
        "antigravity",
        client_installed_antigravity(),
        antigravity_config_paths(roots),
        false,
        expected,
    ));
    out.extend(check_client_entries(
        "cursor",
        client_installed_cursor(),
        cursor_config_paths(roots),
        true, // JSONC
        expected,
    ));
    out.extend(check_client_entries(
        "claude_desktop",
        client_installed_claude_desktop(),
        vec![claude_desktop_config_path(roots)],
        false,
        expected,
    ));
    out.extend(check_client_entries(
        "claude_code",
        client_installed_claude_code(),
        claude_code_config_paths(roots),
        false,
        expected,
    ));
    out.extend(check_client_entries(
        "grok_bot",
        client_installed_grok(),
        grok_config_paths(roots),
        false,
        expected,
    ));
    out.extend(check_client_entries(
        "vscode",
        client_installed_vscode(),
        vscode_config_paths(roots),
        true, // JSONC
        expected,
    ));
    out.extend(check_client_entries(
        "windsurf",
        client_installed_windsurf(),
        windsurf_config_paths(roots),
        true, // JSONC
        expected,
    ));
    out
}

fn client_installed_antigravity() -> bool {
    find_antigravity().is_some_and(|t| t.available)
}
fn client_installed_cursor() -> bool {
    find_cursor_app().is_some_and(|t| t.available)
}
fn client_installed_claude_desktop() -> bool {
    find_claude_desktop().is_some_and(|t| t.available)
}
fn client_installed_claude_code() -> bool {
    find_claude_cli().is_some_and(|t| t.available)
}
fn client_installed_grok() -> bool {
    find_grok_bot().is_some_and(|t| t.available)
}
fn client_installed_vscode() -> bool {
    find_executable("code").is_some()
        || find_executable("code.cmd").is_some()
        || roots_hint_vscode_app()
}
fn client_installed_windsurf() -> bool {
    find_executable("windsurf").is_some()
        || find_executable("Windsurf").is_some()
        || roots_hint_windsurf_app()
}

fn roots_hint_vscode_app() -> bool {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let Some(home) = home else {
        return false;
    };
    [
        home.join("AppData/Local/Programs/Microsoft VS Code"),
        PathBuf::from("/usr/share/code"),
        home.join("Applications/Visual Studio Code.app"),
        PathBuf::from("/Applications/Visual Studio Code.app"),
    ]
    .iter()
    .any(|p| p.exists())
}

fn roots_hint_windsurf_app() -> bool {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from);
    let Some(home) = home else {
        return false;
    };
    [
        home.join(".codeium/windsurf"),
        home.join("AppData/Roaming/Windsurf"),
        PathBuf::from("/Applications/Windsurf.app"),
    ]
    .iter()
    .any(|p| p.exists())
}

fn antigravity_config_paths(roots: &AgentHealthRoots) -> Vec<PathBuf> {
    vec![
        roots.home.join(".gemini/config/mcp_config.json"),
        roots.home.join(".gemini/antigravity/mcp_config.json"),
        roots
            .os_config_dir("Antigravity")
            .join("User/mcp_config.json"),
        roots.workspace.join(".agents/mcp_config.json"),
    ]
}

fn cursor_config_paths(roots: &AgentHealthRoots) -> Vec<PathBuf> {
    vec![
        roots.home.join(".cursor/mcp.json"),
        roots.workspace.join(".cursor/mcp.json"),
    ]
}

fn claude_desktop_config_path(roots: &AgentHealthRoots) -> PathBuf {
    roots
        .os_config_dir("Claude")
        .join("claude_desktop_config.json")
}

fn claude_code_config_paths(roots: &AgentHealthRoots) -> Vec<PathBuf> {
    vec![
        roots.home.join(".claude.json"),
        roots.home.join(".claude/settings.json"),
        roots.workspace.join(".mcp.json"),
    ]
}

fn grok_config_paths(roots: &AgentHealthRoots) -> Vec<PathBuf> {
    vec![
        roots.os_config_dir("Grok").join("mcp.json"),
        roots.os_config_dir("Grok Bot").join("mcp.json"),
        roots.home.join(".grok/mcp.json"),
    ]
}

fn vscode_config_paths(roots: &AgentHealthRoots) -> Vec<PathBuf> {
    vec![
        roots.workspace.join(".vscode/mcp.json"),
        roots.os_config_dir("Code").join("User/mcp.json"),
    ]
}

fn windsurf_config_paths(roots: &AgentHealthRoots) -> Vec<PathBuf> {
    vec![
        roots.home.join(".codeium/windsurf/mcp_config.json"),
        roots.workspace.join(".windsurf/mcp.json"),
        roots.os_config_dir("Windsurf").join("mcp_config.json"),
    ]
}

/// Report every existing config path for this client (no healthy-mask-stale).
pub fn check_client_entries(
    client: &str,
    installed: bool,
    paths: Vec<PathBuf>,
    allow_jsonc: bool,
    expected: &ExpectedLoungeMcp,
) -> Vec<AgentHealthEntry> {
    let mut entries = Vec::new();
    let mut any_file = false;

    for path in &paths {
        if !path.exists() {
            continue;
        }
        any_file = true;
        match read_lounge_entries(path, allow_jsonc, client == "claude_code") {
            Ok(found) if found.is_empty() => {
                entries.push(entry_base(
                    client,
                    AgentMcpStatus::Missing,
                    Some(path.clone()),
                    None,
                    None,
                    expected,
                    Some("lounge-mcp entry absent".into()),
                ));
            }
            Ok(found) => {
                for (scope, raw) in found {
                    let mut entry = classify_entry(client, path, &raw, expected);
                    if let Some(scope) = scope {
                        let note = format!("project scope: {}", mask_home_path(&scope));
                        entry.detail = Some(match entry.detail {
                            Some(d) => format!("{d}; {note}"),
                            None => note,
                        });
                    }
                    entries.push(entry);
                }
            }
            Err(err) => {
                entries.push(entry_base(
                    client,
                    AgentMcpStatus::Unreadable,
                    Some(path.clone()),
                    None,
                    None,
                    expected,
                    Some(err),
                ));
            }
        }
    }

    if entries.is_empty() {
        if !installed && !any_file {
            entries.push(entry_base(
                client,
                AgentMcpStatus::NotInstalled,
                None,
                None,
                None,
                expected,
                Some("client not detected".into()),
            ));
        } else {
            entries.push(entry_base(
                client,
                AgentMcpStatus::Missing,
                None,
                None,
                None,
                expected,
                Some("lounge-mcp entry absent".into()),
            ));
        }
    }
    entries
}

#[derive(Debug)]
struct RawLoungeEntry {
    key: String,
    command: Option<String>,
    url: Option<String>,
    env_keys: Vec<String>,
}

fn read_lounge_entries(
    path: &Path,
    allow_jsonc: bool,
    scan_claude_projects: bool,
) -> Result<Vec<(Option<String>, RawLoungeEntry)>, String> {
    let raw = fs::read_to_string(path).map_err(|e| format!("read failed: {e}"))?;
    let value = if allow_jsonc {
        parse_jsonc(&raw).map_err(|e| format!("jsonc parse failed: {e}"))?
    } else {
        serde_json::from_str(&raw).map_err(|e| format!("json parse failed: {e}"))?
    };
    Ok(collect_lounge_entries_in_value(
        &value,
        scan_claude_projects,
    ))
}

fn collect_lounge_entries_in_value(
    value: &Value,
    scan_claude_projects: bool,
) -> Vec<(Option<String>, RawLoungeEntry)> {
    let mut out = Vec::new();
    if let Some(entry) = find_lounge_entry_in_map(value) {
        out.push((None, entry));
    }
    if scan_claude_projects {
        if let Some(projects) = value.get("projects").and_then(Value::as_object) {
            for (proj_path, proj) in projects {
                if let Some(entry) = find_lounge_entry_in_map(proj) {
                    out.push((Some(proj_path.clone()), entry));
                }
            }
        }
    }
    out
}

fn find_lounge_entry_in_map(value: &Value) -> Option<RawLoungeEntry> {
    for map_key in ["mcpServers", "servers", "context_servers"] {
        let Some(obj) = value.get(map_key).and_then(Value::as_object) else {
            continue;
        };
        if let Some(spec) = obj.get(LOUNGE_MCP_SERVER_KEY) {
            return Some(raw_from_spec(LOUNGE_MCP_SERVER_KEY, spec));
        }
        for (name, spec) in obj {
            let command = spec.get("command").and_then(Value::as_str).unwrap_or("");
            if command_is_exact_lounge_mcp(command) {
                return Some(raw_from_spec(name, spec));
            }
        }
    }
    None
}

fn raw_from_spec(key: &str, spec: &Value) -> RawLoungeEntry {
    let command = spec
        .get("command")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let url = spec
        .get("url")
        .or_else(|| spec.get("endpoint"))
        .or_else(|| spec.get("serverUrl"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let mut env_keys: Vec<String> = spec
        .get("env")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    env_keys.sort();
    RawLoungeEntry {
        key: key.to_string(),
        command,
        url,
        env_keys,
    }
}

/// Exact basename match: `lounge-mcp`, `lounge-mcp.exe`, `.cmd`, `.bat` — no substring.
pub fn command_is_exact_lounge_mcp(command: &str) -> bool {
    let base = Path::new(command.trim())
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(command.trim())
        .to_ascii_lowercase();
    matches!(
        base.as_str(),
        "lounge-mcp" | "lounge-mcp.exe" | "lounge-mcp.cmd" | "lounge-mcp.bat"
    )
}

/// Resolve absolute/relative path or bare PATH name to a file.
pub fn resolve_lounge_mcp_command(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    let as_path = PathBuf::from(command);
    if as_path.is_file() {
        return Some(as_path);
    }
    // Bare name or PATH-relative: look up via PATH.
    let base = as_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(command);
    find_executable(base).or_else(|| {
        if command_is_exact_lounge_mcp(base) {
            find_executable("lounge-mcp")
        } else {
            None
        }
    })
}

fn classify_entry(
    client: &str,
    path: &Path,
    raw: &RawLoungeEntry,
    expected: &ExpectedLoungeMcp,
) -> AgentHealthEntry {
    let canonical_key = raw.key == LOUNGE_MCP_SERVER_KEY;
    let has_command = raw
        .command
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_some();
    let has_url = raw
        .url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_some();

    if !canonical_key {
        return entry_base(
            client,
            AgentMcpStatus::NonCanonical,
            Some(path.to_path_buf()),
            raw.command.clone(),
            None,
            expected,
            Some(format!(
                "server key {:?} is not {LOUNGE_MCP_SERVER_KEY}",
                raw.key
            )),
        );
    }

    if !has_command && !has_url {
        return entry_base(
            client,
            AgentMcpStatus::NonCanonical,
            Some(path.to_path_buf()),
            None,
            None,
            expected,
            Some("entry missing command and url".into()),
        );
    }

    if has_command {
        let command = raw.command.as_deref().unwrap_or("");
        if !command_is_exact_lounge_mcp(command) {
            return entry_base(
                client,
                AgentMcpStatus::NonCanonical,
                Some(path.to_path_buf()),
                Some(command.into()),
                None,
                expected,
                Some("command is not lounge-mcp".into()),
            );
        }
        let has_url_env = raw.env_keys.iter().any(|k| k == "LOUNGE_MCP_URL");
        if !has_url_env && !has_url {
            return entry_base(
                client,
                AgentMcpStatus::NonCanonical,
                Some(path.to_path_buf()),
                Some(command.into()),
                None,
                expected,
                Some("stdio entry missing LOUNGE_MCP_URL env".into()),
            );
        }

        let Some(cmd_path) = resolve_lounge_mcp_command(command) else {
            return entry_base(
                client,
                AgentMcpStatus::NonCanonical,
                Some(path.to_path_buf()),
                Some(command.into()),
                None,
                expected,
                Some("configured lounge-mcp binary not found on PATH".into()),
            );
        };

        let configured_sha = sha256_file_hex(&cmd_path).ok();
        let configured_version = extract_embedded_version(&cmd_path);
        return compare_versions(
            client,
            path,
            Some(command.into()),
            configured_version,
            configured_sha,
            expected,
        );
    }

    entry_base(
        client,
        AgentMcpStatus::Ok,
        Some(path.to_path_buf()),
        None,
        None,
        expected,
        Some("url-only remote entry".into()),
    )
}

fn compare_versions(
    client: &str,
    path: &Path,
    command: Option<String>,
    configured_version: Option<String>,
    configured_sha: Option<String>,
    expected: &ExpectedLoungeMcp,
) -> AgentHealthEntry {
    let Some(cfg_ver) = configured_version.clone() else {
        return entry_base(
            client,
            AgentMcpStatus::NonCanonical,
            Some(path.to_path_buf()),
            command,
            configured_sha,
            expected,
            Some("could not read embedded lounge-mcp version".into()),
        );
    };

    let expected_ver = &expected.version;
    match semver_cmp(&cfg_ver, expected_ver) {
        SemverCmp::Lower => {
            let mut e = entry_base(
                client,
                AgentMcpStatus::Stale,
                Some(path.to_path_buf()),
                command,
                configured_sha,
                expected,
                Some(format!("configured {cfg_ver} < expected {expected_ver}")),
            );
            e.configured_version = Some(cfg_ver);
            e
        }
        SemverCmp::Equal | SemverCmp::Higher => {
            let sha_mismatch = matches!(
                (&configured_sha, &expected.sha256),
                (Some(a), Some(b))
                    if a != b
                        && matches!(semver_cmp(&cfg_ver, expected_ver), SemverCmp::Equal)
            );
            let status = if sha_mismatch {
                AgentMcpStatus::HashMismatch
            } else {
                AgentMcpStatus::Ok
            };
            let detail = if sha_mismatch {
                Some("same semver, different sha256".into())
            } else {
                None
            };
            let mut e = entry_base(
                client,
                status,
                Some(path.to_path_buf()),
                command,
                configured_sha,
                expected,
                detail,
            );
            e.configured_version = Some(cfg_ver);
            e
        }
        SemverCmp::Invalid => entry_base(
            client,
            AgentMcpStatus::NonCanonical,
            Some(path.to_path_buf()),
            command,
            configured_sha,
            expected,
            Some(format!(
                "invalid semver configured={cfg_ver} expected={expected_ver}"
            )),
        ),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SemverCmp {
    Lower,
    Equal,
    Higher,
    Invalid,
}

fn semver_cmp(configured: &str, expected: &str) -> SemverCmp {
    let Ok(c) = semver::Version::parse(strip_v(configured)) else {
        return SemverCmp::Invalid;
    };
    let Ok(e) = semver::Version::parse(strip_v(expected)) else {
        return SemverCmp::Invalid;
    };
    match c.cmp(&e) {
        std::cmp::Ordering::Less => SemverCmp::Lower,
        std::cmp::Ordering::Equal => SemverCmp::Equal,
        std::cmp::Ordering::Greater => SemverCmp::Higher,
    }
}

fn strip_v(s: &str) -> &str {
    s.trim().strip_prefix('v').unwrap_or(s.trim())
}

fn entry_base(
    client: &str,
    status: AgentMcpStatus,
    config_path: Option<PathBuf>,
    command: Option<String>,
    configured_sha: Option<String>,
    expected: &ExpectedLoungeMcp,
    detail: Option<String>,
) -> AgentHealthEntry {
    AgentHealthEntry {
        client: client.into(),
        status,
        severity: status.severity().into(),
        config_path: config_path.map(|p| mask_home_path(&p.to_string_lossy())),
        command: command.map(|c| mask_home_path(&c)),
        configured_version: None,
        expected_version: Some(expected.version.clone()),
        configured_sha256: configured_sha,
        expected_sha256: expected.sha256.clone(),
        // Mask absolute home prefixes inside free-form error text (not only whole paths).
        detail: detail.map(|d| mask_home_in_text(&d)),
    }
}

/// Replace HOME/USERPROFILE prefixes that appear inside arbitrary detail strings.
fn mask_home_in_text(text: &str) -> String {
    let mut out = text.replace('\\', "/");
    let mut homes: Vec<String> = Vec::new();
    for key in ["HOME", "USERPROFILE"] {
        if let Ok(v) = std::env::var(key) {
            let n = v.replace('\\', "/").trim_end_matches('/').to_string();
            if !n.is_empty() {
                homes.push(n);
            }
        }
    }
    homes.sort_by_key(|h| std::cmp::Reverse(h.len()));
    homes.dedup();
    for home in homes {
        if out.contains(&home) {
            out = out.replace(&home, "~");
        }
    }
    // Also handle a whole-path detail.
    if out.starts_with("~/") || !out.contains('/') {
        out
    } else if out.starts_with('/') || (out.len() > 2 && out.as_bytes()[1] == b':') {
        mask_home_path(&out)
    } else {
        out
    }
}

pub fn sha256_file_hex(path: &Path) -> std::io::Result<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

pub fn extract_embedded_version(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    extract_embedded_version_from_bytes(&bytes)
}

pub fn extract_embedded_version_from_bytes(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    if let Some(idx) = text.find(LOUNGE_MCP_VERSION_MARKER_PREFIX) {
        let rest = &text[idx + LOUNGE_MCP_VERSION_MARKER_PREFIX.len()..];
        let ver: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if !ver.is_empty() {
            return Some(ver);
        }
    }
    const NEEDLE: &str = "Agent Lounge OS MCP ";
    if let Some(idx) = text.find(NEEDLE) {
        let rest = &text[idx + NEEDLE.len()..];
        let ver: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if !ver.is_empty() {
            return Some(ver);
        }
    }
    None
}

/// Strip `//` / `/* */` comments and trailing commas (VS Code / Windsurf / Cursor JSONC).
pub fn strip_jsonc(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    let mut escape = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            i += 1;
            continue;
        }
        if c == '/' && i + 1 < chars.len() {
            if chars[i + 1] == '/' {
                i += 2;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            if chars[i + 1] == '*' {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i = (i + 2).min(chars.len());
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    // Remove trailing commas before } or ]
    let mut cleaned = String::with_capacity(out.len());
    let oc: Vec<char> = out.chars().collect();
    let mut j = 0;
    while j < oc.len() {
        if oc[j] == ',' {
            let mut k = j + 1;
            while k < oc.len() && oc[k].is_whitespace() {
                k += 1;
            }
            if k < oc.len() && (oc[k] == '}' || oc[k] == ']') {
                j += 1;
                continue;
            }
        }
        cleaned.push(oc[j]);
        j += 1;
    }
    cleaned
}

pub fn parse_jsonc(input: &str) -> Result<Value, String> {
    let cleaned = strip_jsonc(input);
    serde_json::from_str(&cleaned).map_err(|e| e.to_string())
}

/// Pure classify helper for unit tests (no filesystem install probes).
#[cfg(any(test, feature = "test-helpers"))]
pub fn check_client_for_test(
    client: &str,
    installed: bool,
    paths: Vec<PathBuf>,
    expected: &ExpectedLoungeMcp,
) -> AgentHealthEntry {
    let allow_jsonc = matches!(client, "cursor" | "vscode" | "windsurf");
    check_client_entries(client, installed, paths, allow_jsonc, expected)
        .into_iter()
        .next()
        .expect("at least one health entry")
}

#[cfg(any(test, feature = "test-helpers"))]
pub fn check_client_entries_for_test(
    client: &str,
    installed: bool,
    paths: Vec<PathBuf>,
    allow_jsonc: bool,
    expected: &ExpectedLoungeMcp,
) -> Vec<AgentHealthEntry> {
    check_client_entries(client, installed, paths, allow_jsonc, expected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn write_json(path: &Path, value: &Value) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    fn fake_binary(path: &Path, version: &str) -> String {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(path).unwrap();
        writeln!(
            f,
            "#!/bin/sh\necho fake\n{LOUNGE_MCP_VERSION_MARKER_PREFIX}{version}\n"
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).unwrap();
        }
        sha256_file_hex(path).unwrap()
    }

    fn temp_roots(label: &str) -> (AgentHealthRoots, PathBuf) {
        let base = std::env::temp_dir().join(format!(
            "lounge-health-t3-{}-{}",
            label,
            uuid::Uuid::new_v4()
        ));
        let home = base.join("home");
        let appdata = base.join("appdata");
        let workspace = base.join("workspace");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&appdata).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        let roots = AgentHealthRoots {
            home,
            appdata: Some(appdata),
            xdg_config: Some(base.join("xdg")),
            workspace,
        };
        (roots, base)
    }

    fn snapshot_files(paths: &[PathBuf]) -> Vec<(PathBuf, Vec<u8>)> {
        paths
            .iter()
            .filter(|p| p.is_file())
            .map(|p| (p.clone(), fs::read(p).unwrap()))
            .collect()
    }

    fn assert_unchanged(before: &[(PathBuf, Vec<u8>)]) {
        for (path, bytes) in before {
            let after = fs::read(path).unwrap();
            assert_eq!(
                &after,
                bytes,
                "config must be byte-for-byte unchanged: {}",
                path.display()
            );
        }
    }

    fn assert_paths_masked(entry: &AgentHealthEntry) {
        for field in [
            entry.config_path.as_deref(),
            entry.command.as_deref(),
            entry.detail.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                !field.contains("/home/")
                    && !field.contains("\\Users\\")
                    && !field.contains("/Users/"),
                "unmasked home path in health field: {field}"
            );
            // Absolute unix roots that look like homes should be ~/…
            if field.starts_with("~/") || field == "~/" || !field.starts_with('/') {
                continue;
            }
            // Windows drive letters without home are ok; reject USERPROFILE-style leaks.
            assert!(
                !field.to_ascii_lowercase().contains("userprofile"),
                "leaked profile in {field}"
            );
        }
    }

    #[test]
    fn t3_fake_configs_per_client_byte_unchanged() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (roots, base) = temp_roots("all");
        // Exact basename lounge-mcp (no substring); separate dirs for ok/stale/hash.
        let bin_ok = base.join("bin/ok/lounge-mcp");
        let bin_stale = base.join("bin/stale/lounge-mcp");
        let sha_ok = fake_binary(&bin_ok, "0.1.0");
        let _sha_stale = fake_binary(&bin_stale, "0.0.9");

        let prev_home = std::env::var_os("HOME");
        let prev_userprofile = std::env::var_os("USERPROFILE");
        let prev_appdata = std::env::var_os("APPDATA");
        unsafe {
            std::env::set_var("HOME", &roots.home);
            std::env::set_var("USERPROFILE", &roots.home);
            if let Some(ref ad) = roots.appdata {
                std::env::set_var("APPDATA", ad);
            }
        }

        let expected = ExpectedLoungeMcp {
            version: "0.1.0".into(),
            sha256: Some(sha_ok.clone()),
            path: Some(bin_ok.clone()),
        };

        let canonical = |cmd: &Path| {
            json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "command": cmd.to_string_lossy(),
                        "args": [],
                        "env": {
                            "LOUNGE_MCP_URL": "http://127.0.0.1:18791",
                            "LOUNGE_DB_PATH": "/tmp/lounge.sqlite",
                            "LOUNGE_NATS_URL": "nats://127.0.0.1:4222"
                        }
                    }
                }
            })
        };

        // Cursor ok (user) — also write project stale for multi-config report
        let cursor_cfg = roots.home.join(".cursor/mcp.json");
        write_json(&cursor_cfg, &canonical(&bin_ok));
        let cursor_proj = roots.workspace.join(".cursor/mcp.json");
        write_json(&cursor_proj, &canonical(&bin_stale));

        let claude_cfg = roots
            .os_config_dir("Claude")
            .join("claude_desktop_config.json");
        write_json(&claude_cfg, &canonical(&bin_stale));

        let anti_cfg = roots.home.join(".gemini/config/mcp_config.json");
        write_json(
            &anti_cfg,
            &json!({ "mcpServers": { "other": { "command": "npx" } } }),
        );

        let code_cfg = roots.workspace.join(".mcp.json");
        write_json(
            &code_cfg,
            &json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "command": "node",
                        "args": ["server.js"],
                        "env": { "LOUNGE_MCP_URL": "http://127.0.0.1:18791" }
                    }
                }
            }),
        );

        // Grok unreadable — path under home; detail must be masked
        let grok_cfg = roots.home.join(".grok/mcp.json");
        fs::create_dir_all(grok_cfg.parent().unwrap()).unwrap();
        fs::write(&grok_cfg, "{ not json").unwrap();

        // VS Code JSONC with comments + trailing comma
        let vscode_cfg = roots.workspace.join(".vscode/mcp.json");
        fs::create_dir_all(vscode_cfg.parent().unwrap()).unwrap();
        fs::write(
            &vscode_cfg,
            format!(
                r#"{{
  // lounge health jsonc fixture
  "servers": {{
    "agent-lounge-os": {{
      "command": "{}",
      "args": [],
      "env": {{ "LOUNGE_MCP_URL": "http://127.0.0.1:18791" }},
    }},
  }},
}}"#,
                bin_ok.display()
            ),
        )
        .unwrap();

        // Exact basename lounge-mcp; different content → HashMismatch at equal semver.
        let bin_hash = base.join("bin/hash/lounge-mcp");
        fake_binary(&bin_hash, "0.1.0");
        {
            let mut f = fs::OpenOptions::new().append(true).open(&bin_hash).unwrap();
            writeln!(f, "# pad").unwrap();
        }
        let wind_cfg = roots.home.join(".codeium/windsurf/mcp_config.json");
        write_json(&wind_cfg, &canonical(&bin_hash));

        // Claude Code ~/.claude.json with per-project mcpServers
        let claude_json = roots.home.join(".claude.json");
        let proj_path = roots.workspace.join("proj-a");
        fs::create_dir_all(&proj_path).unwrap();
        write_json(
            &claude_json,
            &json!({
                "mcpServers": {},
                "projects": {
                    proj_path.to_string_lossy().to_string(): {
                        "mcpServers": {
                            "agent-lounge-os": {
                                "command": bin_ok.to_string_lossy(),
                                "args": [],
                                "env": { "LOUNGE_MCP_URL": "http://127.0.0.1:18791" }
                            }
                        }
                    }
                }
            }),
        );

        let watched = vec![
            cursor_cfg.clone(),
            cursor_proj.clone(),
            claude_cfg.clone(),
            anti_cfg.clone(),
            code_cfg.clone(),
            grok_cfg.clone(),
            vscode_cfg.clone(),
            wind_cfg.clone(),
            claude_json.clone(),
        ];
        let before = snapshot_files(&watched);

        let cursor_entries = check_client_entries_for_test(
            "cursor",
            true,
            vec![cursor_cfg.clone(), cursor_proj.clone()],
            true,
            &expected,
        );
        assert_eq!(
            cursor_entries.len(),
            2,
            "both cursor configs reported: {cursor_entries:?}"
        );
        assert!(
            cursor_entries
                .iter()
                .any(|e| e.status == AgentMcpStatus::Ok),
            "want Ok in {cursor_entries:?}"
        );
        assert!(
            cursor_entries
                .iter()
                .any(|e| e.status == AgentMcpStatus::Stale),
            "want Stale in {cursor_entries:?}"
        );

        let claude =
            check_client_for_test("claude_desktop", true, vec![claude_cfg.clone()], &expected);
        let anti = check_client_for_test("antigravity", true, vec![anti_cfg.clone()], &expected);
        let claude_code =
            check_client_for_test("claude_code", true, vec![code_cfg.clone()], &expected);
        let grok = check_client_for_test("grok_bot", true, vec![grok_cfg.clone()], &expected);
        let vscode = check_client_for_test("vscode", true, vec![vscode_cfg.clone()], &expected);
        let wind = check_client_for_test("windsurf", true, vec![wind_cfg.clone()], &expected);
        let claude_proj_entries = check_client_entries_for_test(
            "claude_code",
            true,
            vec![claude_json.clone()],
            false,
            &expected,
        );

        assert_eq!(claude.status, AgentMcpStatus::Stale);
        assert_eq!(anti.status, AgentMcpStatus::Missing);
        assert_eq!(claude_code.status, AgentMcpStatus::NonCanonical);
        assert_eq!(grok.status, AgentMcpStatus::Unreadable);
        assert_eq!(vscode.status, AgentMcpStatus::Ok, "JSONC must parse");
        assert_eq!(wind.status, AgentMcpStatus::HashMismatch);
        assert!(
            claude_proj_entries
                .iter()
                .any(|e| e.status == AgentMcpStatus::Ok
                    && e.detail
                        .as_deref()
                        .is_some_and(|d| d.contains("project scope"))),
            "claude.json projects.<path>.mcpServers scanned: {claude_proj_entries:?}"
        );

        // Blocker 1: unreadable detail must not leak absolute home path
        let detail = grok.detail.as_deref().unwrap_or("");
        assert!(
            !detail.contains(roots.home.to_string_lossy().as_ref()),
            "detail leaked home: {detail}"
        );
        assert_paths_masked(&grok);
        assert_paths_masked(&vscode);
        for e in &cursor_entries {
            assert_paths_masked(e);
        }

        let _ = check_all_agents(&roots, &expected);
        assert_unchanged(&before);

        let none = check_client_for_test(
            "cursor",
            false,
            vec![roots.home.join(".cursor/missing.json")],
            &expected,
        );
        assert_eq!(none.status, AgentMcpStatus::NotInstalled);

        unsafe {
            match prev_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match prev_userprofile {
                Some(v) => std::env::set_var("USERPROFILE", v),
                None => std::env::remove_var("USERPROFILE"),
            }
            match prev_appdata {
                Some(v) => std::env::set_var("APPDATA", v),
                None => std::env::remove_var("APPDATA"),
            }
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn bare_lounge_mcp_on_path_is_ok_not_non_canonical() {
        let _guard = ENV_LOCK.lock().unwrap();
        let (roots, base) = temp_roots("path");
        let bin_dir = base.join("pathbin");
        fs::create_dir_all(&bin_dir).unwrap();
        let bin = bin_dir.join(if cfg!(windows) {
            "lounge-mcp.exe"
        } else {
            "lounge-mcp"
        });
        let sha = fake_binary(&bin, "0.1.0");
        let expected = ExpectedLoungeMcp {
            version: "0.1.0".into(),
            sha256: Some(sha),
            path: Some(bin.clone()),
        };

        let prev_path = std::env::var_os("PATH");
        let mut new_path = bin_dir.display().to_string();
        if let Some(ref p) = prev_path {
            new_path.push(if cfg!(windows) { ';' } else { ':' });
            new_path.push_str(&p.to_string_lossy());
        }
        unsafe { std::env::set_var("PATH", &new_path) };

        let cfg = roots.home.join(".cursor/mcp.json");
        write_json(
            &cfg,
            &json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "command": "lounge-mcp",
                        "args": [],
                        "env": { "LOUNGE_MCP_URL": "http://127.0.0.1:18791" }
                    }
                }
            }),
        );
        let before = fs::read(&cfg).unwrap();
        let entry = check_client_for_test("cursor", true, vec![cfg.clone()], &expected);
        assert_eq!(
            entry.status,
            AgentMcpStatus::Ok,
            "bare PATH command should resolve: {:?}",
            entry.detail
        );
        assert_eq!(fs::read(&cfg).unwrap(), before);

        // Substring must NOT match
        assert!(!command_is_exact_lounge_mcp("my-lounge-mcp-wrapper"));
        assert!(!command_is_exact_lounge_mcp("lounge-mcp-extra"));
        assert!(command_is_exact_lounge_mcp("lounge-mcp"));
        assert!(command_is_exact_lounge_mcp("/opt/bin/lounge-mcp"));

        unsafe {
            match prev_path {
                Some(p) => std::env::set_var("PATH", p),
                None => std::env::remove_var("PATH"),
            }
        }
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn jsonc_strip_comments_and_trailing_commas() {
        let raw = r#"{
          // comment
          "a": 1,
          "b": [1, 2,],
        }"#;
        let v = parse_jsonc(raw).unwrap();
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"][1], 2);
    }

    #[test]
    fn semver_cmp_orders() {
        assert_eq!(semver_cmp("0.0.9", "0.1.0"), SemverCmp::Lower);
        assert_eq!(semver_cmp("0.1.0", "0.1.0"), SemverCmp::Equal);
        assert_eq!(semver_cmp("0.2.0", "0.1.0"), SemverCmp::Higher);
        assert_eq!(semver_cmp("nope", "0.1.0"), SemverCmp::Invalid);
    }

    #[test]
    fn extract_version_marker() {
        let bytes = b"hello LOUNGE_MCP_VERSION=1.2.3\nrest";
        assert_eq!(
            extract_embedded_version_from_bytes(bytes).as_deref(),
            Some("1.2.3")
        );
    }
}
