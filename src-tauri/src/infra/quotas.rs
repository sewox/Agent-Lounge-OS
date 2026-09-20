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
    let needle = agent.to_ascii_lowercase();
    quotas
        .iter()
        .filter(|row| {
            row.id.to_ascii_lowercase().contains(&needle)
                || row.tool.to_ascii_lowercase().contains(&needle)
        })
        .any(ToolQuota::is_exhausted)
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

    #[test]
    fn exhausted_matches_agent_id() {
        let quotas = vec![ToolQuota {
            id: "cursor".into(),
            tool: "Cursor".into(),
            kind: "ai".into(),
            unit: "req".into(),
            used: "500 / 500".into(),
            remaining: "0".into(),
            reset: "00:00 UTC".into(),
            percent: Some(100.0),
            tone: "warn".into(),
            label: "exhausted".into(),
            source: "infra".into(),
            exhausted: true,
        }];
        assert!(quota_exhausted_for(&quotas, "cursor"));
        assert!(!quota_exhausted_for(&quotas, "ollama"));
    }
}
