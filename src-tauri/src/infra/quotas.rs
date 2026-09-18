use std::time::Duration;

use crate::models::ToolQuota;
use crate::services::MemoryBridge;

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn probe_quotas(
    ollama_endpoint: &str,
    nats_monitor: &str,
    memory: &MemoryBridge,
) -> Vec<ToolQuota> {
    let (ollama, nats, mem) = tokio::join!(
        probe_ollama(ollama_endpoint),
        probe_nats(nats_monitor),
        async { probe_memory(memory) }
    );
    let mut rows = vec![ollama, nats, mem];
    rows.extend([
        unconfigured("cursor", "Cursor · Grok 4.6", "ai", "provider API yok"),
        unconfigured("claude", "Claude · Sonnet", "ai", "provider API yok"),
        unconfigured("xai", "xAI · Grok API", "ai", "provider API yok"),
        unconfigured("stitch", "Stitch · Gemini", "bot", "provider API yok"),
    ]);
    rows
}

fn unconfigured(id: &str, tool: &str, kind: &str, label: &str) -> ToolQuota {
    ToolQuota {
        id: id.into(),
        tool: tool.into(),
        kind: kind.into(),
        unit: "api".into(),
        used: "unconfigured".into(),
        remaining: "—".into(),
        reset: "—".into(),
        percent: None,
        tone: "warn".into(),
        label: label.into(),
        source: "infra".into(),
        exhausted: false,
    }
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

async fn probe_ollama(endpoint: &str) -> ToolQuota {
    let url = format!("{endpoint}/api/ps");
    match http_json(&url).await {
        Ok(payload) => {
            let running = payload
                .get("models")
                .and_then(|v| v.as_array())
                .map(|rows| rows.len())
                .unwrap_or(0);
            let size: u64 = payload
                .get("models")
                .and_then(|v| v.as_array())
                .map(|rows| {
                    rows.iter()
                        .filter_map(|row| row.get("size").and_then(|v| v.as_u64()))
                        .sum()
                })
                .unwrap_or(0);
            let gb = size as f32 / (1024.0 * 1024.0 * 1024.0);
            ToolQuota {
                id: "ollama".into(),
                tool: "Ollama · local".into(),
                kind: "ai".into(),
                unit: "local".into(),
                used: format!("{running} loaded · {gb:.1} GB"),
                remaining: "unlimited".into(),
                reset: "LOCAL".into(),
                percent: None,
                tone: "local".into(),
                label: "ok LOCAL".into(),
                source: url,
                exhausted: false,
            }
        }
        Err(err) => ToolQuota {
            id: "ollama".into(),
            tool: "Ollama · local".into(),
            kind: "ai".into(),
            unit: "local".into(),
            used: "down".into(),
            remaining: "—".into(),
            reset: "LOCAL".into(),
            percent: None,
            tone: "warn".into(),
            label: err,
            source: url,
            exhausted: false,
        },
    }
}

async fn probe_nats(monitor: &str) -> ToolQuota {
    let url = format!("{monitor}/varz");
    match http_json(&url).await {
        Ok(payload) => {
            let connections = payload
                .get("connections")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let in_msgs = payload.get("in_msgs").and_then(|v| v.as_u64()).unwrap_or(0);
            ToolQuota {
                id: "nats".into(),
                tool: "NATS broker".into(),
                kind: "bot".into(),
                unit: "conn".into(),
                used: format!("{connections} conn · {in_msgs} in_msgs"),
                remaining: "local".into(),
                reset: "LOCAL".into(),
                percent: None,
                tone: "ok".into(),
                label: "ok LOCAL".into(),
                source: url,
                exhausted: false,
            }
        }
        Err(_) => ToolQuota {
            id: "nats".into(),
            tool: "NATS broker".into(),
            kind: "bot".into(),
            unit: "conn".into(),
            used: "monitor kapalı".into(),
            remaining: "—".into(),
            reset: "LOCAL".into(),
            percent: None,
            tone: "warn".into(),
            label: "no /varz".into(),
            source: url,
            exhausted: false,
        },
    }
}

fn probe_memory(memory: &MemoryBridge) -> ToolQuota {
    let health = memory.diagnose();
    ToolQuota {
        id: "cbm".into(),
        tool: "codebase-memory-mcp".into(),
        kind: "bot".into(),
        unit: "local".into(),
        used: if health.running { "ready" } else { "missing" }.into(),
        remaining: "unlimited".into(),
        reset: "LOCAL".into(),
        percent: None,
        tone: if health.running { "local" } else { "warn" }.into(),
        label: if health.running {
            "ok LOCAL".into()
        } else {
            health.error.unwrap_or_else(|| "binary yok".into())
        },
        source: health.endpoint,
        exhausted: false,
    }
}

pub(crate) async fn http_json(url: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
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
