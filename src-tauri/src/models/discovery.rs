use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveryReport {
    pub scanned_at: String,
    pub sources: Vec<DiscoverySource>,
    pub tools: Vec<DiscoveredTool>,
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
}

impl DiscoveredTool {
    pub fn new(source: &str, name: &str, kind: &str) -> Self {
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
        }
    }
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
        "mcp" => "mcp",
        "model" => "model",
        _ => "cli",
    }
}
