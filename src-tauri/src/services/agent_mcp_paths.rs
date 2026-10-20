//! Shared MCP config path table for Health (read) and Connect (write).
//!
//! Single source of truth for client × OS paths. Roots are injectable — never
//! mutate process environment for tests (Health-1b / #109 lesson).

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Canonical MCP server key for Agent Lounge OS.
pub const LOUNGE_MCP_SERVER_KEY: &str = "agent-lounge-os";

/// Supported first-class clients (parity with `agent_health` / Health-UI-2).
pub const SUPPORTED_CLIENTS: &[&str] = &[
    "antigravity",
    "cursor",
    "claude_desktop",
    "claude_code",
    "grok_bot",
    "vscode",
    "windsurf",
];

/// Connect / disconnect scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectScope {
    /// User-level config (default).
    Global,
    /// Workspace / project config.
    Project,
}

impl ConnectScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "global" | "g" => Some(Self::Global),
            "project" | "p" => Some(Self::Project),
            _ => None,
        }
    }
}

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

    pub fn os_config_dir(&self, app: &str) -> PathBuf {
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

/// One config path target with metadata for connect/health.
#[derive(Debug, Clone)]
pub struct ClientConfigTarget {
    pub path: PathBuf,
    pub scope: ConnectScope,
    pub allow_jsonc: bool,
    /// Top-level map key that holds MCP servers (`mcpServers` or `servers`).
    pub servers_key: &'static str,
}

pub fn client_allows_jsonc(client: &str) -> bool {
    matches!(client, "cursor" | "vscode" | "windsurf")
}

pub fn servers_key_for_client(client: &str) -> &'static str {
    match client {
        "vscode" => "servers",
        _ => "mcpServers",
    }
}

/// All config paths for a client (global + project), same order as Health-1b.
pub fn all_config_targets(
    client: &str,
    roots: &AgentHealthRoots,
) -> Option<Vec<ClientConfigTarget>> {
    let allow_jsonc = client_allows_jsonc(client);
    let servers_key = servers_key_for_client(client);
    let paths = match client {
        "antigravity" => antigravity_config_paths(roots),
        "cursor" => cursor_config_paths(roots),
        "claude_desktop" => vec![claude_desktop_config_path(roots)],
        "claude_code" => claude_code_config_paths(roots),
        "grok_bot" => grok_config_paths(roots),
        "vscode" => vscode_config_paths(roots),
        "windsurf" => windsurf_config_paths(roots),
        _ => return None,
    };
    Some(
        paths
            .into_iter()
            .map(|path| {
                let scope = classify_path_scope(client, &path, roots);
                ClientConfigTarget {
                    path,
                    scope,
                    allow_jsonc,
                    servers_key,
                }
            })
            .collect(),
    )
}

/// Config targets filtered by connect scope.
pub fn config_targets_for_scope(
    client: &str,
    roots: &AgentHealthRoots,
    scope: ConnectScope,
) -> Option<Vec<ClientConfigTarget>> {
    let all = all_config_targets(client, roots)?;
    let filtered: Vec<_> = all.into_iter().filter(|t| t.scope == scope).collect();
    if filtered.is_empty() {
        // Some clients have no project path (e.g. Claude Desktop, Grok).
        return None;
    }
    Some(filtered)
}

/// Primary write target for a scope: prefer the first existing file; else first path.
pub fn primary_write_target(
    client: &str,
    roots: &AgentHealthRoots,
    scope: ConnectScope,
) -> Result<ClientConfigTarget, String> {
    let targets = config_targets_for_scope(client, roots, scope).ok_or_else(|| {
        format!(
            "client {client:?} has no {} config path on this OS/workspace",
            scope.as_str()
        )
    })?;
    if let Some(existing) = targets.iter().find(|t| t.path.is_file()) {
        return Ok(existing.clone());
    }
    targets
        .into_iter()
        .next()
        .ok_or_else(|| format!("client {client:?}: empty path table for {}", scope.as_str()))
}

fn classify_path_scope(client: &str, path: &Path, roots: &AgentHealthRoots) -> ConnectScope {
    let ws = roots.workspace.as_path();
    if path.starts_with(ws) {
        return ConnectScope::Project;
    }
    match client {
        "cursor" => {
            if path == roots.workspace.join(".cursor/mcp.json") {
                ConnectScope::Project
            } else {
                ConnectScope::Global
            }
        }
        "antigravity" => {
            if path == roots.workspace.join(".agents/mcp_config.json") {
                ConnectScope::Project
            } else {
                ConnectScope::Global
            }
        }
        "claude_code" => {
            if path == roots.workspace.join(".mcp.json") {
                ConnectScope::Project
            } else {
                ConnectScope::Global
            }
        }
        "vscode" => {
            if path == roots.workspace.join(".vscode/mcp.json") {
                ConnectScope::Project
            } else {
                ConnectScope::Global
            }
        }
        "windsurf" => {
            if path == roots.workspace.join(".windsurf/mcp.json") {
                ConnectScope::Project
            } else {
                ConnectScope::Global
            }
        }
        _ => ConnectScope::Global,
    }
}

pub fn antigravity_config_paths(roots: &AgentHealthRoots) -> Vec<PathBuf> {
    vec![
        roots.home.join(".gemini/config/mcp_config.json"),
        roots.home.join(".gemini/antigravity/mcp_config.json"),
        roots
            .os_config_dir("Antigravity")
            .join("User/mcp_config.json"),
        roots.workspace.join(".agents/mcp_config.json"),
    ]
}

pub fn cursor_config_paths(roots: &AgentHealthRoots) -> Vec<PathBuf> {
    vec![
        roots.home.join(".cursor/mcp.json"),
        roots.workspace.join(".cursor/mcp.json"),
    ]
}

pub fn claude_desktop_config_path(roots: &AgentHealthRoots) -> PathBuf {
    roots
        .os_config_dir("Claude")
        .join("claude_desktop_config.json")
}

pub fn claude_code_config_paths(roots: &AgentHealthRoots) -> Vec<PathBuf> {
    vec![
        roots.home.join(".claude.json"),
        roots.home.join(".claude/settings.json"),
        roots.workspace.join(".mcp.json"),
    ]
}

pub fn grok_config_paths(roots: &AgentHealthRoots) -> Vec<PathBuf> {
    vec![
        roots.os_config_dir("Grok").join("mcp.json"),
        roots.os_config_dir("Grok Bot").join("mcp.json"),
        roots.home.join(".grok/mcp.json"),
    ]
}

pub fn vscode_config_paths(roots: &AgentHealthRoots) -> Vec<PathBuf> {
    vec![
        roots.workspace.join(".vscode/mcp.json"),
        roots.os_config_dir("Code").join("User/mcp.json"),
    ]
}

pub fn windsurf_config_paths(roots: &AgentHealthRoots) -> Vec<PathBuf> {
    vec![
        roots.home.join(".codeium/windsurf/mcp_config.json"),
        roots.workspace.join(".windsurf/mcp.json"),
        roots.os_config_dir("Windsurf").join("mcp_config.json"),
    ]
}

/// Config paths + JSONC flag for a known agent client id (Health lookup helper).
pub fn config_paths_for_client(
    client: &str,
    roots: &AgentHealthRoots,
) -> Option<(Vec<PathBuf>, bool)> {
    let targets = all_config_targets(client, roots)?;
    let allow_jsonc = targets.first().map(|t| t.allow_jsonc).unwrap_or(false);
    let paths = targets.into_iter().map(|t| t.path).collect();
    Some((paths, allow_jsonc))
}

/// Whether this client gets a rule / skill file written by Connect-1.
pub fn client_supports_rule_file(client: &str) -> bool {
    matches!(client, "cursor" | "claude_code" | "antigravity")
}

/// Rule / skill file targets for connect (may be empty).
pub fn rule_file_targets(
    client: &str,
    roots: &AgentHealthRoots,
    scope: ConnectScope,
) -> Vec<PathBuf> {
    match client {
        "cursor" => match scope {
            ConnectScope::Project => {
                vec![roots.workspace.join(".cursor/rules/agent-lounge.mdc")]
            }
            ConnectScope::Global => {
                vec![roots.home.join(".cursor/rules/agent-lounge.mdc")]
            }
        },
        "claude_code" => match scope {
            // Separate skill file — never edit CLAUDE.md (CC-3).
            ConnectScope::Project => {
                vec![roots.workspace.join(".claude/skills/agent-lounge/SKILL.md")]
            }
            ConnectScope::Global => {
                vec![roots.home.join(".claude/skills/agent-lounge/SKILL.md")]
            }
        },
        "antigravity" => match scope {
            ConnectScope::Project => {
                vec![roots.workspace.join(".agents/rules/agent-lounge.md")]
            }
            ConnectScope::Global => vec![
                roots.home.join(".gemini/antigravity/rules/agent-lounge.md"),
                roots.home.join(".gemini/rules/agent-lounge.md"),
            ],
        },
        _ => Vec::new(),
    }
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

/// True when rewriting via serde_json would drop comments / trailing commas.
pub fn jsonc_write_would_lose_comments(raw: &str) -> bool {
    if serde_json::from_str::<Value>(raw).is_ok() {
        return false;
    }
    parse_jsonc(raw).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_clients_match_health_parity() {
        assert_eq!(SUPPORTED_CLIENTS.len(), 7);
        assert!(SUPPORTED_CLIENTS.contains(&"vscode"));
        assert!(SUPPORTED_CLIENTS.contains(&"windsurf"));
    }

    #[test]
    fn scope_filters_cursor_global_vs_project() {
        let roots = AgentHealthRoots {
            home: PathBuf::from("/tmp/home"),
            appdata: None,
            xdg_config: Some(PathBuf::from("/tmp/xdg")),
            workspace: PathBuf::from("/tmp/ws"),
            path_dirs: Vec::new(),
        };
        let global = config_targets_for_scope("cursor", &roots, ConnectScope::Global).unwrap();
        assert_eq!(global.len(), 1);
        assert!(global[0].path.ends_with(".cursor/mcp.json"));
        assert!(global[0].path.starts_with(&roots.home));

        let project = config_targets_for_scope("cursor", &roots, ConnectScope::Project).unwrap();
        assert_eq!(project.len(), 1);
        assert!(project[0].path.starts_with(&roots.workspace));
    }

    #[test]
    fn jsonc_detects_comment_loss() {
        let pure = r#"{ "mcpServers": {} }"#;
        assert!(!jsonc_write_would_lose_comments(pure));
        let with_comment = "{\n  // hi\n  \"mcpServers\": {}\n}";
        assert!(jsonc_write_would_lose_comments(with_comment));
    }
}
