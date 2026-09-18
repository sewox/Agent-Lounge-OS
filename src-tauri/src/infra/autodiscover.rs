use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::infra::quotas::http_json;
use crate::models::{now_rfc3339, DiscoveredTool, DiscoveryReport, DiscoverySource};

const SOURCE_CLAUDE: &str = "claude_desktop";
const SOURCE_CURSOR: &str = "cursor";
const SOURCE_OLLAMA: &str = "ollama";

pub async fn scan_system(workspace: PathBuf, ollama_endpoint: String) -> DiscoveryReport {
    let (claude, cursor, ollama) = tokio::join!(
        async {
            tokio::task::spawn_blocking(scan_claude_desktop)
                .await
                .unwrap_or_else(|err| join_error(SOURCE_CLAUDE, err))
        },
        async {
            tokio::task::spawn_blocking(move || scan_cursor(&workspace))
                .await
                .unwrap_or_else(|err| join_error(SOURCE_CURSOR, err))
        },
        scan_ollama(&ollama_endpoint)
    );

    let mut sources = Vec::new();
    let mut tools = Vec::new();
    for (source, rows) in [claude, cursor, ollama] {
        sources.push(source);
        tools.extend(rows);
    }

    DiscoveryReport {
        scanned_at: now_rfc3339(),
        sources,
        tools,
    }
}

fn join_error(id: &str, err: tokio::task::JoinError) -> (DiscoverySource, Vec<DiscoveredTool>) {
    (
        DiscoverySource {
            id: id.to_string(),
            available: false,
            origin_path: None,
            detail: Some(format!("tarama iptal: {err}")),
        },
        Vec::new(),
    )
}

pub fn scan_claude_desktop() -> (DiscoverySource, Vec<DiscoveredTool>) {
    scan_mcp_path(SOURCE_CLAUDE, &claude_config_path())
}

pub fn scan_cursor(workspace: &Path) -> (DiscoverySource, Vec<DiscoveredTool>) {
    let paths = cursor_config_paths(workspace);
    let mut found = Vec::new();
    let mut tools = Vec::new();
    let mut details = Vec::new();

    for path in &paths {
        match read_mcp_file(path, SOURCE_CURSOR) {
            Ok(Some(rows)) => {
                found.push(path.display().to_string());
                if rows.is_empty() {
                    details.push(format!("{} · mcpServers yok", path.display()));
                }
                tools.extend(rows);
            }
            Ok(None) => {}
            Err(err) => {
                found.push(path.display().to_string());
                details.push(err);
            }
        }
    }

    let available = !found.is_empty();
    let origin_path = found.first().cloned();
    let detail = if available {
        if details.is_empty() {
            Some(found.join(", "))
        } else {
            Some(format!("{} · {}", found.join(", "), details.join("; ")))
        }
    } else {
        Some("Cursor mcp.json / settings.json bulunamadı".into())
    };

    (
        DiscoverySource {
            id: SOURCE_CURSOR.into(),
            available,
            origin_path,
            detail,
        },
        dedupe_tools(tools),
    )
}

pub async fn scan_ollama(endpoint: &str) -> (DiscoverySource, Vec<DiscoveredTool>) {
    let url = format!("{}/api/tags", endpoint.trim_end_matches('/'));
    match http_json(&url).await {
        Ok(payload) => {
            let tools = parse_ollama_tags(&payload, endpoint);
            let detail = if tools.is_empty() {
                Some("Ollama ayakta, model yok".into())
            } else {
                Some(format!("{} model", tools.len()))
            };
            (
                DiscoverySource {
                    id: SOURCE_OLLAMA.into(),
                    available: true,
                    origin_path: Some(url),
                    detail,
                },
                tools,
            )
        }
        Err(err) => (
            DiscoverySource {
                id: SOURCE_OLLAMA.into(),
                available: false,
                origin_path: Some(url),
                detail: Some(err),
            },
            Vec::new(),
        ),
    }
}

pub fn parse_mcp_servers(value: &Value, source: &str, origin_path: &Path) -> Vec<DiscoveredTool> {
    let Some(servers) = value.get("mcpServers").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut tools = Vec::with_capacity(servers.len());
    for (name, spec) in servers {
        let mut tool = DiscoveredTool::new(source, name, "mcp");
        tool.origin_path = Some(origin_path.display().to_string());
        tool.command = spec
            .get("command")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        tool.args = spec
            .get("args")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        tool.endpoint = spec
            .get("url")
            .or_else(|| spec.get("endpoint"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        tool.detail = env_key_detail(spec.get("env"));
        tool.available = true;
        tools.push(tool);
    }
    tools
}

pub fn parse_ollama_tags(payload: &Value, endpoint: &str) -> Vec<DiscoveredTool> {
    payload
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("name").and_then(Value::as_str))
        .map(|name| {
            let mut tool = DiscoveredTool::new(SOURCE_OLLAMA, name, "model");
            tool.endpoint = Some(endpoint.trim_end_matches('/').to_string());
            tool.origin_path = Some(format!("{}/api/tags", endpoint.trim_end_matches('/')));
            tool.available = true;
            tool
        })
        .collect()
}

fn scan_mcp_path(source: &str, path: &Path) -> (DiscoverySource, Vec<DiscoveredTool>) {
    match read_mcp_file(path, source) {
        Ok(Some(tools)) => (
            DiscoverySource {
                id: source.into(),
                available: true,
                origin_path: Some(path.display().to_string()),
                detail: Some(format!("{} araç", tools.len())),
            },
            tools,
        ),
        Ok(None) => (
            DiscoverySource {
                id: source.into(),
                available: false,
                origin_path: Some(path.display().to_string()),
                detail: Some("config dosyası yok".into()),
            },
            Vec::new(),
        ),
        Err(err) => (
            DiscoverySource {
                id: source.into(),
                available: true,
                origin_path: Some(path.display().to_string()),
                detail: Some(err),
            },
            Vec::new(),
        ),
    }
}

fn read_mcp_file(path: &Path, source: &str) -> Result<Option<Vec<DiscoveredTool>>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .map_err(|err| format!("JSON parse hatası {}: {err}", path.display()))?;
    Ok(Some(parse_mcp_servers(&value, source, path)))
}

fn env_key_detail(env: Option<&Value>) -> Option<String> {
    let keys = env.and_then(Value::as_object).map(|map| {
        let mut names: Vec<&str> = map.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    })?;
    if keys.is_empty() {
        return None;
    }
    Some(format!("env keys: {}", keys.join(", ")))
}

fn dedupe_tools(tools: Vec<DiscoveredTool>) -> Vec<DiscoveredTool> {
    let mut seen = std::collections::HashSet::new();
    tools
        .into_iter()
        .filter(|tool| seen.insert(tool.id.clone()))
        .collect()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn claude_config_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        return home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/Claude/claude_desktop_config.json");
    }
    if cfg!(target_os = "windows") {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("Claude/claude_desktop_config.json");
        }
        return home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("AppData/Roaming/Claude/claude_desktop_config.json");
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/Claude/claude_desktop_config.json")
}

fn cursor_user_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        return home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Library/Application Support/Cursor");
    }
    if cfg!(target_os = "windows") {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("Cursor");
        }
        return home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("AppData/Roaming/Cursor");
    }
    home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/Cursor")
}

fn cursor_config_paths(workspace: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = home_dir() {
        paths.push(home.join(".cursor/mcp.json"));
    }
    paths.push(workspace.join(".cursor/mcp.json"));
    paths.push(cursor_user_dir().join("User/settings.json"));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_claude_mcp_servers_and_redacts_env_values() {
        let secret = "sk-live-super-secret";
        let doc = json!({
            "mcpServers": {
                "github": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-github"],
                    "env": {
                        "GITHUB_TOKEN": secret,
                        "ANOTHER": "also-secret"
                    }
                }
            }
        });
        let path = PathBuf::from("/tmp/claude_desktop_config.json");
        let tools = parse_mcp_servers(&doc, SOURCE_CLAUDE, &path);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].id, "claude_desktop:github");
        assert_eq!(tools[0].kind, "mcp");
        assert_eq!(tools[0].command.as_deref(), Some("npx"));
        assert_eq!(
            tools[0].args,
            vec!["-y", "@modelcontextprotocol/server-github"]
        );
        let detail = tools[0].detail.clone().unwrap_or_default();
        assert!(detail.contains("GITHUB_TOKEN"));
        assert!(detail.contains("ANOTHER"));
        assert!(!detail.contains(secret));
        assert!(!detail.contains("also-secret"));

        let serialized = serde_json::to_string(&tools[0]).expect("json");
        assert!(!serialized.contains(secret));
        assert!(!serialized.contains("also-secret"));
        assert!(!serialized.contains("sk-live"));
    }

    #[test]
    fn parses_cursor_mcp_fixture_file() {
        let dir = std::env::temp_dir().join(format!("lounge-mcp-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("mcp.json");
        fs::write(
            &path,
            r#"{
              "mcpServers": {
                "notion": {
                  "command": "npx",
                  "args": ["-y", "@notionhq/mcp"],
                  "env": { "NOTION_TOKEN": "ntn_secret_value" }
                }
              }
            }"#,
        )
        .expect("write fixture");

        let rows = read_mcp_file(&path, SOURCE_CURSOR)
            .expect("read")
            .expect("exists");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "cursor:notion");
        let blob = serde_json::to_string(&rows[0]).expect("json");
        assert!(!blob.contains("ntn_secret_value"));
        assert!(!blob.to_ascii_lowercase().contains("env\":{"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_mcp_file_is_unavailable() {
        let path = PathBuf::from("/tmp/lounge-missing-claude-config.json");
        let (source, tools) = scan_mcp_path(SOURCE_CLAUDE, &path);
        assert!(!source.available);
        assert!(tools.is_empty());
    }

    #[test]
    fn ollama_tags_become_model_rows() {
        let payload = json!({
            "models": [
                { "name": "llama3.1:8b" },
                { "name": "qwen2.5:7b" }
            ]
        });
        let tools = parse_ollama_tags(&payload, "http://127.0.0.1:11434");
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].id, "ollama:llama3.1:8b");
        assert_eq!(tools[0].kind, "model");
        assert_eq!(tools[0].source, "ollama");
    }

    #[tokio::test]
    async fn ollama_down_returns_empty_models() {
        let (source, tools) = scan_ollama("http://127.0.0.1:9").await;
        assert!(!source.available);
        assert!(tools.is_empty());
        assert!(source.detail.is_some());
    }
}
