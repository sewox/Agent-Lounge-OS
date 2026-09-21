use crate::models::ToolQuota;
use crate::services::{collect_quota_state, MemoryBridge};

pub async fn probe_quotas(
    ollama_endpoint: &str,
    nats_monitor: &str,
    memory: &MemoryBridge,
) -> Vec<ToolQuota> {
    collect_quota_state(ollama_endpoint, nats_monitor, memory)
        .await
        .quotas
}

pub fn quota_exhausted_for(quotas: &[ToolQuota], agent: &str) -> bool {
    quotas
        .iter()
        .filter(|row| quota_matches_agent(row, agent))
        .any(ToolQuota::is_exhausted)
}

pub fn quota_matches_agent(row: &ToolQuota, agent: &str) -> bool {
    let needle = agent.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return false;
    }
    let id = row.id.to_ascii_lowercase();
    let mode = row.access_mode.to_ascii_lowercase();
    let host = row
        .host_id
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    match needle.as_str() {
        "claude" | "claude_desktop" | "claude_cli" => {
            mode == "subscription"
                && (id.contains("claude_desktop")
                    || id.contains("claude_cli")
                    || host == "claude_desktop"
                    || host == "claude_cli")
        }
        "anthropic" => mode == "api" && (id == "anthropic" || id.contains("anthropic")),
        "cursor" => mode == "subscription" && (id.contains("cursor") || host == "cursor"),
        "antigravity" => {
            mode == "subscription" && (id.contains("antigravity") || host == "antigravity")
        }
        "grok" | "grok_bot" => {
            mode == "subscription" && (id.contains("grok_bot") || host == "grok_bot")
        }
        "xai" | "grok_api" => mode == "api" && (id == "xai" || id.contains("xai")),
        "openai" => mode == "api" && id.contains("openai"),
        "lmr" | "ollama" => mode == "local" && (id == "lmr" || id.contains("ollama")),
        other => id == other || id.ends_with(&format!(":{other}")) || host == other,
    }
}

pub(crate) async fn http_json(url: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|err| err.to_string())?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    response.json().await.map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, tool: &str, mode: &str, host: Option<&str>, exhausted: bool) -> ToolQuota {
        ToolQuota {
            id: id.into(),
            tool: tool.into(),
            percent: exhausted.then_some(100.0),
            exhausted,
            ..ToolQuota::default()
        }
        .with_mode(mode, host)
    }

    #[test]
    fn exhausted_matches_subscription_cursor() {
        let quotas = vec![row(
            "app:cursor",
            "Cursor",
            "subscription",
            Some("cursor"),
            true,
        )];
        assert!(quota_exhausted_for(&quotas, "cursor"));
        assert!(!quota_exhausted_for(&quotas, "ollama"));
    }

    #[test]
    fn claude_agent_does_not_use_anthropic_api_row() {
        let quotas = vec![
            row(
                "app:claude_desktop",
                "Claude Desktop",
                "subscription",
                Some("claude_desktop"),
                false,
            ),
            row("anthropic", "Anthropic API", "api", None, true),
        ];
        assert!(!quota_exhausted_for(&quotas, "claude"));
        assert!(quota_exhausted_for(&quotas, "anthropic"));
    }

    #[test]
    fn grok_bot_does_not_use_xai_api_row() {
        let quotas = vec![
            row(
                "app:grok_bot",
                "Grok Bot",
                "subscription",
                Some("grok_bot"),
                false,
            ),
            row("xai", "Grok API", "api", None, true),
        ];
        assert!(!quota_exhausted_for(&quotas, "grok"));
        assert!(quota_exhausted_for(&quotas, "xai"));
    }
}
