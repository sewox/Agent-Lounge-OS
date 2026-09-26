//! Periyodik kota: LMR RAM/VRAM (sysinfo) + abonelik yerel plan dosyaları +
//! (anahtar varsa) OpenAI / Anthropic / xAI usage. 30 sn'de bir `quota-update`.
//! Host Ollama (:11434) sürecine dokunulmaz; hesap sayfasına cookie ile gidilmez.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::{AppHandle, Emitter, Manager};
use tokio::time::sleep;

use super::autodiscover::scan_subscription_hosts;
use super::hardware::{device_profile, is_apple_silicon};
use super::plugin::{lounge_workspace, scan_plugin_catalog};
use super::probe::{
    lounge_ollama_endpoint, tcp_ready, DEFAULT_NATS_HOST, DEFAULT_NATS_PORT, LOUNGE_OLLAMA_PORT,
    SYSTEM_OLLAMA_PORT,
};
use super::subscription_usage::{live_subscription_usage, local_subscription_usage};
use super::MemoryBridge;
use super::SharedServices;
use crate::db::ExperienceStore;
use crate::models::{
    is_kernel, now_rfc3339, ApprovalKind, ApprovalRequest, DiscoveredTool, QuotaState,
    QuotaVerdict, ToolQuota, AMBER_THRESHOLD, LIMIT_POLICY_PERCENT, QUOTA_EVENT,
};

const POLL: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
const LMR_PORT_MARK: &str = "18790";
const LMR_HEALTH_WAIT: Duration = Duration::from_millis(250);

/// Overlay / NATS Quota Alert metni (Stitch / TR).
pub const QUOTA_ALERT_PROMPT: &str =
    "Kota limiti aşıldı. Görev durduruldu. Yerel LMR ile devam edebilirsiniz.";
/// UI düğmesi — metin birebir sabit.
pub const QUOTA_CONTINUE_LOCAL_LABEL: &str = "Yerel Model (Ollama) ile devam et";
/// Görev askı durumu (protokol / UI).
pub const QUOTA_PENDING: &str = "QUOTA_BLOCKED";

/// Env `LOUNGE_QUOTA_LIMIT_PERCENT` yoksa Limit Policy = 90%.
pub fn limit_policy_percent() -> f32 {
    env_f32("LOUNGE_QUOTA_LIMIT_PERCENT", LIMIT_POLICY_PERCENT)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApiKeys {
    pub openai: Option<String>,
    pub anthropic: Option<String>,
    pub grok: Option<String>,
}

impl ApiKeys {
    pub fn from_env() -> Self {
        Self {
            openai: env_key(&["OPENAI_API_KEY"]),
            anthropic: env_key(&["ANTHROPIC_API_KEY", "CLAUDE_API_KEY"]),
            grok: env_key(&["XAI_API_KEY", "GROK_API_KEY"]),
        }
    }

    pub fn merge_json(&mut self, raw: &str) {
        let Ok(value) = serde_json::from_str::<Value>(raw) else {
            return;
        };
        let Value::Object(map) = value else {
            return;
        };
        if self.openai.is_none() {
            self.openai = json_secret(&map, &["openai", "OPENAI_API_KEY", "openai_api_key"]);
        }
        if self.anthropic.is_none() {
            self.anthropic = json_secret(
                &map,
                &["anthropic", "claude", "ANTHROPIC_API_KEY", "CLAUDE_API_KEY"],
            );
        }
        if self.grok.is_none() {
            self.grok = json_secret(&map, &["grok", "xai", "XAI_API_KEY", "GROK_API_KEY"]);
        }
    }

    pub fn fill_if_empty(
        &mut self,
        openai: Option<String>,
        anthropic: Option<String>,
        grok: Option<String>,
    ) {
        if self.openai.is_none() {
            self.openai = openai;
        }
        if self.anthropic.is_none() {
            self.anthropic = anthropic;
        }
        if self.grok.is_none() {
            self.grok = grok;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LmrProcessUsage {
    pub ram_bytes: u64,
    pub vram_bytes: u64,
    pub processes: usize,
}

pub fn spawn_quota_pump(app: AppHandle, services: SharedServices, store: ExperienceStore) {
    tauri::async_runtime::spawn(async move {
        run_quota_pump(app, services, store).await;
    });
}

pub async fn run_quota_pump(app: AppHandle, services: SharedServices, store: ExperienceStore) {
    loop {
        let (endpoint, nats_monitor, memory) = {
            let manager = services.lock().await;
            (
                manager.ollama_endpoint(),
                manager.nats_monitor_url(),
                manager.memory().clone(),
            )
        };
        let keys = api_keys_from_store(&store).await;
        let state = collect_quota_state_with_keys(&endpoint, &nats_monitor, &memory, &keys).await;
        emit_quota_state(&app, &state);
        sleep(POLL).await;
    }
}

pub async fn collect_quota_state(
    ollama_endpoint: &str,
    nats_monitor: &str,
    memory: &MemoryBridge,
) -> QuotaState {
    collect_quota_state_with_keys(ollama_endpoint, nats_monitor, memory, &ApiKeys::from_env()).await
}

pub async fn collect_quota_state_with_keys(
    ollama_endpoint: &str,
    nats_monitor: &str,
    memory: &MemoryBridge,
    keys: &ApiKeys,
) -> QuotaState {
    let workspace = lounge_workspace();
    let (
        hosts,
        plugins,
        lmr,
        nats,
        mem,
        openai,
        anthropic,
        grok,
        cursor_live,
        grok_bot_live,
        anti_live,
        claude_live,
    ) = tokio::join!(
        async {
            tokio::task::spawn_blocking(scan_subscription_hosts)
                .await
                .unwrap_or_default()
        },
        async {
            tokio::task::spawn_blocking(move || scan_plugin_catalog(&workspace).plugins)
                .await
                .unwrap_or_default()
        },
        probe_ollama(ollama_endpoint),
        probe_nats(nats_monitor),
        async { probe_memory(memory) },
        probe_openai(keys.openai.as_deref()),
        probe_anthropic(keys.anthropic.as_deref()),
        probe_grok(keys.grok.as_deref()),
        live_subscription_usage("cursor"),
        live_subscription_usage("grok_bot"),
        live_subscription_usage("antigravity"),
        live_subscription_usage("claude_desktop"),
    );
    let mut quotas = vec![lmr, nats, mem];
    quotas.extend(
        hosts
            .into_iter()
            .filter(|host| host.available)
            .flat_map(|host| {
                let live = match host.source.as_str() {
                    "cursor" => cursor_live.clone(),
                    "grok_bot" => grok_bot_live.clone(),
                    "antigravity" => anti_live.clone(),
                    "claude_desktop" | "claude_cli" => claude_live.clone(),
                    _ => None,
                };
                subscription_rows_with_live(&host, live)
            }),
    );
    quotas.extend(plugins.into_iter().map(|plugin| plugin_quota(&plugin)));
    quotas.extend(openai);
    quotas.extend(anthropic);
    quotas.extend(grok);
    merge_claude_subscription_rows(&mut quotas);
    QuotaState::from_quotas(quotas)
}

/// K9: collapse claude_desktop + claude_cli subscription rows into one `app:claude` account.
pub fn merge_claude_subscription_rows(quotas: &mut Vec<ToolQuota>) {
    let claude_idxs: Vec<usize> = quotas
        .iter()
        .enumerate()
        .filter(|(_, row)| is_claude_subscription_row(row))
        .map(|(i, _)| i)
        .collect();
    if claude_idxs.is_empty() {
        return;
    }
    // Prefer the row with live percent / richer used text.
    let keep = *claude_idxs
        .iter()
        .max_by_key(|&&i| {
            let row = &quotas[i];
            let live_score = row.percent.map(|p| (p * 100.0) as i64).unwrap_or(-1);
            let rich = if row.used.contains('/') || row.percent.is_some() {
                1_000_000
            } else {
                0
            };
            rich + live_score
        })
        .unwrap_or(&claude_idxs[0]);
    quotas[keep].id = "app:claude".into();
    quotas[keep].tool = "Claude".into();
    quotas[keep].host_id = Some("claude".into());
    let mut drop: Vec<usize> = claude_idxs.into_iter().filter(|&i| i != keep).collect();
    drop.sort_unstable_by(|a, b| b.cmp(a));
    for i in drop {
        quotas.remove(i);
    }
}

fn is_claude_subscription_row(row: &ToolQuota) -> bool {
    let id = row.id.to_ascii_lowercase();
    let host = row.host_id.as_deref().unwrap_or("").to_ascii_lowercase();
    let tool = row.tool.to_ascii_lowercase();
    id.contains("claude_desktop")
        || id.contains("claude_cli")
        || id == "app:claude"
        || host == "claude_desktop"
        || host == "claude_cli"
        || host == "claude"
        || (tool.contains("claude") && row.access_mode == "subscription")
}

pub async fn api_keys_from_store(store: &ExperienceStore) -> ApiKeys {
    let mut keys = ApiKeys::from_env();
    if let Ok(Some(raw)) = store.get_setting("api_keys".into()).await {
        keys.merge_json(&raw);
    }
    let openai = match store.get_setting("openai_api_key".into()).await {
        Ok(value) => value.and_then(|raw| parse_stored_secret(&raw)),
        Err(_) => None,
    };
    let anthropic = match store.get_setting("anthropic_api_key".into()).await {
        Ok(value) => value.and_then(|raw| parse_stored_secret(&raw)),
        Err(_) => None,
    }
    .or(match store.get_setting("claude_api_key".into()).await {
        Ok(value) => value.and_then(|raw| parse_stored_secret(&raw)),
        Err(_) => None,
    });
    let grok = match store.get_setting("xai_api_key".into()).await {
        Ok(value) => value.and_then(|raw| parse_stored_secret(&raw)),
        Err(_) => None,
    };
    keys.fill_if_empty(openai, anthropic, grok);
    keys
}

/// Yerel LMR / kernel hedefleri kota engeline takılmaz (continue yolu LMR'ye gider).
pub fn is_local_quota_target(agent: &str) -> bool {
    if is_kernel(agent) {
        return true;
    }
    matches!(
        agent.trim().to_ascii_lowercase().as_str(),
        "lmr" | "ollama" | "lounge-lmr" | "local"
    )
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

/// Atama öncesi: allow | block(reason, tool, percent).
/// Yapılandırılmamış araç satırı yoksa Allow (sahte kota satırı üretilmez).
/// Exhausted veya `percent >= limit` → Block.
pub fn evaluate_assignment(quotas: &[ToolQuota], agent: &str, limit_percent: f32) -> QuotaVerdict {
    if is_local_quota_target(agent) {
        return QuotaVerdict::Allow;
    }
    let matching: Vec<&ToolQuota> = quotas
        .iter()
        .filter(|row| quota_matches_agent(row, agent))
        .collect();
    if matching.is_empty() {
        return QuotaVerdict::Allow;
    }

    let limit = if limit_percent > 0.0 {
        limit_percent
    } else {
        LIMIT_POLICY_PERCENT
    };

    let mut worst: Option<(&ToolQuota, Option<f32>, bool)> = None;
    for row in matching {
        let exhausted = row.is_exhausted();
        let percent = row.percent;
        let over_limit = percent.map(|value| value >= limit).unwrap_or(false);
        if !exhausted && !over_limit {
            continue;
        }
        let score = percent.unwrap_or(if exhausted { 100.0 } else { 0.0 });
        let replace = worst
            .map(|(_, prev, _)| score >= prev.unwrap_or(0.0))
            .unwrap_or(true);
        if replace {
            worst = Some((row, percent, exhausted));
        }
    }

    match worst {
        Some((row, percent, exhausted)) => {
            let tool = if row.tool.is_empty() {
                row.id.clone()
            } else {
                row.tool.clone()
            };
            let reason = if exhausted {
                format!("{tool} kotası tükendi (exhausted); Limit Policy ≥ {limit:.0}%")
            } else {
                format!(
                    "{tool} kotası Limit Policy'yi aştı ({:.0}% ≥ {limit:.0}%)",
                    percent.unwrap_or(limit)
                )
            };
            QuotaVerdict::Block {
                reason,
                tool,
                percent,
            }
        }
        None => QuotaVerdict::Allow,
    }
}

pub fn quota_exhausted_for(quotas: &[ToolQuota], agent: &str) -> bool {
    quotas
        .iter()
        .filter(|row| quota_matches_agent(row, agent))
        .any(ToolQuota::is_exhausted)
}

pub fn quota_blocked_for(quotas: &[ToolQuota], agent: &str, limit_percent: f32) -> bool {
    evaluate_assignment(quotas, agent, limit_percent).is_blocked()
}

/// Lounge LMR (127.0.0.1:18790) ayakta mı? Host :11434 Ollama sayılmaz.
pub async fn lmr_runtime_up() -> bool {
    lmr_endpoint_up(&lounge_ollama_endpoint()).await
}

pub async fn lmr_endpoint_up(endpoint: &str) -> bool {
    if let Some((host, port)) = parse_http_host_port(endpoint) {
        if port == SYSTEM_OLLAMA_PORT {
            log::warn!(
                "lmr_endpoint_up: host Ollama portu ({SYSTEM_OLLAMA_PORT}) reddedildi; LMR={LOUNGE_OLLAMA_PORT}"
            );
            return false;
        }
        return tcp_ready(&host, port, LMR_HEALTH_WAIT).await;
    }
    tcp_ready("127.0.0.1", LOUNGE_OLLAMA_PORT, LMR_HEALTH_WAIT).await
}

fn parse_http_host_port(endpoint: &str) -> Option<(String, u16)> {
    let trimmed = endpoint.trim().trim_end_matches('/');
    let without = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))?;
    let authority = without.split('/').next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().ok()?),
        None => (authority.to_string(), 80),
    };
    Some((host, port))
}

/// Fallback ajan adını LMR'ye normalize et (host `ollama` PATH'i değil).
pub fn normalize_lmr_agent(agent: &str) -> String {
    let trimmed = agent.trim();
    if trimmed.is_empty() || is_local_quota_target(trimmed) {
        "lmr".into()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuotaAlertPayload {
    pub task_id: String,
    pub summary: String,
    pub from_agent: String,
    pub to_agent: String,
    pub kind: ApprovalKind,
    pub reason: String,
    pub tool: String,
    pub percent: Option<f32>,
    pub limit_percent: f32,
    pub message: String,
    pub continue_label: String,
    pub lmr_available: bool,
    pub lmr_endpoint: String,
    pub status: String,
}

impl QuotaAlertPayload {
    pub fn from_block(
        request: &ApprovalRequest,
        verdict: &QuotaVerdict,
        limit_percent: f32,
        lmr_available: bool,
    ) -> Self {
        let (tool, percent) = match verdict {
            QuotaVerdict::Block { tool, percent, .. } => (tool.clone(), *percent),
            QuotaVerdict::Allow => (String::new(), None),
        };
        let mut reason = request.reason.clone();
        if !lmr_available {
            reason = format!(
                "{reason} · Lounge LMR (127.0.0.1:{LOUNGE_OLLAMA_PORT}) ayakta değil; host Ollama kullanılmaz."
            );
        }
        Self {
            task_id: request.task_id.clone(),
            summary: request.summary.clone(),
            from_agent: request.from_agent.clone(),
            to_agent: request.to_agent.clone(),
            kind: request.kind.clone(),
            reason,
            tool,
            percent,
            limit_percent,
            message: QUOTA_ALERT_PROMPT.into(),
            continue_label: QUOTA_CONTINUE_LOCAL_LABEL.into(),
            lmr_available,
            lmr_endpoint: lounge_ollama_endpoint(),
            status: QUOTA_PENDING.into(),
        }
    }

    pub fn as_approval(&self) -> ApprovalRequest {
        ApprovalRequest {
            task_id: self.task_id.clone(),
            summary: self.summary.clone(),
            from_agent: self.from_agent.clone(),
            to_agent: self.to_agent.clone(),
            kind: self.kind.clone(),
            reason: self.reason.clone(),
            expires_at: None,
            timeout_secs: None,
        }
    }
}

pub fn is_quota_approval(kind: &ApprovalKind) -> bool {
    matches!(
        kind,
        ApprovalKind::QuotaLocalFallback | ApprovalKind::QuotaAbort
    )
}

pub fn quota_alert_envelope(
    request: &ApprovalRequest,
    verdict: &QuotaVerdict,
    limit_percent: f32,
    lmr_available: bool,
) -> serde_json::Value {
    serde_json::to_value(QuotaAlertPayload::from_block(
        request,
        verdict,
        limit_percent,
        lmr_available,
    ))
    .unwrap_or_else(|_| serde_json::json!({ "task_id": request.task_id }))
}

/// Kota engeli için ApprovalRequest üretir (AskThenLocal varsayılan yolu).
pub fn quota_approval_request(
    task_id: &str,
    summary: &str,
    from_agent: &str,
    blocked_agent: &str,
    verdict: &QuotaVerdict,
    local_fallback_agent: &str,
) -> ApprovalRequest {
    let reason = verdict.reason().unwrap_or("kota limiti aşıldı").to_string();
    ApprovalRequest {
        task_id: task_id.into(),
        summary: summary.into(),
        from_agent: from_agent.into(),
        to_agent: normalize_lmr_agent(local_fallback_agent),
        kind: ApprovalKind::QuotaLocalFallback,
        reason: format!("{reason} · hedef={blocked_agent} → LMR"),
        expires_at: None,
        timeout_secs: None,
    }
}

fn emit_quota_state(app: &AppHandle, state: &QuotaState) {
    if state.amber_alert {
        log::warn!(
            "Amber Alert: {} kota ≥ {AMBER_THRESHOLD}% ({})",
            state.amber_tools.len(),
            state.amber_tools.join(", ")
        );
    }
    if let Some(window) = app.get_webview_window("main") {
        if let Err(err) = window.emit(QUOTA_EVENT, state) {
            log::debug!("{QUOTA_EVENT} window emit: {err}");
        }
    }
    if let Err(err) = app.emit(QUOTA_EVENT, state) {
        log::debug!("{QUOTA_EVENT} app emit: {err}");
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

pub fn parse_stored_secret(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        match value {
            Value::String(text) => {
                let text = text.trim().to_string();
                return (!text.is_empty()).then_some(text);
            }
            Value::Object(map) => {
                return json_secret(
                    &map,
                    &["key", "token", "value", "openai", "anthropic", "claude"],
                );
            }
            _ => return None,
        }
    }
    Some(trimmed.to_string())
}

fn json_secret(map: &serde_json::Map<String, Value>, names: &[&str]) -> Option<String> {
    for name in names {
        if let Some(Value::String(text)) = map.get(*name) {
            let text = text.trim().to_string();
            if !text.is_empty() {
                return Some(text);
            }
        }
    }
    None
}

pub fn is_lmr_ollama_exe(exe: &str) -> bool {
    let path = exe.replace('\\', "/").to_lowercase();
    (path.contains("/data/lmr/") && path.contains("ollama")) || path.contains("/lmr/ollama")
}

pub fn looks_like_ollama(name: &str, exe: &str) -> bool {
    let name = name.to_lowercase();
    let exe = exe.replace('\\', "/").to_lowercase();
    name.contains("ollama")
        || exe
            .rsplit('/')
            .next()
            .is_some_and(|file| file.contains("ollama"))
}

fn environ_has_lmr_host(proc: &Process) -> bool {
    proc.environ().iter().any(|entry| {
        let value = entry.to_string_lossy();
        value.starts_with("OLLAMA_HOST=") && value.contains(LMR_PORT_MARK)
    })
}

fn is_lmr_root(proc: &Process) -> bool {
    let name = proc.name().to_string_lossy();
    let exe = proc
        .exe()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    if !looks_like_ollama(&name, &exe) {
        return false;
    }
    is_lmr_ollama_exe(&exe) || environ_has_lmr_host(proc)
}

pub fn lmr_sysinfo_usage() -> LmrProcessUsage {
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::everything(),
    );
    lmr_usage_from_system(&sys, is_apple_silicon())
}

pub fn lmr_usage_from_system(sys: &System, apple_silicon: bool) -> LmrProcessUsage {
    let mut roots: Vec<Pid> = Vec::new();
    for (pid, proc) in sys.processes() {
        if is_lmr_root(proc) {
            roots.push(*pid);
        }
    }
    if roots.is_empty() {
        return LmrProcessUsage::default();
    }

    let mut include: HashSet<Pid> = roots.iter().copied().collect();
    let mut changed = true;
    while changed {
        changed = false;
        for (pid, proc) in sys.processes() {
            if include.contains(pid) {
                continue;
            }
            if let Some(parent) = proc.parent() {
                if include.contains(&parent) {
                    include.insert(*pid);
                    changed = true;
                }
            }
        }
    }

    let ram_bytes = include
        .iter()
        .filter_map(|pid| sys.process(*pid).map(Process::memory))
        .sum();
    let vram_bytes = if apple_silicon {
        ram_bytes
    } else {
        include
            .iter()
            .filter_map(|pid| {
                let proc = sys.process(*pid)?;
                let name = proc.name().to_string_lossy().to_lowercase();
                name.contains("runner").then_some(proc.memory())
            })
            .sum()
    };
    LmrProcessUsage {
        ram_bytes,
        vram_bytes,
        processes: include.len(),
    }
}

fn subscription_rows_with_live(
    tool: &DiscoveredTool,
    live: Option<crate::services::subscription_usage::LocalPlanUsage>,
) -> Vec<ToolQuota> {
    let running = tool
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("çalışıyor"));
    if let Some(usage) = live {
        let rows = usage.into_quotas(tool, running);
        if !rows.is_empty() {
            return rows;
        }
    }
    subscription_rows(tool)
}

fn subscription_rows(tool: &DiscoveredTool) -> Vec<ToolQuota> {
    let running = tool
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("çalışıyor"));
    if let Some(usage) = local_subscription_usage(&tool.source) {
        let rows = usage.into_quotas(tool, running);
        if !rows.is_empty() {
            return rows;
        }
    }
    let usage = (tool.source == "claude_cli")
        .then(claude_cli_local_usage)
        .flatten();
    if let Some((used, limit)) = usage {
        let percent = if limit > 0 {
            Some(((used as f32 / limit as f32) * 100.0).clamp(0.0, 999.0))
        } else {
            None
        };
        return vec![ToolQuota {
            id: tool.id.clone(),
            tool: tool.name.clone(),
            unit: "subscription".into(),
            used: format!("{used} / {limit}"),
            remaining: format!("{}", limit.saturating_sub(used)),
            reset: "—".into(),
            percent,
            tone: tone_for_percent(percent, if running { "live" } else { "ok" }),
            label: percent
                .map(|value| format!("{value:.0}%"))
                .unwrap_or_else(|| "abonelik".into()),
            source: tool.origin_path.clone().unwrap_or_default(),
            exhausted: percent.map(|value| value >= 100.0).unwrap_or(false),
            ..ToolQuota::default()
        }
        .with_mode("subscription", tool.host_id.as_deref())];
    }
    vec![ToolQuota {
        id: tool.id.clone(),
        tool: tool.name.clone(),
        unit: "subscription".into(),
        used: if running { "çalışıyor" } else { "kurulu" }.into(),
        remaining: "abonelik".into(),
        reset: "—".into(),
        percent: None,
        tone: if running { "live" } else { "ok" }.into(),
        label: if running { "live" } else { "abonelik" }.into(),
        source: tool.origin_path.clone().unwrap_or_default(),
        exhausted: false,
        ..ToolQuota::default()
    }
    .with_mode("subscription", tool.host_id.as_deref())]
}

fn plugin_quota(tool: &DiscoveredTool) -> ToolQuota {
    let hosts = tool.hosts();
    let labels = crate::models::format_host_labels(&hosts);
    let host = hosts
        .first()
        .cloned()
        .unwrap_or_else(|| tool.source.clone());
    ToolQuota {
        id: tool.id.clone(),
        tool: tool.name.clone(),
        unit: "plugin".into(),
        used: if hosts.len() == 1 {
            "host üzerinden".into()
        } else {
            format!("{} host", hosts.len())
        },
        remaining: labels,
        reset: "HOST".into(),
        percent: None,
        tone: "ok".into(),
        label: "plugin".into(),
        source: tool.origin_path.clone().unwrap_or_default(),
        exhausted: false,
        ..ToolQuota::default()
    }
    .with_mode("plugin", Some(&host))
}

pub fn claude_cli_local_usage() -> Option<(u64, u64)> {
    for dir in claude_cli_dirs() {
        for name in ["stats-cache.json", "stats.json", "usage.json"] {
            let path = dir.join(name);
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            if let Some(pair) = parse_local_usage_pair(&value) {
                return Some(pair);
            }
        }
    }
    None
}

fn claude_cli_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
        dirs.push(dir);
    }
    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
    {
        dirs.push(home.join(".claude"));
        dirs.push(home.join(".config/claude"));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        dirs.push(xdg.join("claude"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        dirs.push(appdata.join("Claude"));
        dirs.push(appdata.join("claude"));
    }
    dirs
}

pub fn parse_local_usage_pair(value: &Value) -> Option<(u64, u64)> {
    let used = value
        .get("used")
        .or_else(|| value.get("tokensUsed"))
        .or_else(|| value.get("tokens_used"))
        .or_else(|| value.pointer("/usage/used"))
        .and_then(Value::as_u64)?;
    let limit = value
        .get("limit")
        .or_else(|| value.get("tokensLimit"))
        .or_else(|| value.get("tokens_limit"))
        .or_else(|| value.pointer("/usage/limit"))
        .and_then(Value::as_u64)
        .filter(|value| *value > 0)?;
    Some((used, limit))
}

pub fn has_api_key(key: Option<&str>) -> bool {
    key.map(str::trim).is_some_and(|value| !value.is_empty())
}

async fn probe_ollama(endpoint: &str) -> ToolQuota {
    let profile = device_profile();
    let budget = env_f32("LOUNGE_LMR_RAM_BUDGET_GB", profile.usable_budget_gb);
    let usage = tokio::task::spawn_blocking(lmr_sysinfo_usage)
        .await
        .unwrap_or_else(|_| LmrProcessUsage::default());

    let url = format!("{}/api/ps", endpoint.trim_end_matches('/'));
    let (model_label, api_up) = match http_json(&url, &[]).await {
        Ok(payload) => {
            let models = payload
                .get("models")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let name = models
                .iter()
                .filter_map(|row| row.get("name").and_then(Value::as_str))
                .next()
                .unwrap_or("LMR");
            (name.to_string(), true)
        }
        Err(_) => ("LMR".into(), false),
    };

    if usage.processes == 0 && !api_up {
        return ToolQuota {
            id: "lmr".into(),
            tool: "LMR".into(),
            unit: "ram/vram".into(),
            used: "down".into(),
            remaining: "—".into(),
            reset: "LOCAL".into(),
            percent: None,
            tone: "warn".into(),
            label: "sysinfo: ollama yok".into(),
            source: "sysinfo".into(),
            exhausted: false,
            ..ToolQuota::default()
        }
        .with_mode("local", None);
    }

    let percent = ram_percent(usage.ram_bytes, budget);
    let ram_gb = usage.ram_bytes as f32 / (1024.0 * 1024.0 * 1024.0);
    let vram_gb = usage.vram_bytes as f32 / (1024.0 * 1024.0 * 1024.0);
    let used = if is_apple_silicon() {
        format!("{ram_gb:.1} / {budget:.1} GB RAM/VRAM")
    } else {
        format!("{ram_gb:.1} GB RAM · {vram_gb:.1} GB VRAM / {budget:.1} GB")
    };
    ToolQuota {
        id: "lmr".into(),
        tool: format!("LMR · {model_label}"),
        unit: "ram/vram".into(),
        used,
        remaining: format!("{:.1} GB", (budget - ram_gb).max(0.0)),
        reset: "LOCAL".into(),
        percent,
        tone: tone_for_percent(percent, "local"),
        label: percent
            .map(|value| format!("{:.0}% ram", value))
            .unwrap_or_else(|| "ok LOCAL".into()),
        source: "sysinfo".into(),
        exhausted: false,
        ..ToolQuota::default()
    }
    .with_mode("local", None)
}

async fn probe_nats(monitor: &str) -> ToolQuota {
    let url = format!("{}/varz", monitor.trim_end_matches('/'));
    let varz = http_json(&url, &[]).await.ok();
    let tcp = tcp_ready(
        DEFAULT_NATS_HOST,
        DEFAULT_NATS_PORT,
        Duration::from_millis(400),
    )
    .await;
    nats_quota(varz.as_ref(), tcp, &url)
}

pub fn nats_quota(varz: Option<&Value>, tcp_up: bool, source: &str) -> ToolQuota {
    if let Some(payload) = varz {
        let connections = payload
            .get("connections")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let in_msgs = payload.get("in_msgs").and_then(Value::as_u64).unwrap_or(0);
        return ToolQuota {
            id: "nats".into(),
            tool: "NATS broker".into(),
            unit: "conn".into(),
            used: format!("{connections} conn · {in_msgs} in_msgs"),
            remaining: "local".into(),
            reset: "LOCAL".into(),
            percent: None,
            tone: "ok".into(),
            label: "ok LOCAL".into(),
            source: source.into(),
            exhausted: false,
            ..ToolQuota::default()
        }
        .with_mode("local", None);
    }
    if tcp_up {
        return ToolQuota {
            id: "nats".into(),
            tool: "NATS broker".into(),
            unit: "conn".into(),
            used: "tcp live".into(),
            remaining: "local".into(),
            reset: "LOCAL".into(),
            percent: None,
            tone: "ok".into(),
            label: "ok LOCAL".into(),
            source: source.into(),
            exhausted: false,
            ..ToolQuota::default()
        }
        .with_mode("local", None);
    }
    ToolQuota {
        id: "nats".into(),
        tool: "NATS broker".into(),
        unit: "conn".into(),
        used: "down".into(),
        remaining: "—".into(),
        reset: "LOCAL".into(),
        percent: None,
        tone: "warn".into(),
        label: "down".into(),
        source: source.into(),
        exhausted: false,
        ..ToolQuota::default()
    }
    .with_mode("local", None)
}

fn probe_memory(memory: &MemoryBridge) -> ToolQuota {
    let health = memory.diagnose();
    ToolQuota {
        id: "cbm".into(),
        tool: "codebase-memory-mcp".into(),
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
        ..ToolQuota::default()
    }
    .with_mode("local", None)
}

async fn probe_openai(key: Option<&str>) -> Option<ToolQuota> {
    if !has_api_key(key) {
        return None;
    }
    let key = key.unwrap_or_default();
    let start = midnight_unix();
    let url = format!(
        "https://api.openai.com/v1/organization/usage/completions?start_time={start}&bucket_width=1d"
    );
    let budget = env_f32("OPENAI_TOKEN_BUDGET", 0.0);
    let auth = [
        ("Authorization", format!("Bearer {key}")),
        ("OpenAI-Beta", "usage=v1".into()),
    ];
    Some(match http_json(&url, &auth).await {
        Ok(payload) => usage_row(
            "openai",
            "OpenAI API",
            parse_openai_tokens(&payload),
            budget,
            url,
        ),
        Err(usage_err) => match http_json("https://api.openai.com/v1/models", &auth).await {
            Ok(_) => key_present_no_usage("openai", "OpenAI API", usage_err),
            Err(err) => key_present_no_usage("openai", "OpenAI API", err),
        },
    })
}

async fn probe_anthropic(key: Option<&str>) -> Option<ToolQuota> {
    if !has_api_key(key) {
        return None;
    }
    let key = key.unwrap_or_default();
    let start = now_rfc3339();
    let day_start = format!("{}T00:00:00Z", &start[..10.min(start.len())]);
    let url = format!(
        "https://api.anthropic.com/v1/organizations/usage_report/messages?starting_at={day_start}&bucket_width=1d"
    );
    let budget = env_f32("ANTHROPIC_TOKEN_BUDGET", 0.0);
    let headers = [
        ("x-api-key", key.to_string()),
        ("anthropic-version", "2023-06-01".into()),
    ];
    Some(match http_json(&url, &headers).await {
        Ok(payload) => usage_row(
            "anthropic",
            "Anthropic API",
            parse_anthropic_tokens(&payload),
            budget,
            url,
        ),
        Err(usage_err) => match http_json("https://api.anthropic.com/v1/models", &headers).await {
            Ok(_) => key_present_no_usage("anthropic", "Anthropic API", usage_err),
            Err(err) => key_present_no_usage("anthropic", "Anthropic API", err),
        },
    })
}

async fn probe_grok(key: Option<&str>) -> Option<ToolQuota> {
    if !has_api_key(key) {
        return None;
    }
    let key = key.unwrap_or_default();
    let url = "https://api.x.ai/v1/api-key";
    let budget = env_f32("XAI_CREDIT_BUDGET", 0.0);
    Some(
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
                        tool: "Grok API".into(),
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
                        ..ToolQuota::default()
                    }
                    .with_mode("api", None)
                }
                None => key_present_no_usage("xai", "Grok API", "usage alanı yok".into()),
            },
            Err(err) => key_present_no_usage("xai", "Grok API", err),
        },
    )
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
        ..ToolQuota::default()
    }
    .with_mode("api", None)
}

fn key_present_no_usage(id: &str, tool: &str, detail: String) -> ToolQuota {
    ToolQuota {
        id: id.into(),
        tool: tool.into(),
        unit: "api".into(),
        used: "key var".into(),
        remaining: "usage yok".into(),
        reset: "—".into(),
        percent: None,
        tone: "ok".into(),
        label: short_err(&detail),
        source: "provider".into(),
        exhausted: false,
        ..ToolQuota::default()
    }
    .with_mode("api", None)
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
            unit: "ram".into(),
            used: "6.4 / 8.0 GB".into(),
            remaining: "1.6 GB".into(),
            reset: "LOCAL".into(),
            percent: Some(percent),
            tone: tone_for_percent(Some(percent), "local"),
            label: "80% ram".into(),
            source: "test".into(),
            exhausted: false,
            ..ToolQuota::default()
        }
        .with_mode("local", None)]);
        assert!(state.amber_alert);
        assert_eq!(state.amber_tools, vec!["lmr"]);
    }

    #[test]
    fn seventy_nine_is_not_amber() {
        let state = QuotaState::from_quotas(vec![ToolQuota {
            id: "openai".into(),
            tool: "OpenAI API".into(),
            unit: "tokens".into(),
            used: "79".into(),
            remaining: "21".into(),
            reset: "24h".into(),
            percent: Some(79.0),
            tone: "ok".into(),
            label: "79%".into(),
            source: "test".into(),
            exhausted: false,
            ..ToolQuota::default()
        }
        .with_mode("api", None)]);
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

    #[test]
    fn lmr_exe_matches_isolated_binary_not_host() {
        assert!(is_lmr_ollama_exe(
            "/Users/me/Agent-Lounge-OS/data/lmr/ollama"
        ));
        assert!(is_lmr_ollama_exe(r"C:\Agent-Lounge-OS\data\lmr\ollama.exe"));
        assert!(!is_lmr_ollama_exe("/opt/homebrew/bin/ollama"));
        assert!(looks_like_ollama("ollama", "/opt/homebrew/bin/ollama"));
        assert!(!looks_like_ollama(
            "nats-server",
            "/opt/homebrew/bin/nats-server"
        ));
    }

    #[test]
    fn stored_secrets_accept_json_or_plain() {
        assert_eq!(parse_stored_secret(" sk-test "), Some("sk-test".into()));
        assert_eq!(parse_stored_secret("\"sk-json\""), Some("sk-json".into()));
        assert_eq!(parse_stored_secret("null"), None);
        assert_eq!(parse_stored_secret(""), None);
        let mut keys = ApiKeys::default();
        keys.merge_json(r#"{"openai":"sk-oai","claude":"sk-ant"}"#);
        assert_eq!(keys.openai.as_deref(), Some("sk-oai"));
        assert_eq!(keys.anthropic.as_deref(), Some("sk-ant"));
    }

    #[test]
    fn quota_event_is_update() {
        assert_eq!(QUOTA_EVENT, "quota-update");
    }

    #[test]
    fn nats_quota_tcp_without_varz_is_not_down() {
        let row = nats_quota(None, true, "http://127.0.0.1:8222/varz");
        assert_eq!(row.label, "ok LOCAL");
        assert_eq!(row.used, "tcp live");
        assert_eq!(row.tone, "ok");
    }

    #[test]
    fn nats_quota_down_when_tcp_and_varz_fail() {
        let row = nats_quota(None, false, "http://127.0.0.1:8222/varz");
        assert_eq!(row.label, "down");
        assert_eq!(row.used, "down");
        assert_eq!(row.tone, "warn");
    }

    #[test]
    fn nats_quota_prefers_varz_metrics() {
        let payload = json!({ "connections": 3, "in_msgs": 40 });
        let row = nats_quota(Some(&payload), false, "http://127.0.0.1:8222/varz");
        assert_eq!(row.used, "3 conn · 40 in_msgs");
        assert_eq!(row.label, "ok LOCAL");
    }

    #[test]
    fn missing_api_key_skips_row() {
        assert!(!has_api_key(None));
        assert!(!has_api_key(Some("")));
        assert!(!has_api_key(Some("  ")));
        assert!(has_api_key(Some("sk-test")));
    }

    #[test]
    fn parses_claude_cli_local_usage_without_inventing() {
        let payload = json!({ "used": 12, "limit": 100 });
        assert_eq!(parse_local_usage_pair(&payload), Some((12, 100)));
        assert_eq!(parse_local_usage_pair(&json!({ "used": 12 })), None);
    }

    fn sub_row(
        id: &str,
        tool: &str,
        host: &str,
        percent: Option<f32>,
        exhausted: bool,
    ) -> ToolQuota {
        ToolQuota {
            id: id.into(),
            tool: tool.into(),
            percent,
            exhausted,
            ..ToolQuota::default()
        }
        .with_mode("subscription", Some(host))
    }

    #[test]
    fn merges_claude_desktop_and_cli_into_one_account() {
        let mut quotas = vec![
            sub_row(
                "app:claude_desktop",
                "Claude Desktop",
                "claude_desktop",
                Some(40.0),
                false,
            ),
            sub_row(
                "app:claude_cli",
                "Claude CLI",
                "claude_cli",
                Some(82.0),
                false,
            ),
            sub_row("app:cursor", "Cursor", "cursor", Some(15.0), false),
        ];
        merge_claude_subscription_rows(&mut quotas);
        let claude: Vec<_> = quotas
            .iter()
            .filter(|row| row.id == "app:claude" || row.tool == "Claude")
            .collect();
        assert_eq!(claude.len(), 1, "expected a single Claude account row");
        assert_eq!(claude[0].id, "app:claude");
        assert_eq!(claude[0].tool, "Claude");
        assert_eq!(claude[0].host_id.as_deref(), Some("claude"));
        // Prefer live usage from either source (higher/known percent wins via sort).
        assert!(claude[0].percent.is_some());
        assert_eq!(quotas.iter().filter(|r| r.id == "app:cursor").count(), 1);
    }

    #[test]
    fn evaluate_allows_under_limit_policy() {
        let quotas = vec![sub_row(
            "app:claude_desktop",
            "Claude",
            "claude_desktop",
            Some(89.0),
            false,
        )];
        assert_eq!(
            evaluate_assignment(&quotas, "claude", LIMIT_POLICY_PERCENT),
            QuotaVerdict::Allow
        );
    }

    #[test]
    fn evaluate_blocks_at_ninety_percent() {
        let quotas = vec![sub_row(
            "app:claude_desktop",
            "Claude",
            "claude_desktop",
            Some(90.0),
            false,
        )];
        let verdict = evaluate_assignment(&quotas, "claude", LIMIT_POLICY_PERCENT);
        assert!(verdict.is_blocked());
        assert_eq!(verdict.tool(), Some("Claude"));
        assert_eq!(verdict.percent(), Some(90.0));
    }

    #[test]
    fn evaluate_blocks_when_exhausted() {
        let quotas = vec![sub_row("app:cursor", "Cursor", "cursor", Some(100.0), true)];
        assert!(evaluate_assignment(&quotas, "cursor", LIMIT_POLICY_PERCENT).is_blocked());
    }

    #[test]
    fn evaluate_allows_when_no_configured_rows() {
        assert_eq!(
            evaluate_assignment(&[], "claude", LIMIT_POLICY_PERCENT),
            QuotaVerdict::Allow
        );
        let anthropic_only = vec![ToolQuota {
            id: "anthropic".into(),
            tool: "Anthropic API".into(),
            percent: Some(99.0),
            exhausted: false,
            ..ToolQuota::default()
        }
        .with_mode("api", None)];
        // Claude abonelik satırı yok → sahte blok yok
        assert_eq!(
            evaluate_assignment(&anthropic_only, "claude", LIMIT_POLICY_PERCENT),
            QuotaVerdict::Allow
        );
    }

    #[test]
    fn evaluate_allows_local_lmr_target() {
        let quotas = vec![sub_row("app:cursor", "Cursor", "cursor", Some(95.0), false)];
        assert_eq!(
            evaluate_assignment(&quotas, "lmr", LIMIT_POLICY_PERCENT),
            QuotaVerdict::Allow
        );
        assert_eq!(
            evaluate_assignment(&quotas, "ollama", LIMIT_POLICY_PERCENT),
            QuotaVerdict::Allow
        );
    }

    #[test]
    fn normalize_lmr_agent_forces_lounge_runtime() {
        assert_eq!(normalize_lmr_agent("ollama"), "lmr");
        assert_eq!(normalize_lmr_agent("lmr"), "lmr");
        assert_eq!(normalize_lmr_agent(""), "lmr");
        assert_eq!(normalize_lmr_agent("cursor"), "cursor");
    }

    #[test]
    fn continue_path_endpoint_is_lounge_lmr_not_host() {
        let endpoint = lounge_ollama_endpoint();
        assert!(
            endpoint.contains(&LOUNGE_OLLAMA_PORT.to_string()),
            "LMR endpoint beklenirdi: {endpoint}"
        );
        assert!(
            !endpoint.contains(&SYSTEM_OLLAMA_PORT.to_string()),
            "host Ollama portu olmamalı: {endpoint}"
        );
        let parsed = parse_http_host_port(&endpoint).expect("endpoint parse");
        assert_eq!(parsed.1, LOUNGE_OLLAMA_PORT);
        assert_ne!(parsed.1, SYSTEM_OLLAMA_PORT);
    }

    #[test]
    fn quota_alert_payload_mentions_lmr_down() {
        let request = ApprovalRequest {
            task_id: "t1".into(),
            summary: "gen".into(),
            from_agent: "cursor".into(),
            to_agent: "lmr".into(),
            kind: ApprovalKind::QuotaLocalFallback,
            reason: "Cursor kotası".into(),
            expires_at: None,
            timeout_secs: None,
        };
        let verdict = QuotaVerdict::Block {
            reason: "Cursor kotası Limit Policy'yi aştı (92% ≥ 90%)".into(),
            tool: "Cursor".into(),
            percent: Some(92.0),
        };
        let alert = QuotaAlertPayload::from_block(&request, &verdict, 90.0, false);
        assert_eq!(alert.continue_label, QUOTA_CONTINUE_LOCAL_LABEL);
        assert_eq!(alert.continue_label, "Yerel Model (Ollama) ile devam et");
        assert!(!alert.lmr_available);
        assert!(alert.reason.contains("ayakta değil"));
        assert!(alert.lmr_endpoint.contains("18790"));
        assert!(!alert.lmr_endpoint.contains("11434"));
        assert_eq!(crate::models::ALERT_QUOTA, "lounge.alert.quota");
    }
}
