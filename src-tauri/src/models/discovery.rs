use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveryReport {
    pub scanned_at: String,
    pub sources: Vec<DiscoverySource>,
    pub tools: Vec<DiscoveredTool>,
    #[serde(default)]
    pub apps: Vec<DiscoveredTool>,
    #[serde(default)]
    pub models: Vec<DiscoveredTool>,
    #[serde(default)]
    pub mcp_servers: Vec<DiscoveredTool>,
    #[serde(default)]
    pub system_tools: Vec<SystemTool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SystemTool {
    pub id: String,
    pub name: String,
    pub available: bool,
    pub path: Option<String>,
    pub detail: Option<String>,
}

impl SystemTool {
    pub fn to_discovered(&self) -> DiscoveredTool {
        let mut tool = DiscoveredTool::new("system", &self.name, "system");
        tool.origin_path = self.path.clone();
        tool.command = self.path.clone().or_else(|| Some(self.name.clone()));
        tool.detail = self.detail.clone();
        tool.available = self.available;
        tool
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoverySource {
    pub id: String,
    pub available: bool,
    pub origin_path: Option<String>,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveredTool {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub source: String,
    pub origin_path: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub endpoint: Option<String>,
    pub detail: Option<String>,
    pub available: bool,
    #[serde(default = "default_access_mode")]
    pub access_mode: String,
    #[serde(default)]
    pub host_id: Option<String>,
    #[serde(default)]
    pub host_ids: Vec<String>,
}

fn default_access_mode() -> String {
    "local".into()
}

impl DiscoveredTool {
    pub fn new(source: &str, name: &str, kind: &str) -> Self {
        let access_mode = access_mode_for(kind).to_string();
        let host_id = host_id_for(kind, source);
        let host_ids = host_id.iter().cloned().collect();
        Self {
            id: tool_id(source, name),
            name: name.to_string(),
            kind: kind.to_string(),
            source: source.to_string(),
            origin_path: None,
            command: None,
            args: Vec::new(),
            endpoint: None,
            detail: None,
            available: true,
            access_mode,
            host_id,
            host_ids,
        }
    }

    pub fn host_app(source: &str, name: &str) -> Self {
        Self {
            id: tool_id("app", source),
            name: name.to_string(),
            kind: "app".into(),
            source: source.to_string(),
            origin_path: None,
            command: None,
            args: Vec::new(),
            endpoint: None,
            detail: None,
            available: true,
            access_mode: "subscription".into(),
            host_id: Some(source.to_string()),
            host_ids: vec![source.to_string()],
        }
    }

    pub fn hosts(&self) -> Vec<String> {
        let mut hosts = Vec::new();
        for host in self.host_ids.iter().chain(self.host_id.iter()) {
            if !hosts.iter().any(|existing| existing == host) {
                hosts.push(host.clone());
            }
        }
        if hosts.is_empty() && matches!(self.kind.as_str(), "plugin" | "mcp" | "app") {
            hosts.push(self.source.clone());
        }
        hosts
    }
}

pub fn access_mode_for(kind: &str) -> &'static str {
    match kind {
        "app" => "subscription",
        "mcp" | "plugin" => "plugin",
        "model" | "system" | "cli" => "local",
        _ => "local",
    }
}

fn host_id_for(kind: &str, source: &str) -> Option<String> {
    match kind {
        "app" | "mcp" | "plugin" => Some(source.to_string()),
        _ => None,
    }
}

pub fn host_display_name(id: &str) -> String {
    match id {
        "claude_desktop" => "Claude Desktop".into(),
        "claude_cli" => "Claude CLI".into(),
        "cursor" => "Cursor".into(),
        "grok_bot" => "Grok Bot".into(),
        "antigravity" => "Antigravity".into(),
        other => other.to_string(),
    }
}

pub fn format_host_labels(ids: &[String]) -> String {
    ids.iter()
        .map(|id| host_display_name(id))
        .collect::<Vec<_>>()
        .join(" · ")
}

pub fn tool_id(source: &str, name: &str) -> String {
    format!("{source}:{name}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConnectedTool {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub source: String,
    pub origin_path: Option<String>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub endpoint: Option<String>,
    pub enabled: bool,
    pub connected_at: String,
    pub payload: serde_json::Value,
    #[serde(rename = "type")]
    pub tool_type: String,
    pub config_path: Option<String>,
    pub is_active: bool,
    pub last_synced: String,
}

impl ConnectedTool {
    pub fn from_discovered(tool: &DiscoveredTool, connected_at: String) -> Self {
        Self {
            id: tool.id.clone(),
            name: tool.name.clone(),
            kind: tool.kind.clone(),
            source: tool.source.clone(),
            origin_path: tool.origin_path.clone(),
            command: tool.command.clone(),
            args: tool.args.clone(),
            endpoint: tool.endpoint.clone(),
            enabled: true,
            connected_at: connected_at.clone(),
            payload: serde_json::json!({
                "detail": tool.detail,
                "available": tool.available,
                "access_mode": tool.access_mode,
                "host_id": tool.host_id,
                "host_ids": tool.hosts(),
            }),
            tool_type: sqlite_tool_type(&tool.kind).to_string(),
            config_path: tool.origin_path.clone(),
            is_active: true,
            last_synced: connected_at,
        }
    }
}

pub fn sqlite_tool_type(kind: &str) -> &'static str {
    match kind {
        "mcp" | "plugin" => "mcp",
        "model" => "model",
        "app" => "app",
        "worker" => "worker",
        _ => "cli",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_app_id_and_subscription_mode() {
        let tool = DiscoveredTool::host_app("claude_desktop", "Claude Desktop");
        assert_eq!(tool.id, "app:claude_desktop");
        assert_eq!(tool.kind, "app");
        assert_eq!(tool.access_mode, "subscription");
        assert_eq!(tool.host_id.as_deref(), Some("claude_desktop"));
        assert_eq!(sqlite_tool_type("app"), "app");
        assert_eq!(sqlite_tool_type("plugin"), "mcp");
    }
}
