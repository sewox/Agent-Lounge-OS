use crate::models::ToolQuota;
use crate::services::{collect_quota_state, MemoryBridge};

pub use crate::services::quota_manager::{
    evaluate_assignment as check_quota, limit_policy_percent, quota_blocked_for,
    quota_exhausted_for, quota_matches_agent,
};

pub async fn probe_quotas(
    ollama_endpoint: &str,
    nats_monitor: &str,
    memory: &MemoryBridge,
) -> Vec<ToolQuota> {
    collect_quota_state(ollama_endpoint, nats_monitor, memory)
        .await
        .quotas
}

/// Hafif GET JSON — keşif / Ollama tags (subscription_usage HTTP'sinden bağımsız).
pub async fn http_json(url: &str) -> Result<serde_json::Value, String> {
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
    use crate::models::{QuotaVerdict, LIMIT_POLICY_PERCENT};

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
        assert!(!quota_matches_agent(&quotas[1], "claude"));
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

    #[test]
    fn check_quota_block_matches_verdict() {
        let quotas = vec![row(
            "app:cursor",
            "Cursor",
            "subscription",
            Some("cursor"),
            true,
        )];
        let verdict = check_quota(&quotas, "cursor", LIMIT_POLICY_PERCENT);
        assert!(matches!(verdict, QuotaVerdict::Block { .. }));
    }

    #[test]
    fn limit_policy_blocks_at_ninety() {
        let quotas = vec![ToolQuota {
            id: "app:cursor".into(),
            tool: "Cursor".into(),
            percent: Some(90.0),
            exhausted: false,
            ..ToolQuota::default()
        }
        .with_mode("subscription", Some("cursor"))];
        assert!(quota_blocked_for(&quotas, "cursor", LIMIT_POLICY_PERCENT));
        assert!(matches!(
            check_quota(&quotas, "cursor", LIMIT_POLICY_PERCENT),
            QuotaVerdict::Block { .. }
        ));
    }
}
