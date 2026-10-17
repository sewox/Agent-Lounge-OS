//! H3: read-only lounge-mcp entry health per desktop/CLI client.
//!
//! Never writes MCP configs. Checks only at caller request (startup + on-demand).
//! When several configs exist for one client, every path is reported (no "best" mask).

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::probe::{find_executable, find_executable_in_dirs};
use crate::models::{AgentHealthEntry, AgentMcpStatus};
use crate::services::autodiscover::{
    find_antigravity, find_claude_cli, find_claude_desktop, find_cursor_app, find_grok_bot,
};
use crate::services::health_filters::{mask_home_in_text, mask_home_path_with};

/// Canonical MCP server key for Agent Lounge OS.
pub const LOUNGE_MCP_SERVER_KEY: &str = "agent-lounge-os";
/// Embedded version marker scanned from lounge-mcp binaries (tests + release).
pub const LOUNGE_MCP_VERSION_MARKER_PREFIX: &str = "LOUNGE_MCP_VERSION=";

/// Env / path roots for injectable checks (no process-env mutation required).
#[derive(Debug, Clone)]
pub struct AgentHealthRoots {
    pub home: PathBuf,
    pub appdata: Option<PathBuf>,
    pub xdg_config: Option<PathBuf>,
    pub workspace: PathBuf,
    /// Extra dirs searched before the process PATH for bare `lounge-mcp` commands.
    pub path_dirs: Vec<PathBuf>,
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
            path_dirs: Vec::new(),
        }
    }

    /// Home candidates used for `~/…` masking (injectable; no env read).
    pub fn mask_homes(&self) -> Vec<String> {
        let mut out = vec![self.home.to_string_lossy().into_owned()];
        if let Some(ad) = &self.appdata {
            // Roaming appdata is usually under the profile; include parent profile if distinct.
            if let Some(parent) = ad.parent().and_then(|p| p.parent()) {
                let s = parent.to_string_lossy().into_owned();
                if !s.is_empty() && !out.iter().any(|h| h.eq_ignore_ascii_case(&s)) {
                    out.push(s);
                }
            }
        }
        out
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
        roots,
    ));
    out.extend(check_client_entries(
        "cursor",
        client_installed_cursor(),
        cursor_config_paths(roots),
        true, // JSONC
        expected,
        roots,
    ));
    out.extend(check_client_entries(
        "claude_desktop",
        client_installed_claude_desktop(),
        vec![claude_desktop_config_path(roots)],
        false,
        expected,
        roots,
    ));
    out.extend(check_client_entries(
        "claude_code",
        client_installed_claude_code(),
        claude_code_config_paths(roots),
        false,
        expected,
        roots,
    ));
    out.extend(check_client_entries(
        "grok_bot",
        client_installed_grok(),
        grok_config_paths(roots),
        false,
        expected,
        roots,
    ));
    out.extend(check_client_entries(
        "vscode",
        client_installed_vscode(),
        vscode_config_paths(roots),
        true, // JSONC
        expected,
        roots,
    ));
    out.extend(check_client_entries(
        "windsurf",
        client_installed_windsurf(),
        windsurf_config_paths(roots),
        true, // JSONC
        expected,
        roots,
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
///
/// Masking and bare-command PATH resolution use `roots` only — never mutate process env.
pub fn check_client_entries(
    client: &str,
    installed: bool,
    paths: Vec<PathBuf>,
    allow_jsonc: bool,
    expected: &ExpectedLoungeMcp,
    roots: &AgentHealthRoots,
) -> Vec<AgentHealthEntry> {
    let homes = roots.mask_homes();
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
                    &homes,
                ));
            }
            Ok(found) => {
                for (scope, raw) in found {
                    let mut entry = classify_entry(client, path, &raw, expected, roots, &homes);
                    if let Some(scope) = scope {
                        let note =
                            format!("project scope: {}", mask_home_path_with(&scope, &homes));
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
                    &homes,
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
                &homes,
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
                &homes,
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
/// `extra_path_dirs` are searched before the process PATH (injectable for tests).
pub fn resolve_lounge_mcp_command(command: &str, extra_path_dirs: &[PathBuf]) -> Option<PathBuf> {
    let command = command.trim();
    if command.is_empty() {
        return None;
    }
    let as_path = PathBuf::from(command);
    if as_path.is_file() {
        return Some(as_path);
    }
    // Bare name or PATH-relative: look up via injected dirs, then process PATH.
    let base = as_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(command);
    find_executable_in_dirs(base, extra_path_dirs)
        .or_else(|| find_executable(base))
        .or_else(|| {
            if command_is_exact_lounge_mcp(base) {
                find_executable_in_dirs("lounge-mcp", extra_path_dirs)
                    .or_else(|| find_executable("lounge-mcp"))
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
    roots: &AgentHealthRoots,
    homes: &[String],
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
            homes,
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
            homes,
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
                homes,
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
                homes,
            );
        }

        let Some(cmd_path) = resolve_lounge_mcp_command(command, &roots.path_dirs) else {
            return entry_base(
                client,
                AgentMcpStatus::NonCanonical,
                Some(path.to_path_buf()),
                Some(command.into()),
                None,
                expected,
                Some("configured lounge-mcp binary not found on PATH".into()),
                homes,
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
            homes,
        );
    }

    entry_base(
        client,
        AgentMcpStatus::Unverified,
        Some(path.to_path_buf()),
        None,
        None,
        expected,
        Some("url-only entry, not probed".into()),
        homes,
    )
}

fn compare_versions(
    client: &str,
    path: &Path,
    command: Option<String>,
    configured_version: Option<String>,
    configured_sha: Option<String>,
    expected: &ExpectedLoungeMcp,
    homes: &[String],
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
            homes,
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
                homes,
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
                homes,
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
            homes,
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

#[allow(clippy::too_many_arguments)]
fn entry_base(
    client: &str,
    status: AgentMcpStatus,
    config_path: Option<PathBuf>,
    command: Option<String>,
    configured_sha: Option<String>,
    expected: &ExpectedLoungeMcp,
    detail: Option<String>,
    homes: &[String],
) -> AgentHealthEntry {
    AgentHealthEntry {
        client: client.into(),
        status,
        severity: status.severity().into(),
        config_path: config_path.map(|p| mask_home_path_with(&p.to_string_lossy(), homes)),
        command: command.map(|c| mask_home_path_with(&c, homes)),
        configured_version: None,
        expected_version: Some(expected.version.clone()),
        configured_sha256: configured_sha,
        expected_sha256: expected.sha256.clone(),
        detail: detail.map(|d| mask_home_in_text(&d, homes)),
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

/// Config paths + JSONC flag for a known agent client id.
fn config_paths_for_client(client: &str, roots: &AgentHealthRoots) -> Option<(Vec<PathBuf>, bool)> {
    match client {
        "antigravity" => Some((antigravity_config_paths(roots), false)),
        "cursor" => Some((cursor_config_paths(roots), true)),
        "claude_desktop" => Some((vec![claude_desktop_config_path(roots)], false)),
        "claude_code" => Some((claude_code_config_paths(roots), false)),
        "grok_bot" => Some((grok_config_paths(roots), false)),
        "vscode" => Some((vscode_config_paths(roots), true)),
        "windsurf" => Some((windsurf_config_paths(roots), true)),
        _ => None,
    }
}

/// Re-read lounge-mcp URL from disk for `client` + masked `config_path_id`.
///
/// Used by the on-demand probe — never accepts a URL from the UI. Returns
/// `Ok(None)` when the config/path/entry is missing or has no URL field.
pub fn lookup_agent_mcp_url(
    client: &str,
    config_path_id: &str,
    roots: &AgentHealthRoots,
) -> Result<Option<String>, String> {
    let Some((paths, allow_jsonc)) = config_paths_for_client(client, roots) else {
        return Ok(None);
    };
    let homes = roots.mask_homes();
    let want = config_path_id.trim().replace('\\', "/");
    if want.is_empty() {
        return Ok(None);
    }
    let scan_claude = client == "claude_code";
    for path in paths {
        if !path.exists() {
            continue;
        }
        let masked = mask_home_path_with(&path.to_string_lossy(), &homes).replace('\\', "/");
        if masked != want {
            continue;
        }
        let entries = read_lounge_entries(&path, allow_jsonc, scan_claude)?;
        for (_scope, raw) in entries {
            let has_command = raw
                .command
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .is_some();
            let url = raw.url.as_deref().map(str::trim).filter(|s| !s.is_empty());
            // Probe is URL-only — refuse entries that also declare a command.
            if let (Some(url), false) = (url, has_command) {
                return Ok(Some(url.to_string()));
            }
        }
        return Ok(None);
    }
    Ok(None)
}

/// Pure classify helper for unit tests (no filesystem install probes).
#[cfg(any(test, feature = "test-helpers"))]
pub fn check_client_for_test(
    client: &str,
    installed: bool,
    paths: Vec<PathBuf>,
    expected: &ExpectedLoungeMcp,
    roots: &AgentHealthRoots,
) -> AgentHealthEntry {
    let allow_jsonc = matches!(client, "cursor" | "vscode" | "windsurf");
    check_client_entries(client, installed, paths, allow_jsonc, expected, roots)
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
    roots: &AgentHealthRoots,
) -> Vec<AgentHealthEntry> {
    check_client_entries(client, installed, paths, allow_jsonc, expected, roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn write_json(path: &Path, value: &Value) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    }

    /// Slash-normalized absolute path safe to embed in JSON/JSONC string literals
    /// (Windows backslashes would be interpreted as escapes).
    fn json_path(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
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
            path_dirs: Vec::new(),
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

    fn assert_paths_masked(entry: &AgentHealthEntry, home: &Path) {
        let home_s = home.to_string_lossy();
        let home_slash = home_s.replace('\\', "/");
        for field in [
            entry.config_path.as_deref(),
            entry.command.as_deref(),
            entry.detail.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            assert!(
                !field.contains(home_s.as_ref()) && !field.contains(home_slash.as_str()),
                "unmasked injectable home in health field: {field}"
            );
            // Config paths under the injectable home must be ~/…
            if let Some(cp) = entry.config_path.as_deref() {
                if field == cp {
                    assert!(
                        cp.starts_with("~/") || !cp.contains('/'),
                        "config_path should be masked: {cp}"
                    );
                }
            }
        }
    }

    #[test]
    fn t3_fake_configs_per_client_byte_unchanged() {
        // No process-env mutation: roots.home / path_dirs are injected.
        let (roots, base) = temp_roots("all");
        // Exact basename lounge-mcp (no substring); separate dirs for ok/stale/hash.
        let bin_ok = base.join("bin/ok/lounge-mcp");
        let bin_stale = base.join("bin/stale/lounge-mcp");
        let sha_ok = fake_binary(&bin_ok, "0.1.0");
        let _sha_stale = fake_binary(&bin_stale, "0.0.9");

        let expected = ExpectedLoungeMcp {
            version: "0.1.0".into(),
            sha256: Some(sha_ok.clone()),
            path: Some(bin_ok.clone()),
        };

        let canonical = |cmd: &Path| {
            json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "command": json_path(cmd),
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

        // Grok unreadable — path under home; detail must be masked via injected homes
        let grok_cfg = roots.home.join(".grok/mcp.json");
        fs::create_dir_all(grok_cfg.parent().unwrap()).unwrap();
        fs::write(&grok_cfg, "{ not json").unwrap();

        // VS Code JSONC with comments + trailing comma.
        // Use forward-slash path (and JSON-escaped via serde for the command string)
        // so Windows backslashes cannot break JSONC parsing.
        let vscode_cfg = roots.workspace.join(".vscode/mcp.json");
        fs::create_dir_all(vscode_cfg.parent().unwrap()).unwrap();
        let cmd_json = serde_json::to_string(&json_path(&bin_ok)).unwrap();
        fs::write(
            &vscode_cfg,
            format!(
                r#"{{
  // lounge health jsonc fixture
  "servers": {{
    "agent-lounge-os": {{
      "command": {cmd_json},
      "args": [],
      "env": {{ "LOUNGE_MCP_URL": "http://127.0.0.1:18791" }},
    }},
  }},
}}"#
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
                    json_path(&proj_path): {
                        "mcpServers": {
                            "agent-lounge-os": {
                                "command": json_path(&bin_ok),
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
            &roots,
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

        let claude = check_client_for_test(
            "claude_desktop",
            true,
            vec![claude_cfg.clone()],
            &expected,
            &roots,
        );
        let anti = check_client_for_test(
            "antigravity",
            true,
            vec![anti_cfg.clone()],
            &expected,
            &roots,
        );
        let claude_code = check_client_for_test(
            "claude_code",
            true,
            vec![code_cfg.clone()],
            &expected,
            &roots,
        );
        let grok =
            check_client_for_test("grok_bot", true, vec![grok_cfg.clone()], &expected, &roots);
        let vscode =
            check_client_for_test("vscode", true, vec![vscode_cfg.clone()], &expected, &roots);
        let wind =
            check_client_for_test("windsurf", true, vec![wind_cfg.clone()], &expected, &roots);
        let claude_proj_entries = check_client_entries_for_test(
            "claude_code",
            true,
            vec![claude_json.clone()],
            false,
            &expected,
            &roots,
        );

        assert_eq!(claude.status, AgentMcpStatus::Stale);
        assert_eq!(anti.status, AgentMcpStatus::Missing);
        assert_eq!(claude_code.status, AgentMcpStatus::NonCanonical);
        assert_eq!(grok.status, AgentMcpStatus::Unreadable);
        assert_eq!(
            vscode.status,
            AgentMcpStatus::Ok,
            "JSONC must parse (Windows-safe path escaping): {:?}",
            vscode.detail
        );
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

        // Unreadable detail must not leak absolute home path (injected mask homes).
        let detail = grok.detail.as_deref().unwrap_or("");
        assert!(
            !detail.contains(roots.home.to_string_lossy().as_ref()),
            "detail leaked home: {detail}"
        );
        assert_paths_masked(&grok, &roots.home);
        assert_paths_masked(&vscode, &roots.home);
        for e in &cursor_entries {
            assert_paths_masked(e, &roots.home);
        }

        let _ = check_all_agents(&roots, &expected);
        assert_unchanged(&before);

        let none = check_client_for_test(
            "cursor",
            false,
            vec![roots.home.join(".cursor/missing.json")],
            &expected,
            &roots,
        );
        assert_eq!(none.status, AgentMcpStatus::NotInstalled);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn url_only_entries_are_unverified_not_ok() {
        let (roots, base) = temp_roots("url-only");
        let expected = ExpectedLoungeMcp {
            version: "0.1.0".into(),
            sha256: None,
            path: None,
        };

        let url_cfg = roots.home.join(".cursor/mcp.json");
        write_json(
            &url_cfg,
            &json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "url": "http://127.0.0.1:18791/mcp"
                    }
                }
            }),
        );
        let url_entry =
            check_client_for_test("cursor", true, vec![url_cfg.clone()], &expected, &roots);
        assert_eq!(url_entry.status, AgentMcpStatus::Unverified);
        assert_eq!(url_entry.status.as_str(), "unverified");
        assert_eq!(url_entry.severity, "info");
        assert_eq!(
            url_entry.detail.as_deref(),
            Some("url-only entry, not probed")
        );
        assert!(url_entry.command.is_none());

        let server_url_cfg = roots.home.join(".claude/mcp.json");
        write_json(
            &server_url_cfg,
            &json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "serverUrl": "https://example.invalid/mcp"
                    }
                }
            }),
        );
        let server_url_entry = check_client_for_test(
            "claude_desktop",
            true,
            vec![server_url_cfg.clone()],
            &expected,
            &roots,
        );
        assert_eq!(server_url_entry.status, AgentMcpStatus::Unverified);
        assert_eq!(server_url_entry.severity, "info");
        assert_eq!(
            server_url_entry.detail.as_deref(),
            Some("url-only entry, not probed")
        );

        let before = snapshot_files(&[url_cfg.clone(), server_url_cfg.clone()]);
        let _ = check_all_agents(&roots, &expected);
        assert_unchanged(&before);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn command_plus_url_keeps_local_binary_check() {
        let (roots, base) = temp_roots("cmd-url");
        let bin_ok = base.join("bin/ok/lounge-mcp");
        let bin_stale = base.join("bin/stale/lounge-mcp");
        let sha_ok = fake_binary(&bin_ok, "0.1.0");
        let _sha_stale = fake_binary(&bin_stale, "0.0.9");
        let expected = ExpectedLoungeMcp {
            version: "0.1.0".into(),
            sha256: Some(sha_ok.clone()),
            path: Some(bin_ok.clone()),
        };

        let with_both = |cmd: &Path| {
            json!({
                "mcpServers": {
                    "agent-lounge-os": {
                        "command": json_path(cmd),
                        "url": "http://127.0.0.1:18791/mcp",
                        "args": [],
                        "env": { "LOUNGE_MCP_URL": "http://127.0.0.1:18791" }
                    }
                }
            })
        };

        let ok_cfg = roots.home.join(".cursor/mcp.json");
        write_json(&ok_cfg, &with_both(&bin_ok));
        let ok_entry =
            check_client_for_test("cursor", true, vec![ok_cfg.clone()], &expected, &roots);
        assert_eq!(
            ok_entry.status,
            AgentMcpStatus::Ok,
            "command+url must use local binary check: {:?}",
            ok_entry.detail
        );

        let stale_cfg = roots.home.join(".claude/mcp.json");
        write_json(&stale_cfg, &with_both(&bin_stale));
        let stale_entry = check_client_for_test(
            "claude_desktop",
            true,
            vec![stale_cfg.clone()],
            &expected,
            &roots,
        );
        assert_eq!(stale_entry.status, AgentMcpStatus::Stale);

        let bin_hash = base.join("bin/hash/lounge-mcp");
        fake_binary(&bin_hash, "0.1.0");
        {
            let mut f = fs::OpenOptions::new().append(true).open(&bin_hash).unwrap();
            writeln!(f, "# pad").unwrap();
        }
        let hash_cfg = roots.home.join(".codeium/windsurf/mcp_config.json");
        write_json(&hash_cfg, &with_both(&bin_hash));
        let hash_entry =
            check_client_for_test("windsurf", true, vec![hash_cfg.clone()], &expected, &roots);
        assert_eq!(hash_entry.status, AgentMcpStatus::HashMismatch);

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn bare_lounge_mcp_on_path_is_ok_not_non_canonical() {
        // Inject path_dirs instead of mutating process PATH.
        let (mut roots, base) = temp_roots("path");
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
        roots.path_dirs = vec![bin_dir];

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
        let entry = check_client_for_test("cursor", true, vec![cfg.clone()], &expected, &roots);
        assert_eq!(
            entry.status,
            AgentMcpStatus::Ok,
            "bare PATH command should resolve via roots.path_dirs: {:?}",
            entry.detail
        );
        assert_eq!(fs::read(&cfg).unwrap(), before);

        // Substring must NOT match
        assert!(!command_is_exact_lounge_mcp("my-lounge-mcp-wrapper"));
        assert!(!command_is_exact_lounge_mcp("lounge-mcp-extra"));
        assert!(command_is_exact_lounge_mcp("lounge-mcp"));
        assert!(command_is_exact_lounge_mcp("/opt/bin/lounge-mcp"));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn jsonc_accepts_windows_style_escaped_command_path() {
        // Regression: raw `C:\Users\...` inside a JSONC string is Unreadable on Windows.
        let raw = r#"{
  // comment
  "servers": {
    "agent-lounge-os": {
      "command": "C:/Temp/bin/ok/lounge-mcp",
      "args": [],
      "env": { "LOUNGE_MCP_URL": "http://127.0.0.1:18791" },
    },
  },
}"#;
        let v = parse_jsonc(raw).expect("forward-slash path JSONC");
        assert_eq!(
            v["servers"]["agent-lounge-os"]["command"],
            "C:/Temp/bin/ok/lounge-mcp"
        );

        // Unescaped backslashes must fail (documents why T3 uses json_path).
        let bad =
            r#"{ "servers": { "agent-lounge-os": { "command": "C:\Temp\bin\lounge-mcp" } } }"#;
        assert!(
            parse_jsonc(bad).is_err(),
            "raw Windows backslashes in JSON string must not silently parse"
        );
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
