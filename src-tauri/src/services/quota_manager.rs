//! Periyodik kota: LMR RAM + (anahtar varsa) OpenAI / Anthropic / Grok usage.
//! %80 üzeri → Amber Alert. Host Ollama sürecine dokunulmaz.

use std::time::Duration;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use tokio::time::sleep;

use super::MemoryBridge;
use super::SharedServices;
use crate::models::{now_rfc3339, QuotaState, ToolQuota, AMBER_THRESHOLD, QUOTA_EVENT};

const POLL: Duration = Duration::from_secs(60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
const NATS_MONITOR: &str = "http://127.0.0.1:8222";
const DEFAULT_RAM_BUDGET_GB: f32 = 8.0;

pub fn spawn_quota_pump(app: AppHandle, services: SharedServices) {
    tauri::async_runtime::spawn(async move {
        run_quota_pump(app, services).await;
    });
}

pub async fn run_quota_pump(app: AppHandle, services: SharedServices) {
    loop {
        let (endpoint, memory) = {
            let manager = services.lock().await;
            (manager.ollama_endpoint(), manager.memory().clone())
        };
        let state = collect_quota_state(&endpoint, NATS_MONITOR, &memory).await;
        emit_quota_state(&app, &state);
        sleep(POLL).await;
    }
}

pub async fn collect_quota_state(
    ollama_endpoint: &str,
    nats_monitor: &str,
    memory: &MemoryBridge,
) -> QuotaState {
    let (lmr, nats, mem, openai, anthropic, grok) = tokio::join!(
        probe_ollama_ram(ollama_endpoint),
        probe_nats(nats_monitor),
        async { probe_memory(memory) },
        probe_openai(),
        probe_anthropic(),
        probe_grok(),
    );
    QuotaState::from_quotas(vec![lmr, nats, mem, openai, anthropic, grok])
}

fn emit_quota_state(app: &AppHandle, state: &QuotaState) {
    if state.amber_alert {
        log::warn!(
            "Amber Alert: {} kota ≥ {AMBER_THRESHOLD}% ({})",
            state.amber_tools.len(),
            state.amber_tools.join(", ")
        );
    }
    match app.get_webview_window("main") {
        Some(window) => {
            if let Err(err) = window.emit(QUOTA_EVENT, state) {
                log::debug!("{QUOTA_EVENT} emit: {err}");
            }
        }
        None => log::debug!("main window yok, {QUOTA_EVENT} düştü"),
    }
}

fn env_key(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

fn env_f32(name: &str, default: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0.0)
        .unwrap_or(default)
}

pub fn ram_percent(bytes: u64, budget_gb: f32) -> Option<f32> {
    if budget_gb <= 0.0 {
        return None;
    }
    let used_gb = bytes as f32 / (1024.0 * 1024.0 * 1024.0);
    Some(((used_gb / budget_gb) * 100.0).clamp(0.0, 999.0))
}

pub fn tone_for_percent(percent: Option<f32>, fallback: &str) -> String {
    match percent {
        Some(value) if value >= AMBER_THRESHOLD => "amber".into(),
        Some(_) => "ok".into(),
        None => fallback.into(),
    }
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
        source: "env".into(),
        exhausted: false,
    }
}

async fn probe_ollama_ram(endpoint: &str) -> ToolQuota {
    let url = format!("{}/api/ps", endpoint.trim_end_matches('/'));
    let budget = env_f32("LOUNGE_LMR_RAM_BUDGET_GB", DEFAULT_RAM_BUDGET_GB);
    match http_json(&url, &[]).await {
        Ok(payload) => {
            let models = payload
                .get("models")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let size: u64 = models
                .iter()
                .filter_map(|row| row.get("size").and_then(Value::as_u64))
                .sum();
            let percent = ram_percent(size, budget);
            let gb = size as f32 / (1024.0 * 1024.0 * 1024.0);
            ToolQuota {
                id: "lmr".into(),
                tool: "LMR".into(),
                kind: "ai".into(),
                unit: "ram".into(),
                used: format!("{gb:.1} / {budget:.1} GB"),
                remaining: format!("{:.1} GB", (budget - gb).max(0.0)),
                reset: "LOCAL".into(),
                percent,
                tone: tone_for_percent(percent, "local"),
                label: percent
                    .map(|value| format!("{:.0}% ram", value))
                    .unwrap_or_else(|| "ok LOCAL".into()),
                source: url,
                exhausted: false,
            }
        }
        Err(err) => ToolQuota {
            id: "lmr".into(),
            tool: "LMR".into(),
            kind: "ai".into(),
            unit: "ram".into(),
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
    let url = format!("{}/varz", monitor.trim_end_matches('/'));
    match http_json(&url, &[]).await {
        Ok(payload) => {
            let connections = payload
                .get("connections")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let in_msgs = payload.get("in_msgs").and_then(Value::as_u64).unwrap_or(0);
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

async fn probe_openai() -> ToolQuota {
    let Some(key) = env_key(&["OPENAI_API_KEY"]) else {
        return unconfigured("openai", "OpenAI", "ai", "API key yok");
    };
    let start = midnight_unix();
    let url = format!(
        "https://api.openai.com/v1/organization/usage/completions?start_time={start}&bucket_width=1d"
    );
    let budget = env_f32("OPENAI_TOKEN_BUDGET", 0.0);
    match http_json(
        &url,
        &[
            ("Authorization", format!("Bearer {key}")),
            ("OpenAI-Beta", "usage=v1".into()),
        ],
    )
    .await
    {
        Ok(payload) => usage_row(
            "openai",
            "OpenAI",
            parse_openai_tokens(&payload),
            budget,
            url,
        ),
        Err(err) => key_present_no_usage("openai", "OpenAI", err),
    }
}

async fn probe_anthropic() -> ToolQuota {
    let Some(key) = env_key(&["ANTHROPIC_API_KEY"]) else {
        return unconfigured("anthropic", "Anthropic · Claude", "ai", "API key yok");
    };
    let start = now_rfc3339();
    let day_start = format!("{}T00:00:00Z", &start[..10.min(start.len())]);
    let url = format!(
        "https://api.anthropic.com/v1/organizations/usage_report/messages?starting_at={day_start}&bucket_width=1d"
    );
    let budget = env_f32("ANTHROPIC_TOKEN_BUDGET", 0.0);
    match http_json(
        &url,
        &[
            ("x-api-key", key),
            ("anthropic-version", "2023-06-01".into()),
        ],
    )
    .await
    {
        Ok(payload) => usage_row(
            "anthropic",
            "Anthropic · Claude",
            parse_anthropic_tokens(&payload),
            budget,
            url,
        ),
        Err(err) => key_present_no_usage("anthropic", "Anthropic · Claude", err),
    }
}

async fn probe_grok() -> ToolQuota {
    let Some(key) = env_key(&["XAI_API_KEY", "GROK_API_KEY"]) else {
        return unconfigured("xai", "xAI · Grok", "ai", "API key yok");
    };
    let url = "https://api.x.ai/v1/api-key";
    let budget = env_f32("XAI_CREDIT_BUDGET", 0.0);
    match http_json(url, &[("Authorization", format!("Bearer {key}"))]).await {
        Ok(payload) => match parse_xai_usage(&payload) {
            Some((used, limit)) => {
                let cap = if budget > 0.0 { budget } else { limit };
                let percent = if cap > 0.0 {
                    Some(((used / cap) * 100.0).clamp(0.0, 999.0))
                } else {
                    None
                };
                ToolQuota {
                    id: "xai".into(),
                    tool: "xAI · Grok".into(),
                    kind: "ai".into(),
                    unit: "credits".into(),
                    used: format!("{used:.0} / {cap:.0}"),
                    remaining: format!("{:.0}", (cap - used).max(0.0)),
                    reset: "rolling".into(),
                    percent,
                    tone: tone_for_percent(percent, "ok"),
                    label: percent
                        .map(|value| format!("{value:.0}%"))
                        .unwrap_or_else(|| "key aktif".into()),
                    source: url.into(),
                    exhausted: percent.map(|value| value >= 100.0).unwrap_or(false),
                }
            }
            None => key_present_no_usage("xai", "xAI · Grok", "usage alanı yok".into()),
        },
        Err(err) => key_present_no_usage("xai", "xAI · Grok", err),
    }
}

fn usage_row(id: &str, tool: &str, used: u64, budget: f32, source: String) -> ToolQuota {
    let percent = if budget > 0.0 {
        Some(((used as f32 / budget) * 100.0).clamp(0.0, 999.0))
    } else {
        None
    };
    ToolQuota {
        id: id.into(),
        tool: tool.into(),
        kind: "ai".into(),
        unit: "tokens".into(),
        used: if budget > 0.0 {
            format!("{used} / {budget:.0}")
        } else {
            format!("{used} tokens")
        },
        remaining: if budget > 0.0 {
            format!("{:.0}", (budget - used as f32).max(0.0))
        } else {
            "—".into()
        },
        reset: "24h".into(),
        percent,
        tone: tone_for_percent(percent, "ok"),
        label: percent
            .map(|value| format!("{value:.0}%"))
            .unwrap_or_else(|| "key aktif".into()),
        source,
        exhausted: percent.map(|value| value >= 100.0).unwrap_or(false),
    }
}

fn key_present_no_usage(id: &str, tool: &str, detail: String) -> ToolQuota {
    ToolQuota {
        id: id.into(),
        tool: tool.into(),
        kind: "ai".into(),
        unit: "api".into(),
        used: "key var".into(),
        remaining: "usage yok".into(),
        reset: "—".into(),
        percent: None,
        tone: "ok".into(),
        label: short_err(&detail),
        source: "provider".into(),
        exhausted: false,
    }
}

fn short_err(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.chars().count() <= 48 {
        return trimmed.to_string();
    }
    trimmed.chars().take(45).collect::<String>() + "…"
}

pub fn parse_openai_tokens(payload: &Value) -> u64 {
    payload
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|bucket| {
            bucket
                .get("results")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .map(|row| {
            row.get("input_tokens").and_then(Value::as_u64).unwrap_or(0)
                + row
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
        })
        .sum()
}

pub fn parse_anthropic_tokens(payload: &Value) -> u64 {
    payload
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|bucket| {
            bucket
                .get("results")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .map(|row| {
            row.get("uncached_input_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                + row
                    .get("output_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
        })
        .sum()
}

pub fn parse_xai_usage(payload: &Value) -> Option<(f32, f32)> {
    let used = payload
        .get("used_credits")
        .or_else(|| payload.get("spend"))
        .and_then(Value::as_f64)
        .or_else(|| {
            let remaining = payload.get("remaining_credits").and_then(Value::as_f64)?;
            let limit = payload
                .get("limit")
                .or_else(|| payload.get("credits"))
                .and_then(Value::as_f64)?;
            Some(limit - remaining)
        })?;
    let limit = payload
        .get("limit")
        .or_else(|| payload.get("credits"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    Some((used as f32, limit as f32))
}

fn midnight_unix() -> i64 {
    let now = chrono::Utc::now();
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|naive| naive.and_utc().timestamp())
        .unwrap_or_else(|| now.timestamp())
}

async fn http_json(url: &str, headers: &[(&str, String)]) -> Result<Value, String> {
    let mut builder = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|err| err.to_string())?
        .get(url);
    for (key, value) in headers {
        builder = builder.header(*key, value);
    }
    let response = builder.send().await.map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    response.json().await.map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ram_percent_hits_amber_at_eighty() {
        let budget = 8.0;
        let eighty = (0.80 * budget * 1024.0 * 1024.0 * 1024.0) as u64;
        let percent = ram_percent(eighty, budget).unwrap();
        assert!((percent - 80.0).abs() < 0.5);
        let state = QuotaState::from_quotas(vec![ToolQuota {
            id: "lmr".into(),
            tool: "LMR".into(),
            kind: "ai".into(),
            unit: "ram".into(),
            used: "6.4 / 8.0 GB".into(),
            remaining: "1.6 GB".into(),
            reset: "LOCAL".into(),
            percent: Some(percent),
            tone: tone_for_percent(Some(percent), "local"),
            label: "80% ram".into(),
            source: "test".into(),
            exhausted: false,
        }]);
        assert!(state.amber_alert);
        assert_eq!(state.amber_tools, vec!["lmr"]);
    }

    #[test]
    fn seventy_nine_is_not_amber() {
        let state = QuotaState::from_quotas(vec![ToolQuota {
            id: "openai".into(),
            tool: "OpenAI".into(),
            kind: "ai".into(),
            unit: "tokens".into(),
            used: "79".into(),
            remaining: "21".into(),
            reset: "24h".into(),
            percent: Some(79.0),
            tone: "ok".into(),
            label: "79%".into(),
            source: "test".into(),
            exhausted: false,
        }]);
        assert!(!state.amber_alert);
        assert!(state.amber_tools.is_empty());
    }

    #[test]
    fn parses_openai_and_anthropic_usage_buckets() {
        let openai = json!({
            "data": [{
                "results": [
                    { "input_tokens": 1000, "output_tokens": 200 },
                    { "input_tokens": 50, "output_tokens": 25 }
                ]
            }]
        });
        assert_eq!(parse_openai_tokens(&openai), 1275);

        let anthropic = json!({
            "data": [{
                "results": [
                    { "uncached_input_tokens": 80, "output_tokens": 20 }
                ]
            }]
        });
        assert_eq!(parse_anthropic_tokens(&anthropic), 100);
    }

    #[test]
    fn parses_xai_remaining_credits() {
        let payload = json!({
            "remaining_credits": 20.0,
            "limit": 100.0
        });
        assert_eq!(parse_xai_usage(&payload), Some((80.0, 100.0)));
    }
}
