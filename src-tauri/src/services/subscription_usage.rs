//! Abonelik kullanımı.
//! Claude: Claude Code OAuth `GET /api/oauth/usage` (5s / 7g + resets_at).
//! Yerel `plan-usage-history.json` yalnızca canlı çağrı yoksa, son örneği uydurmadan.
//! Cursor + Grok Bot: IDE'nin `state.vscdb` accessToken'ı ile `api2.cursor.sh`
//! (hesap sayfası cookie kazıması yok). Antigravity: Cloud Code quota summary,
//! yoksa yerel userStatus proto `remainingFraction`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration as StdDuration, Instant};

use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;

use crate::models::{DiscoveredTool, ToolQuota, AMBER_THRESHOLD};

const HTTP_TIMEOUT: StdDuration = StdDuration::from_secs(8);
const CURSOR_API: &str = "https://api2.cursor.sh";
const CLOUD_CODE: &str = "https://cloudcode-pa.googleapis.com";
const CLAUDE_USAGE_API: &str = "https://api.anthropic.com/api/oauth/usage";
const CLAUDE_USAGE_TTL: StdDuration = StdDuration::from_secs(300);
const CLAUDE_CODE_UA: &str = "claude-code/2.1.278";

struct CachedPlanUsage {
    stored_at: Instant,
    usage: LocalPlanUsage,
}

static CLAUDE_LIVE_CACHE: Mutex<Option<CachedPlanUsage>> = Mutex::new(None);

const FIVE_HOUR_PERIOD: Duration = Duration::hours(5);
const SEVEN_DAY_PERIOD: Duration = Duration::days(7);

#[derive(Debug, Clone, PartialEq)]
pub struct QuotaWindow {
    pub suffix: String,
    pub label: String,
    pub used_percent: Option<f32>,
    pub reset_at: Option<chrono::DateTime<Utc>>,
}

impl QuotaWindow {
    fn short_tag(&self) -> &'static str {
        match self.suffix.as_str() {
            "5h" => "5s",
            "7d" => "7g",
            "3p-5h" => "3p 5s",
            "3p-7d" => "3p 7g",
            other if other.contains("5h") => "5s",
            other if other.contains("7d") || other.contains("week") => "7g",
            _ => "",
        }
    }

    fn reset_text(&self) -> String {
        if let Some(at) = self.reset_at {
            return remaining_until(at, Utc::now());
        }
        if self.suffix.contains("5h") {
            "kullanınca".into()
        } else {
            "—".into()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LocalPlanUsage {
    pub plan: Option<String>,
    pub status: Option<String>,
    pub five_hour_percent: Option<u32>,
    pub seven_day_percent: Option<u32>,
    pub five_hour_reset: Option<chrono::DateTime<Utc>>,
    pub seven_day_reset: Option<chrono::DateTime<Utc>>,
    pub percent: Option<f32>,
    pub spend_used: Option<f32>,
    pub spend_limit: Option<f32>,
    pub remaining_spend: Option<f32>,
    pub reset: Option<String>,
    pub reset_at: Option<chrono::DateTime<Utc>>,
    pub windows: Vec<QuotaWindow>,
    pub source: String,
}

impl LocalPlanUsage {
    fn finalize(mut self) -> Self {
        if self.windows.is_empty() {
            if let Some(fh) = self.five_hour_percent {
                self.windows.push(QuotaWindow {
                    suffix: "5h".into(),
                    label: "5 saat".into(),
                    used_percent: Some(fh as f32),
                    reset_at: self.five_hour_reset,
                });
            }
            if let Some(sd) = self.seven_day_percent {
                self.windows.push(QuotaWindow {
                    suffix: "7d".into(),
                    label: "7 gün".into(),
                    used_percent: Some(sd as f32),
                    reset_at: self.seven_day_reset,
                });
            }
        }
        if self.percent.is_none() {
            self.percent = self
                .windows
                .iter()
                .filter_map(|window| window.used_percent)
                .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .or_else(|| {
                    [self.five_hour_percent, self.seven_day_percent]
                        .into_iter()
                        .flatten()
                        .max()
                        .map(|value| value as f32)
                });
        }
        if self.reset.is_none() {
            self.reset = self
                .reset_at
                .or_else(|| {
                    self.windows
                        .iter()
                        .filter_map(|window| window.reset_at)
                        .min()
                })
                .map(|at| remaining_until(at, Utc::now()));
        }
        self
    }

    pub fn used_label(&self, running: bool) -> String {
        let parts = self.window_parts(|window, used| format!("{used:.0}% {}", window.short_tag()));
        if !parts.is_empty() {
            return parts.join(" · ");
        }
        match (self.percent, self.spend_used, self.spend_limit) {
            (Some(percent), Some(used), Some(limit)) if limit > 0.0 => {
                return format!("{percent:.0}% · ${used:.0} / ${limit:.0}");
            }
            (Some(percent), _, _) => return format!("{percent:.0}%"),
            _ => {}
        }
        match (self.plan.as_deref(), self.status.as_deref()) {
            (Some(plan), Some(status)) => format!("{plan} · {status}"),
            (Some(plan), None) => plan.to_string(),
            (None, Some(status)) => status.to_string(),
            _ if running => "çalışıyor".into(),
            _ => "kurulu".into(),
        }
    }

    pub fn remaining_label(&self) -> String {
        if let Some(remaining) = self.remaining_spend {
            return format!("${remaining:.0}");
        }
        let parts = self.window_parts(|window, used| {
            format!("{:.0}% {}", (100.0 - used).max(0.0), window.short_tag())
        });
        if !parts.is_empty() {
            return parts.join(" · ");
        }
        if let Some(percent) = self.percent {
            return format!("{:.0}% plan", (100.0 - percent).max(0.0));
        }
        if self.plan.is_some() {
            return "kota yok".into();
        }
        "abonelik".into()
    }

    pub fn reset_label(&self) -> String {
        let parts: Vec<String> = self
            .windows
            .iter()
            .filter(|window| window.used_percent.is_some())
            .map(|window| window.reset_text())
            .collect();
        if !parts.is_empty() {
            return parts.join(" · ");
        }
        if let Some(reset) = &self.reset {
            return reset.clone();
        }
        if let Some(at) = self.reset_at {
            return remaining_until(at, Utc::now());
        }
        "—".into()
    }

    fn window_parts(&self, format: impl Fn(&QuotaWindow, f32) -> String) -> Vec<String> {
        self.windows
            .iter()
            .filter_map(|window| window.used_percent.map(|used| format(window, used)))
            .collect()
    }

    pub fn into_quotas(self, tool: &DiscoveredTool, running: bool) -> Vec<ToolQuota> {
        vec![self.into_quota(tool, running)]
    }

    pub fn state_label(&self, running: bool) -> String {
        if let Some(percent) = self.percent {
            return format!("{percent:.0}%");
        }
        if running {
            return "live".into();
        }
        self.plan
            .clone()
            .or_else(|| self.status.clone())
            .unwrap_or_else(|| "abonelik".into())
    }

    pub fn into_quota(self, tool: &DiscoveredTool, running: bool) -> ToolQuota {
        let usage = self.finalize();
        let percent = usage.percent;
        let exhausted = percent.map(|value| value >= 100.0).unwrap_or(false);
        let fallback = if usage
            .status
            .as_deref()
            .is_some_and(|status| status.contains("past") || status.contains("cancel"))
        {
            "warn"
        } else if running {
            "live"
        } else {
            "ok"
        };
        ToolQuota {
            id: tool.id.clone(),
            tool: tool.name.clone(),
            unit: "subscription".into(),
            used: usage.used_label(running),
            remaining: usage.remaining_label(),
            reset: usage.reset_label(),
            percent,
            tone: if exhausted {
                "amber".into()
            } else {
                match percent {
                    Some(value) if value >= AMBER_THRESHOLD => "amber".into(),
                    Some(_) => "ok".into(),
                    None => fallback.into(),
                }
            },
            label: usage.state_label(running),
            source: if usage.source.is_empty() {
                tool.origin_path.clone().unwrap_or_default()
            } else {
                usage.source.clone()
            },
            exhausted,
            ..ToolQuota::default()
        }
        .with_mode("subscription", tool.host_id.as_deref())
    }
}

pub fn local_subscription_usage(source: &str) -> Option<LocalPlanUsage> {
    match source {
        "claude_desktop" | "claude_cli" => claude_plan_usage(),
        "cursor" => cursor_membership(),
        "antigravity" => antigravity_plan(),
        "grok_bot" => grok_session(),
        _ => None,
    }
}

pub async fn live_subscription_usage(source: &str) -> Option<LocalPlanUsage> {
    match source {
        "cursor" => cursor_live_usage().await,
        "grok_bot" => grok_live_usage().await,
        "antigravity" => antigravity_live_usage().await,
        "claude_desktop" | "claude_cli" => claude_live_usage().await,
        other => {
            let source = other.to_string();
            tokio::task::spawn_blocking(move || local_subscription_usage(&source))
                .await
                .ok()
                .flatten()
        }
    }
}

async fn cursor_live_usage() -> Option<LocalPlanUsage> {
    let membership = tokio::task::spawn_blocking(cursor_membership)
        .await
        .ok()
        .flatten();
    let token = tokio::task::spawn_blocking(cursor_access_token)
        .await
        .ok()
        .flatten();
    let mut live = None;
    if let Some(token) = token {
        live = cursor_rpc(
            "/aiserver.v1.DashboardService/GetCurrentPeriodUsage",
            &token,
        )
        .await
        .ok()
        .and_then(|payload| parse_cursor_period_usage(&payload));
        if live.is_none() {
            live = cursor_get("/auth/usage", &token)
                .await
                .ok()
                .and_then(|payload| parse_cursor_auth_usage(&payload));
        }
    }
    merge_plan(live, membership)
}

async fn grok_live_usage() -> Option<LocalPlanUsage> {
    let session = tokio::task::spawn_blocking(grok_session)
        .await
        .ok()
        .flatten();
    let token = tokio::task::spawn_blocking(cursor_access_token)
        .await
        .ok()
        .flatten();
    let live = if let Some(token) = token {
        cursor_rpc("/aiserver.v1.DashboardService/GetSandUsageStatus", &token)
            .await
            .ok()
            .and_then(|payload| parse_sand_usage(&payload))
    } else {
        None
    };
    merge_plan(live, session)
}

async fn claude_live_usage() -> Option<LocalPlanUsage> {
    if let Some(cached) = claude_cache_get(false) {
        return Some(cached);
    }
    let local = tokio::task::spawn_blocking(claude_plan_usage)
        .await
        .ok()
        .flatten();
    let token = tokio::task::spawn_blocking(claude_oauth_token)
        .await
        .ok()
        .flatten();
    let live = if let Some(token) = token {
        match http_json(
            "GET",
            CLAUDE_USAGE_API,
            &token,
            None,
            &[
                ("anthropic-beta", "oauth-2025-04-20"),
                ("User-Agent", CLAUDE_CODE_UA),
            ],
        )
        .await
        {
            Ok(payload) => parse_claude_oauth_usage(&payload),
            Err(err) => {
                log::debug!("claude oauth usage: {err}");
                None
            }
        }
    } else {
        None
    };
    let merged = merge_plan(live.clone(), local);
    if let Some(usage) = live {
        claude_cache_put(usage);
    }
    merged.or_else(|| claude_cache_get(true))
}

fn claude_cache_get(allow_stale: bool) -> Option<LocalPlanUsage> {
    let guard = CLAUDE_LIVE_CACHE.lock().ok()?;
    let cached = guard.as_ref()?;
    if allow_stale || cached.stored_at.elapsed() < CLAUDE_USAGE_TTL {
        Some(cached.usage.clone())
    } else {
        None
    }
}

fn claude_cache_put(usage: LocalPlanUsage) {
    if let Ok(mut guard) = CLAUDE_LIVE_CACHE.lock() {
        *guard = Some(CachedPlanUsage {
            stored_at: Instant::now(),
            usage,
        });
    }
}

pub fn parse_claude_oauth_usage(value: &Value) -> Option<LocalPlanUsage> {
    let five = claude_usage_bucket(
        value,
        &["five_hour", "five-hour", "5h", "five_hour_utilization"],
    );
    let week = claude_usage_bucket(
        value,
        &[
            "seven_day",
            "seven-day",
            "7d",
            "weekly",
            "weekly_all_models",
            "seven_day_utilization",
        ],
    );
    if five.is_none() && week.is_none() {
        return None;
    }
    let (five_used, five_reset) = five.unwrap_or((None, None));
    let (week_used, week_reset) = week.unwrap_or((None, None));
    Some(
        LocalPlanUsage {
            five_hour_percent: five_used.map(|value| value.round() as u32),
            seven_day_percent: week_used.map(|value| value.round() as u32),
            five_hour_reset: five_reset,
            seven_day_reset: week_reset,
            source: "api.anthropic.com/api/oauth/usage".into(),
            ..LocalPlanUsage::default()
        }
        .finalize(),
    )
}

fn claude_usage_bucket(
    value: &Value,
    names: &[&str],
) -> Option<(Option<f32>, Option<DateTime<Utc>>)> {
    for name in names {
        if let Some(bucket) = value.get(*name) {
            if bucket.is_null() {
                continue;
            }
            if let Some(parsed) = parse_claude_bucket(bucket) {
                return Some(parsed);
            }
        }
    }
    let limits = value.get("limits").and_then(Value::as_array)?;
    for item in limits {
        let label = item
            .get("type")
            .or_else(|| item.get("id"))
            .or_else(|| item.get("name"))
            .or_else(|| item.get("display_name"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .replace('-', "_");
        if names.iter().any(|name| {
            let expected = name.replace('-', "_");
            label == expected
                || label == format!("{expected}_utilization")
                || (expected == "weekly_all_models" && label.contains("all_models"))
        }) {
            if let Some(parsed) = parse_claude_bucket(item) {
                return Some(parsed);
            }
        }
    }
    None
}

fn parse_claude_bucket(bucket: &Value) -> Option<(Option<f32>, Option<DateTime<Utc>>)> {
    let used = json_f32_opt(bucket.get("utilization"))
        .or_else(|| json_f32_opt(bucket.get("used_percentage")))
        .or_else(|| json_f32_opt(bucket.get("usedPercent")))
        .or_else(|| json_f32_opt(bucket.get("used")))
        .or_else(|| json_f32_opt(Some(bucket)));
    let reset = parse_reset_at(bucket.get("resets_at"))
        .or_else(|| parse_reset_at(bucket.get("resetsAt")))
        .or_else(|| parse_reset_at(bucket.get("resetTime")));
    if used.is_none() && reset.is_none() {
        return None;
    }
    Some((used, reset))
}

fn claude_oauth_token() -> Option<String> {
    if let Some(token) = claude_oauth_token_from_file() {
        return Some(token);
    }
    claude_oauth_token_from_os_store()
}

fn claude_oauth_token_from_file() -> Option<String> {
    for path in claude_credential_files() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            if let Some(token) = extract_oauth_token(&value) {
                return Some(token);
            }
        }
    }
    None
}

fn claude_credential_files() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
        paths.push(dir.join(".credentials.json"));
        paths.push(dir.join("credentials.json"));
    }
    if let Some(home) = home_dir() {
        paths.push(home.join(".claude/.credentials.json"));
        paths.push(home.join(".claude/credentials.json"));
        paths.push(home.join(".config/claude/.credentials.json"));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        paths.push(xdg.join("claude/.credentials.json"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        paths.push(appdata.join("Claude/.credentials.json"));
        paths.push(appdata.join("claude/.credentials.json"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        paths.push(local.join("Claude/.credentials.json"));
        paths.push(local.join("claude/.credentials.json"));
    }
    paths
}

fn extract_oauth_token(value: &Value) -> Option<String> {
    let oauth = value.get("claudeAiOauth").unwrap_or(value);
    oauth
        .get("accessToken")
        .or_else(|| oauth.get("access_token"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| token.len() > 20)
        .map(str::to_string)
}

fn claude_oauth_token_from_os_store() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        read_secret_command(
            "security",
            &[
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ],
        )
    }
    #[cfg(target_os = "windows")]
    {
        windows_credential("Claude Code-credentials")
            .or_else(|| windows_credential("Claude Code-credentials/Claude Code"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        read_secret_command(
            "secret-tool",
            &["lookup", "service", "Claude Code-credentials"],
        )
        .or_else(|| {
            read_secret_command(
                "secret-tool",
                &[
                    "lookup",
                    "service",
                    "Claude Code-credentials",
                    "account",
                    "Claude Code",
                ],
            )
        })
    }
}

fn read_secret_command(program: &str, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_secret_blob(&String::from_utf8(output.stdout).ok()?)
}

fn parse_secret_blob(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(raw) {
        return extract_oauth_token(&value);
    }
    (raw.len() > 20).then(|| raw.to_string())
}

#[cfg(target_os = "windows")]
fn windows_credential(target: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const SCRIPT: &str = r#"$ErrorActionPreference='SilentlyContinue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class NativeCred {
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
  public struct CREDENTIAL {
    public uint Flags; public uint Type; public string TargetName; public string Comment;
    public System.Runtime.InteropServices.ComTypes.FILETIME LastWritten;
    public uint CredentialBlobSize; public IntPtr CredentialBlob; public uint Persist;
    public uint AttributeCount; public IntPtr Attributes; public string TargetAlias; public string UserName;
  }
  [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  public static extern bool CredRead(string target, uint type, uint flags, out IntPtr cred);
  [DllImport("advapi32.dll")] public static extern void CredFree(IntPtr cred);
  public static string Read(string target) {
    IntPtr p;
    if (!CredRead(target, 1, 0, out p)) return null;
    var c = Marshal.PtrToStructure<CREDENTIAL>(p);
    string s = (c.CredentialBlob != IntPtr.Zero && c.CredentialBlobSize > 0)
      ? Marshal.PtrToStringUni(c.CredentialBlob, (int)c.CredentialBlobSize / 2) : null;
    CredFree(p);
    return s;
  }
}
"@
[NativeCred]::Read($env:LOUNGE_CRED_TARGET)"#;
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("LOUNGE_CRED_TARGET", target)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_secret_blob(&String::from_utf8(output.stdout).ok()?)
}

async fn antigravity_live_usage() -> Option<LocalPlanUsage> {
    let local = tokio::task::spawn_blocking(antigravity_plan)
        .await
        .ok()
        .flatten();
    let token = tokio::task::spawn_blocking(antigravity_google_token)
        .await
        .ok()
        .flatten();
    let mut live = None;
    if let Some(token) = token {
        if let Ok(assist) = google_post(
            &format!("{CLOUD_CODE}/v1internal:loadCodeAssist"),
            &token,
            serde_json::json!({ "metadata": { "ideType": "ANTIGRAVITY" } }),
        )
        .await
        {
            if let Some(project) = parse_code_assist_project(&assist) {
                live = google_post(
                    &format!("{CLOUD_CODE}/v1internal:retrieveUserQuotaSummary"),
                    &token,
                    serde_json::json!({ "project": project }),
                )
                .await
                .ok()
                .and_then(|payload| parse_antigravity_quota_summary(&payload));
                if live.is_none() {
                    live = google_post(
                        &format!("{CLOUD_CODE}/v1internal:fetchAvailableModels"),
                        &token,
                        serde_json::json!({ "project": project }),
                    )
                    .await
                    .ok()
                    .and_then(|payload| parse_antigravity_models(&payload));
                }
            }
        }
    }
    if live.is_none() {
        live = tokio::task::spawn_blocking(antigravity_proto_usage)
            .await
            .ok()
            .flatten();
    }
    merge_plan(live, local)
}

fn merge_plan(
    live: Option<LocalPlanUsage>,
    fallback: Option<LocalPlanUsage>,
) -> Option<LocalPlanUsage> {
    match (live, fallback) {
        (Some(mut live), Some(fallback)) => {
            if live.plan.is_none() {
                live.plan = fallback.plan;
            }
            if live.status.is_none() {
                live.status = fallback.status;
            }
            if live.windows.is_empty() && !fallback.windows.is_empty() {
                live.windows = fallback.windows;
            }
            if let Some(percent) = live.percent {
                if live
                    .windows
                    .iter()
                    .all(|window| window.used_percent.is_none())
                {
                    if let Some(first) = live.windows.first_mut() {
                        first.used_percent = Some(percent);
                    }
                }
            }
            Some(live.finalize())
        }
        (Some(live), None) => Some(live.finalize()),
        (None, fallback) => fallback,
    }
}

pub fn parse_cursor_period_usage(value: &Value) -> Option<LocalPlanUsage> {
    let plan = value.get("planUsage")?;
    let used_cents =
        json_f32_opt(plan.get("includedSpend")).or_else(|| json_f32_opt(plan.get("totalSpend")));
    let limit_cents = json_f32_opt(plan.get("limit")).filter(|value| *value > 0.0);
    let remaining_cents = json_f32_opt(plan.get("remaining"));
    let percent = json_f32_opt(plan.get("totalPercentUsed")).or_else(|| {
        let used = used_cents?;
        let limit = limit_cents?;
        Some(((used / limit) * 100.0).clamp(0.0, 999.0))
    })?;
    let reset_at = parse_reset_at(value.get("billingCycleEnd"));
    Some(
        LocalPlanUsage {
            percent: Some(percent),
            spend_used: used_cents.map(|cents| cents / 100.0),
            spend_limit: limit_cents.map(|cents| cents / 100.0),
            remaining_spend: remaining_cents.map(|cents| cents / 100.0),
            reset_at,
            reset: reset_at.map(|at| remaining_until(at, Utc::now())),
            source: "api2.cursor.sh/GetCurrentPeriodUsage".into(),
            ..LocalPlanUsage::default()
        }
        .finalize(),
    )
}

pub fn parse_cursor_auth_usage(value: &Value) -> Option<LocalPlanUsage> {
    let bucket = value.get("gpt-4").or_else(|| value.get("gpt-4o"))?;
    let used = json_f32_opt(bucket.get("numRequests"))?;
    let limit = json_f32_opt(bucket.get("maxRequestUsage")).filter(|value| *value > 0.0)?;
    let percent = ((used / limit) * 100.0).clamp(0.0, 999.0);
    Some(
        LocalPlanUsage {
            percent: Some(percent),
            reset: value
                .get("startOfMonth")
                .and_then(Value::as_str)
                .map(|text| text.chars().take(10).collect()),
            source: "api2.cursor.sh/auth/usage".into(),
            ..LocalPlanUsage::default()
        }
        .finalize(),
    )
}

pub fn parse_sand_usage(value: &Value) -> Option<LocalPlanUsage> {
    if value
        .get("usesPooledEnterpriseAllowance")
        .and_then(Value::as_bool)
        == Some(true)
    {
        return None;
    }
    if value.get("includedLimitZero").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    if value
        .get("hasNonZeroIncludedLimit")
        .and_then(Value::as_bool)
        == Some(false)
    {
        return None;
    }
    let percent = json_f32_opt(value.get("usagePercent"))?;
    let reset_at = parse_reset_at(value.get("nextResetTimestampUtc"));
    Some(
        LocalPlanUsage {
            plan: Some("Grok Bot".into()),
            percent: Some(percent.clamp(0.0, 999.0)),
            reset_at,
            reset: reset_at.map(|at| remaining_until(at, Utc::now())),
            source: "api2.cursor.sh/GetSandUsageStatus".into(),
            ..LocalPlanUsage::default()
        }
        .finalize(),
    )
}

pub fn parse_antigravity_quota_summary(value: &Value) -> Option<LocalPlanUsage> {
    let groups = value
        .pointer("/response/groups")
        .or_else(|| value.get("groups"))
        .and_then(Value::as_array)?;
    let mut windows = Vec::new();
    let mut five = None;
    let mut week = None;
    let mut five_reset = None;
    let mut week_reset = None;
    for bucket in groups
        .iter()
        .filter_map(|group| group.get("buckets").and_then(Value::as_array))
        .flatten()
    {
        let id = bucket
            .get("bucketId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let Some(remaining) = json_f32_opt(bucket.get("remainingFraction")) else {
            continue;
        };
        let used = ((1.0 - remaining.clamp(0.0, 1.0)) * 100.0).round() as u32;
        let reset_at = parse_reset_at(bucket.get("resetTime"))
            .or_else(|| parse_reset_at(bucket.get("resetAt")))
            .or_else(|| parse_reset_at(bucket.get("reset_time")));
        let Some(window) = antigravity_bucket_window(&id, used, reset_at) else {
            continue;
        };
        if window.suffix.ends_with("5h") {
            five = Some(five.map_or(used, |current: u32| current.max(used)));
            five_reset = earliest(five_reset, reset_at);
        }
        if window.suffix.ends_with("7d") {
            week = Some(week.map_or(used, |current: u32| current.max(used)));
            week_reset = earliest(week_reset, reset_at);
        }
        windows.push(window);
    }
    if windows.is_empty() {
        return None;
    }
    Some(
        LocalPlanUsage {
            five_hour_percent: five,
            seven_day_percent: week,
            five_hour_reset: five_reset,
            seven_day_reset: week_reset,
            windows,
            source: "cloudcode retrieveUserQuotaSummary".into(),
            ..LocalPlanUsage::default()
        }
        .finalize(),
    )
}

fn antigravity_bucket_window(
    id: &str,
    used: u32,
    reset_at: Option<DateTime<Utc>>,
) -> Option<QuotaWindow> {
    let third_party = id.contains("3p") || id.contains("claude") || id.contains("gpt");
    let (suffix, label) = if (id.contains("5h") || id.contains("session")) && third_party {
        ("3p-5h", "Claude/GPT · 5 saat")
    } else if id.contains("week") && third_party {
        ("3p-7d", "Claude/GPT · 7 gün")
    } else if id.contains("5h") || id.contains("session") {
        ("5h", "Gemini · 5 saat")
    } else if id.contains("week") {
        ("7d", "Gemini · 7 gün")
    } else {
        return None;
    };
    Some(QuotaWindow {
        suffix: suffix.into(),
        label: label.into(),
        used_percent: Some(used as f32),
        reset_at,
    })
}

fn earliest(current: Option<DateTime<Utc>>, next: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (current, next) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn default_antigravity_windows() -> Vec<QuotaWindow> {
    vec![
        QuotaWindow {
            suffix: "5h".into(),
            label: "Gemini · 5 saat".into(),
            used_percent: None,
            reset_at: None,
        },
        QuotaWindow {
            suffix: "7d".into(),
            label: "Gemini · 7 gün".into(),
            used_percent: None,
            reset_at: None,
        },
    ]
}

pub fn parse_antigravity_models(value: &Value) -> Option<LocalPlanUsage> {
    let models = value.get("models")?.as_object()?;
    let mut worst_remaining = None;
    for model in models.values() {
        if model.get("isInternal").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(remaining) = model
            .get("quotaInfo")
            .and_then(|info| json_f32_opt(info.get("remainingFraction")))
        else {
            continue;
        };
        worst_remaining = Some(match worst_remaining {
            Some(current) if remaining < current => remaining,
            Some(current) => current,
            None => remaining,
        });
    }
    let remaining = worst_remaining?;
    let used = ((1.0 - remaining.clamp(0.0, 1.0)) * 100.0).round() as u32;
    Some(
        LocalPlanUsage {
            percent: Some(used as f32),
            source: "cloudcode fetchAvailableModels".into(),
            ..LocalPlanUsage::default()
        }
        .finalize(),
    )
}

pub fn parse_code_assist_project(value: &Value) -> Option<String> {
    value
        .get("cloudaicompanionProject")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .pointer("/cloudaicompanionProject/id")
                .and_then(Value::as_str)
        })
        .or_else(|| value.get("project").and_then(Value::as_str))
        .or_else(|| value.get("currentProject").and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn antigravity_proto_usage() -> Option<LocalPlanUsage> {
    let path = app_support("Antigravity")?
        .join("User")
        .join("globalStorage")
        .join("state.vscdb");
    let raw = sqlite_item(&path, "antigravityUnifiedStateSync.userStatus")?;
    let used = used_percent_from_synced_blob(&raw)?;
    let mut windows = default_antigravity_windows();
    if let Some(first) = windows.first_mut() {
        first.used_percent = Some(used);
    }
    Some(
        LocalPlanUsage {
            percent: Some(used),
            windows,
            source: "antigravity userStatus proto".into(),
            ..LocalPlanUsage::default()
        }
        .finalize(),
    )
}

pub fn used_percent_from_synced_blob(raw: &str) -> Option<f32> {
    let mut blobs = Vec::new();
    if let Ok(bytes) =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, raw.trim())
    {
        blobs.push(bytes);
    } else {
        blobs.push(raw.as_bytes().to_vec());
    }
    let mut remaining = Vec::new();
    for blob in &blobs {
        collect_proto_fractions(blob, &mut remaining, 0);
        for nested in extract_nested_base64(blob) {
            collect_proto_fractions(&nested, &mut remaining, 0);
        }
    }
    let min_remaining = remaining
        .into_iter()
        .filter(|value| (0.0..=1.0).contains(value))
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))?;
    Some(((1.0 - min_remaining) * 100.0).clamp(0.0, 100.0))
}

fn extract_nested_base64(blob: &[u8]) -> Vec<Vec<u8>> {
    let Ok(text) = std::str::from_utf8(blob) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '+' || ch == '/' || ch == '=' {
            current.push(ch);
        } else if current.len() >= 80 {
            if let Ok(bytes) =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &current)
            {
                out.push(bytes);
            }
            current.clear();
        } else {
            current.clear();
        }
    }
    if current.len() >= 80 {
        if let Ok(bytes) =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &current)
        {
            out.push(bytes);
        }
    }
    out
}

fn collect_proto_fractions(data: &[u8], out: &mut Vec<f32>, depth: u8) {
    if depth > 8 {
        return;
    }
    let mut i = 0;
    while i < data.len() {
        let Some((key, next)) = decode_varint(data, i) else {
            break;
        };
        i = next;
        match key & 7 {
            0 => {
                let Some((_, next)) = decode_varint(data, i) else {
                    break;
                };
                i = next;
            }
            1 => {
                if i + 8 > data.len() {
                    break;
                }
                i += 8;
            }
            5 => {
                if i + 4 > data.len() {
                    break;
                }
                let bits = u32::from_le_bytes(data[i..i + 4].try_into().unwrap_or([0; 4]));
                let value = f32::from_bits(bits);
                if value.is_finite() && (0.0..=1.0).contains(&value) {
                    out.push(value);
                }
                i += 4;
            }
            2 => {
                let Some((len, next)) = decode_varint(data, i) else {
                    break;
                };
                i = next;
                let end = i.saturating_add(len as usize);
                if end > data.len() {
                    break;
                }
                collect_proto_fractions(&data[i..end], out, depth + 1);
                i = end;
            }
            _ => break,
        }
    }
}

fn decode_varint(data: &[u8], mut i: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0;
    while i < data.len() {
        let byte = data[i];
        i += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte < 0x80 {
            return Some((value, i));
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
    None
}

fn cursor_access_token() -> Option<String> {
    let path = app_support("Cursor")?
        .join("User")
        .join("globalStorage")
        .join("state.vscdb");
    sqlite_item(&path, "cursorAuth/accessToken").filter(|token| token.len() > 20)
}

fn antigravity_google_token() -> Option<String> {
    let path = app_support("Antigravity")?
        .join("User")
        .join("globalStorage")
        .join("state.vscdb");
    let raw = sqlite_item(&path, "antigravityAuthStatus")?;
    let value: Value = serde_json::from_str(&raw).ok()?;
    value
        .get("apiKey")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| token.len() > 20)
        .map(str::to_string)
}

pub fn parse_reset_at(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let value = value?;
    if let Some(text) = value.as_str() {
        return parse_reset_text(text);
    }
    if let Some(n) = json_i64(value) {
        return timestamp_to_utc(n);
    }
    if let Some(obj) = value.as_object() {
        if let Some(secs) = obj.get("seconds").and_then(json_i64) {
            return Utc.timestamp_opt(secs, 0).single();
        }
        if let Some(ms) = obj
            .get("millis")
            .or_else(|| obj.get("milliseconds"))
            .and_then(json_i64)
        {
            return Utc.timestamp_millis_opt(ms).single();
        }
        if let Some(nested) = obj.get("value").or_else(|| obj.get("time")) {
            return parse_reset_at(Some(nested));
        }
    }
    None
}

fn parse_reset_text(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(text) {
        return Some(parsed.with_timezone(&Utc));
    }
    if let Ok(n) = text.parse::<i64>() {
        return timestamp_to_utc(n);
    }
    if let Ok(day) = NaiveDate::parse_from_str(text, "%Y-%m-%d") {
        return day
            .and_hms_opt(0, 0, 0)
            .map(|naive| DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc));
    }
    None
}

fn timestamp_to_utc(n: i64) -> Option<DateTime<Utc>> {
    if n.abs() >= 1_000_000_000_000 {
        Utc.timestamp_millis_opt(n).single()
    } else if n.abs() >= 1_000_000_000 {
        Utc.timestamp_opt(n, 0).single()
    } else {
        None
    }
}

pub fn remaining_until(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    if at <= now {
        return "şimdi".into();
    }
    let minutes = at.signed_duration_since(now).num_minutes().max(1);
    let days = minutes / (60 * 24);
    let hours = (minutes / 60) % 24;
    let mins = minutes % 60;
    if days >= 1 {
        if hours == 0 {
            format!("{days}g")
        } else {
            format!("{days}g {hours}s")
        }
    } else if hours >= 1 {
        if mins == 0 {
            format!("{hours}s")
        } else {
            format!("{hours}s {mins}dk")
        }
    } else {
        format!("{mins}dk")
    }
}

async fn cursor_rpc(path: &str, token: &str) -> Result<Value, String> {
    http_json(
        "POST",
        &format!("{CURSOR_API}{path}"),
        token,
        Some("{}"),
        &[
            ("Content-Type", "application/json"),
            ("Connect-Protocol-Version", "1"),
        ],
    )
    .await
}

async fn cursor_get(path: &str, token: &str) -> Result<Value, String> {
    http_json("GET", &format!("{CURSOR_API}{path}"), token, None, &[]).await
}

async fn google_post(url: &str, token: &str, body: Value) -> Result<Value, String> {
    http_json(
        "POST",
        url,
        token,
        Some(&body.to_string()),
        &[
            ("Content-Type", "application/json"),
            ("User-Agent", "antigravity"),
        ],
    )
    .await
}

async fn http_json(
    method: &str,
    url: &str,
    token: &str,
    body: Option<&str>,
    extra: &[(&str, &str)],
) -> Result<Value, String> {
    let client = reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|err| err.to_string())?;
    let mut builder = match method {
        "POST" => client.post(url),
        _ => client.get(url),
    }
    .header("Authorization", format!("Bearer {token}"));
    for (key, value) in extra {
        builder = builder.header(*key, *value);
    }
    if let Some(body) = body {
        builder = builder.body(body.to_string());
    }
    let response = builder.send().await.map_err(|err| err.to_string())?;
    let status = response.status();
    if !status.is_success() {
        log::debug!("subscription usage HTTP {status}");
        return Err(format!("HTTP {status}"));
    }
    response.json().await.map_err(|err| err.to_string())
}

pub fn claude_plan_usage() -> Option<LocalPlanUsage> {
    let mut usage = LocalPlanUsage {
        source: "claude/plan-usage-history.json".into(),
        ..LocalPlanUsage::default()
    };
    if let Some(plan) = claude_organization_plan() {
        usage.plan = Some(plan);
        usage.source = "~/.claude.json".into();
    }
    if let Some(windows) = claude_usage_windows() {
        usage.five_hour_percent = windows.five_hour_percent;
        usage.seven_day_percent = windows.seven_day_percent;
        usage.five_hour_reset = windows.five_hour_reset;
        usage.seven_day_reset = windows.seven_day_reset;
        usage.source = windows.source;
    }
    let mut usage = usage.finalize();
    if usage.windows.is_empty() {
        usage.windows = vec![
            QuotaWindow {
                suffix: "5h".into(),
                label: "5 saat".into(),
                used_percent: None,
                reset_at: None,
            },
            QuotaWindow {
                suffix: "7d".into(),
                label: "7 gün".into(),
                used_percent: None,
                reset_at: None,
            },
        ];
    }
    if usage.plan.is_none()
        && usage.percent.is_none()
        && usage.windows.iter().all(|w| w.used_percent.is_none())
    {
        None
    } else {
        Some(usage)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ClaudeWindows {
    five_hour_percent: Option<u32>,
    seven_day_percent: Option<u32>,
    five_hour_reset: Option<DateTime<Utc>>,
    seven_day_reset: Option<DateTime<Utc>>,
    source: String,
}

fn claude_usage_windows() -> Option<ClaudeWindows> {
    for path in claude_history_files() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(mut windows) = parse_claude_plan_history(&value, Utc::now()) {
            windows.source = path.display().to_string();
            return Some(windows);
        }
    }
    None
}

fn claude_history_files() -> Vec<PathBuf> {
    ["Claude", "claude"]
        .into_iter()
        .filter_map(app_support)
        .map(|root| root.join("plan-usage-history.json"))
        .collect()
}

fn parse_claude_plan_history(value: &Value, now: DateTime<Utc>) -> Option<ClaudeWindows> {
    let samples = value.get("samples").and_then(Value::as_array)?;
    let mut last_five = None;
    let mut last_week = None;
    for sample in samples {
        let captured = sample
            .get("t")
            .and_then(json_i64)
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single())?;
        let usage = sample.get("u").and_then(Value::as_object)?;
        if let Some(fh) = usage.get("fh").and_then(json_u32) {
            last_five = Some((captured, fh));
        }
        if let Some(sd) = usage.get("sd").and_then(json_u32) {
            last_week = Some((captured, sd));
        }
    }
    if last_five.is_none() && last_week.is_none() {
        return None;
    }
    let five_hour_percent = last_five.map(|(captured, used)| {
        if now.signed_duration_since(captured) > FIVE_HOUR_PERIOD {
            0
        } else {
            used
        }
    });
    let seven_day_percent = last_week.map(|(captured, used)| {
        if now.signed_duration_since(captured) > SEVEN_DAY_PERIOD {
            0
        } else {
            used
        }
    });
    Some(ClaudeWindows {
        five_hour_percent,
        seven_day_percent,
        five_hour_reset: None,
        seven_day_reset: None,
        source: String::new(),
    })
}

fn claude_organization_plan() -> Option<String> {
    for path in claude_org_files() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(org) = value
            .pointer("/oauthAccount/organizationType")
            .and_then(Value::as_str)
        {
            return Some(human_claude_plan(org));
        }
    }
    None
}

fn claude_org_files() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
        paths.push(dir.join(".claude.json"));
        paths.push(dir.join("claude.json"));
    }
    if let Some(home) = home_dir() {
        paths.push(home.join(".claude.json"));
        paths.push(home.join(".claude/.claude.json"));
    }
    if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
        paths.push(appdata.join("Claude/.claude.json"));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        paths.push(xdg.join("claude/.claude.json"));
    }
    paths
}

pub fn human_claude_plan(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "claude_pro" | "pro" => "Pro".into(),
        "claude_max" | "max" => "Max".into(),
        "claude_team" | "team" => "Team".into(),
        "claude_enterprise" | "enterprise" => "Enterprise".into(),
        "" => "Pro".into(),
        other => other.replace('_', " "),
    }
}

fn cursor_membership() -> Option<LocalPlanUsage> {
    let path = app_support("Cursor")?
        .join("User")
        .join("globalStorage")
        .join("state.vscdb");
    let plan =
        sqlite_item(&path, "cursorAuth/stripeMembershipType").map(|raw| human_cursor_plan(&raw))?;
    let status =
        sqlite_item(&path, "cursorAuth/stripeSubscriptionStatus").map(|raw| human_status(&raw));
    Some(
        LocalPlanUsage {
            plan: Some(plan),
            status,
            source: path.display().to_string(),
            ..LocalPlanUsage::default()
        }
        .finalize(),
    )
}

pub fn human_cursor_plan(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "pro" => "Pro".into(),
        "pro_plus" | "pro-plus" | "proplus" => "Pro+".into(),
        "business" | "team" => "Business".into(),
        "ultra" => "Ultra".into(),
        "free" | "hobby" => "Free".into(),
        "" => "Cursor".into(),
        other => other.replace('_', " "),
    }
}

fn antigravity_plan() -> Option<LocalPlanUsage> {
    let support = app_support("Antigravity")?;
    if !support.exists() {
        return None;
    }
    let path = support
        .join("User")
        .join("globalStorage")
        .join("state.vscdb");
    let haystack = sqlite_item(&path, "antigravityUnifiedStateSync.userStatus")
        .or_else(|| sqlite_item(&path, "antigravityAuthStatus"));
    let plan = haystack.as_deref().and_then(plan_from_haystack);
    Some(
        LocalPlanUsage {
            plan,
            status: Some("active".into()),
            windows: default_antigravity_windows(),
            source: if path.exists() {
                path.display().to_string()
            } else {
                support.display().to_string()
            },
            ..LocalPlanUsage::default()
        }
        .finalize(),
    )
}

pub fn plan_from_haystack(raw: &str) -> Option<String> {
    for needle in [
        "Google AI Ultra",
        "Google AI Pro",
        "Google AI Free",
        "g1-ultra-tier",
        "g1-pro-tier",
    ] {
        if raw.contains(needle) {
            return Some(match needle {
                "g1-ultra-tier" => "Google AI Ultra".into(),
                "g1-pro-tier" => "Google AI Pro".into(),
                other => other.into(),
            });
        }
    }
    None
}

fn grok_session() -> Option<LocalPlanUsage> {
    let path = app_support("Grok Bot")?.join("desktop-status.json");
    let text = std::fs::read_to_string(&path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let signed_in = value
        .get("signedIn")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(
        LocalPlanUsage {
            plan: Some("Grok".into()),
            status: Some(if signed_in {
                "oturum açık".into()
            } else {
                "oturum yok".into()
            }),
            source: path.display().to_string(),
            ..LocalPlanUsage::default()
        }
        .finalize(),
    )
}

pub fn human_status(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().as_str() {
        "active" => "active".into(),
        "past_due" | "pastdue" => "past due".into(),
        "canceled" | "cancelled" => "canceled".into(),
        "trialing" | "trial" => "trial".into(),
        other => other.replace('_', " "),
    }
}

fn sqlite_item(path: &Path, key: &str) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let conn = Connection::open_with_flags(path, flags).ok()?;
    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key = ?1")
        .ok()?;
    let mut rows = stmt.query([key]).ok()?;
    let row = rows.next().ok()??;
    if let Ok(text) = row.get::<_, String>(0) {
        let trimmed = text.trim().to_string();
        return (!trimmed.is_empty()).then_some(trimmed);
    }
    let bytes: Vec<u8> = row.get(0).ok()?;
    String::from_utf8(bytes)
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn app_support(app: &str) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        Some(home_dir()?.join("Library/Application Support").join(app))
    }
    #[cfg(target_os = "windows")]
    {
        Some(PathBuf::from(std::env::var_os("APPDATA")?).join(app))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
            return Some(PathBuf::from(xdg).join(app));
        }
        Some(home_dir()?.join(".config").join(app))
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().map(|n| n as i64))
        .or_else(|| value.as_f64().map(|n| n as i64))
}

fn json_u32(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .map(|n| n as u32)
        .or_else(|| value.as_f64().map(|n| n.round() as u32))
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.parse::<f64>().ok().map(|n| n.round() as u32))
        })
}

fn json_f32_opt(value: Option<&Value>) -> Option<f32> {
    let value = value?;
    value
        .as_f64()
        .map(|n| n as f32)
        .or_else(|| value.as_i64().map(|n| n as f32))
        .or_else(|| value.as_u64().map(|n| n as f32))
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .filter(|n| n.is_finite())
}

#[allow(dead_code)]
pub fn amber_from_percent(percent: Option<f32>) -> bool {
    percent
        .map(|value| value >= AMBER_THRESHOLD)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_history_keeps_fresh_windows() {
        let now = Utc.with_ymd_and_hms(2026, 9, 21, 8, 0, 0).unwrap();
        let fresh = now - Duration::minutes(20);
        let payload = json!({
            "samples": [{
                "t": fresh.timestamp_millis(),
                "u": { "fh": 82, "sd": 41 }
            }]
        });
        let parsed = parse_claude_plan_history(&payload, now).unwrap();
        assert_eq!(parsed.five_hour_percent, Some(82));
        assert_eq!(parsed.seven_day_percent, Some(41));
        assert!(parsed.five_hour_reset.is_none());
        assert!(parsed.seven_day_reset.is_none());
        let usage = LocalPlanUsage {
            five_hour_percent: parsed.five_hour_percent,
            seven_day_percent: parsed.seven_day_percent,
            five_hour_reset: parsed.five_hour_reset,
            seven_day_reset: parsed.seven_day_reset,
            ..LocalPlanUsage::default()
        }
        .finalize();
        assert_eq!(usage.percent, Some(82.0));
        assert!(amber_from_percent(usage.percent));
        assert_eq!(usage.used_label(false), "82% 5s · 41% 7g");
        assert_eq!(usage.remaining_label(), "18% 5s · 59% 7g");
        assert!(usage.reset_label().contains(" · "));
        assert_eq!(usage.windows.len(), 2);
    }

    #[test]
    fn claude_history_expired_five_hour_keeps_week_if_in_window() {
        let now = Utc.with_ymd_and_hms(2026, 9, 21, 8, 0, 0).unwrap();
        let four_days_ago = now - Duration::days(4);
        let payload = json!({
            "samples": [{
                "t": four_days_ago.timestamp_millis(),
                "u": { "fh": 90, "sd": 56 }
            }]
        });
        let parsed = parse_claude_plan_history(&payload, now).unwrap();
        assert_eq!(parsed.five_hour_percent, Some(0));
        assert_eq!(parsed.seven_day_percent, Some(56));
        assert!(parsed.five_hour_reset.is_none());
        let usage = LocalPlanUsage {
            five_hour_percent: parsed.five_hour_percent,
            seven_day_percent: parsed.seven_day_percent,
            ..LocalPlanUsage::default()
        }
        .finalize();
        assert_eq!(usage.percent, Some(56.0));
        assert!(!amber_from_percent(usage.percent));
        assert_eq!(usage.used_label(true), "0% 5s · 56% 7g");
        assert_eq!(usage.remaining_label(), "100% 5s · 44% 7g");
        assert_eq!(usage.reset_label(), "kullanınca · —");
    }

    #[test]
    fn claude_history_keeps_week_within_seven_days() {
        let now = Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap();
        let week_start = Utc.with_ymd_and_hms(2026, 9, 14, 7, 55, 0).unwrap();
        let mid = Utc.with_ymd_and_hms(2026, 9, 16, 20, 48, 0).unwrap();
        let payload = json!({
            "samples": [
                { "t": (week_start - Duration::hours(1)).timestamp_millis(), "u": { "fh": 40, "sd": 88 } },
                { "t": week_start.timestamp_millis(), "u": { "fh": 0, "sd": 0 } },
                { "t": mid.timestamp_millis(), "u": { "fh": 46, "sd": 56 } }
            ]
        });
        let parsed = parse_claude_plan_history(&payload, now).unwrap();
        assert_eq!(parsed.five_hour_percent, Some(0));
        assert_eq!(parsed.seven_day_percent, Some(56));
        assert!(parsed.seven_day_reset.is_none());
    }

    #[test]
    fn claude_history_all_stale_reads_as_zero() {
        let now = Utc.with_ymd_and_hms(2026, 9, 21, 8, 0, 0).unwrap();
        let old = now - Duration::days(20);
        let payload = json!({
            "samples": [{ "t": old.timestamp_millis(), "u": { "fh": 99, "sd": 99 } }]
        });
        let parsed = parse_claude_plan_history(&payload, now).unwrap();
        assert_eq!(parsed.five_hour_percent, Some(0));
        assert_eq!(parsed.seven_day_percent, Some(0));
    }

    #[test]
    fn exhausted_at_hundred() {
        let usage = LocalPlanUsage {
            seven_day_percent: Some(100),
            ..LocalPlanUsage::default()
        }
        .finalize();
        let tool = DiscoveredTool {
            id: "app:claude_desktop".into(),
            name: "Claude Desktop".into(),
            kind: "app".into(),
            source: "claude_desktop".into(),
            origin_path: None,
            command: None,
            args: Vec::new(),
            endpoint: None,
            detail: None,
            available: true,
            access_mode: "subscription".into(),
            host_id: Some("claude_desktop".into()),
            host_ids: Vec::new(),
        };
        let row = usage.clone().into_quota(&tool, false);
        assert!(row.exhausted);
        assert_eq!(row.tone, "amber");
        assert_eq!(row.percent, Some(100.0));
        let rows = usage.into_quotas(&tool, false);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "app:claude_desktop");
        assert_eq!(rows[0].tool, "Claude Desktop");
        assert_eq!(rows[0].used, "100% 7g");
        assert_eq!(rows[0].reset, "—");
    }

    #[test]
    fn labels_membership_without_inventing_percent() {
        let usage = LocalPlanUsage {
            plan: Some("Pro".into()),
            status: Some("active".into()),
            source: "state.vscdb".into(),
            ..LocalPlanUsage::default()
        }
        .finalize();
        assert_eq!(usage.percent, None);
        assert_eq!(usage.used_label(true), "Pro · active");
        assert_eq!(usage.remaining_label(), "kota yok");
        assert!(!amber_from_percent(usage.percent));
    }

    #[test]
    fn antigravity_plan_from_proto_haystack() {
        assert_eq!(
            plan_from_haystack("xxx g1-pro-tier Google AI Pro yyy").as_deref(),
            Some("Google AI Pro")
        );
        assert_eq!(
            plan_from_haystack("g1-ultra-tier").as_deref(),
            Some("Google AI Ultra")
        );
        assert_eq!(plan_from_haystack("no plan here"), None);
    }

    #[test]
    fn parses_claude_oauth_usage_windows_and_resets() {
        let payload = json!({
            "five_hour": { "utilization": 23.4, "resets_at": "2026-09-21T14:30:00Z" },
            "seven_day": { "utilization": 61.0, "resets_at": "2026-09-25T07:55:00Z" },
            "seven_day_opus": null
        });
        let usage = parse_claude_oauth_usage(&payload).unwrap();
        assert_eq!(usage.five_hour_percent, Some(23));
        assert_eq!(usage.seven_day_percent, Some(61));
        assert!(usage.five_hour_reset.is_some());
        assert!(usage.seven_day_reset.is_some());
        assert_eq!(usage.used_label(false), "23% 5s · 61% 7g");
        assert_ne!(usage.reset_label(), "7g");
        assert!(!usage.reset_label().contains("kullanınca · 6g"));
    }

    #[test]
    fn parses_claude_oauth_limits_array() {
        let payload = json!({
            "limits": [
                { "type": "five_hour", "used_percentage": 10, "resets_at": 1789990000 },
                { "id": "seven_day", "utilization": 40.2, "resets_at": "2026-09-28T08:00:00Z" }
            ]
        });
        let usage = parse_claude_oauth_usage(&payload).unwrap();
        assert_eq!(usage.five_hour_percent, Some(10));
        assert_eq!(usage.seven_day_percent, Some(40));
    }

    #[test]
    fn humanizes_known_plans() {
        assert_eq!(human_claude_plan("claude_pro"), "Pro");
        assert_eq!(human_cursor_plan("pro"), "Pro");
        assert_eq!(human_status("past_due"), "past due");
    }

    #[test]
    fn reads_cursor_membership_from_vscdb() {
        let path =
            std::env::temp_dir().join(format!("lounge-cursor-quota-{}.vscdb", std::process::id()));
        let _ = std::fs::remove_file(&path);
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT);
                 INSERT INTO ItemTable (key, value) VALUES
                   ('cursorAuth/stripeMembershipType', 'pro'),
                   ('cursorAuth/stripeSubscriptionStatus', 'active');",
            )
            .unwrap();
        }
        assert_eq!(
            sqlite_item(&path, "cursorAuth/stripeMembershipType").as_deref(),
            Some("pro")
        );
        assert_eq!(human_cursor_plan("pro"), "Pro");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parses_cursor_period_spend_without_cookies() {
        let payload = json!({
            "billingCycleEnd": "1771077734000",
            "planUsage": {
                "totalSpend": 23222,
                "includedSpend": 23222,
                "remaining": 16778,
                "limit": 40000,
                "totalPercentUsed": 15.48
            }
        });
        let usage = parse_cursor_period_usage(&payload).unwrap();
        assert!((usage.percent.unwrap() - 15.48).abs() < 0.01);
        assert_eq!(usage.used_label(false), "15% · $232 / $400");
        assert_eq!(usage.remaining_label(), "$168");
        assert_eq!(
            usage.reset_at.unwrap().format("%Y-%m-%d").to_string(),
            "2026-02-14"
        );
        assert_eq!(usage.reset.as_deref(), Some("şimdi"));
    }

    #[test]
    fn parses_sand_usage_only_when_included() {
        assert!(parse_sand_usage(&json!({
            "usagePercent": 0,
            "includedLimitZero": true
        }))
        .is_none());
        let usage = parse_sand_usage(&json!({
            "usagePercent": 42.0,
            "hasNonZeroIncludedLimit": true,
            "includedLimitZero": false,
            "usesPooledEnterpriseAllowance": false
        }))
        .unwrap();
        assert_eq!(usage.percent, Some(42.0));
        assert_eq!(usage.used_label(true), "42%");
        assert_eq!(usage.remaining_label(), "58% plan");
        assert!(usage.reset.is_none());
        assert_eq!(usage.reset_label(), "—");

        let future = Utc::now() + Duration::hours(3) + Duration::minutes(12);
        let timed = parse_sand_usage(&json!({
            "usagePercent": 42.0,
            "hasNonZeroIncludedLimit": true,
            "includedLimitZero": false,
            "usesPooledEnterpriseAllowance": false,
            "nextResetTimestampUtc": { "seconds": future.timestamp() }
        }))
        .unwrap();
        assert!(timed.reset_at.unwrap() > Utc::now());
        assert_ne!(timed.reset.as_deref(), Some("7g"));
        assert!(timed.reset.as_deref().unwrap_or_default().contains('s'));
    }

    #[test]
    fn parses_antigravity_quota_buckets() {
        let payload = json!({
            "groups": [{
                "buckets": [
                    { "bucketId": "gemini-5h", "remainingFraction": 0.8, "resetTime": "2026-09-21T14:30:00Z" },
                    { "bucketId": "gemini-weekly", "remainingFraction": 0.65, "resetTime": "2026-09-28T07:55:00Z" },
                    { "bucketId": "3p-5h", "remainingFraction": 0.9, "resetTime": "2026-09-21T13:00:00Z" }
                ]
            }]
        });
        let usage = parse_antigravity_quota_summary(&payload).unwrap();
        assert_eq!(usage.five_hour_percent, Some(20));
        assert_eq!(usage.seven_day_percent, Some(35));
        assert_eq!(usage.windows.len(), 3);
        assert_eq!(usage.windows[0].label, "Gemini · 5 saat");
        assert_eq!(usage.windows[1].suffix, "7d");
        assert_eq!(usage.windows[2].suffix, "3p-5h");
        assert!(usage.windows[0].reset_at.is_some());
        assert_eq!(usage.used_label(false), "20% 5s · 35% 7g · 10% 3p 5s");
        assert_eq!(usage.remaining_label(), "80% 5s · 65% 7g · 90% 3p 5s");
    }

    #[test]
    fn remaining_until_formats_countdown() {
        let now = Utc.with_ymd_and_hms(2026, 9, 21, 12, 0, 0).unwrap();
        assert_eq!(remaining_until(now - Duration::minutes(1), now), "şimdi");
        assert_eq!(remaining_until(now + Duration::minutes(12), now), "12dk");
        assert_eq!(
            remaining_until(now + Duration::hours(3) + Duration::minutes(10), now),
            "3s 10dk"
        );
        assert_eq!(
            remaining_until(now + Duration::days(2) + Duration::hours(4), now),
            "2g 4s"
        );
    }

    #[test]
    fn parse_reset_accepts_rfc3339_and_unix() {
        let rfc = parse_reset_at(Some(&json!("2026-09-28T07:55:22Z"))).unwrap();
        assert_eq!(rfc.format("%Y-%m-%d").to_string(), "2026-09-28");
        let ms = parse_reset_at(Some(&json!("1771077734000"))).unwrap();
        assert_eq!(ms.format("%Y-%m-%d").to_string(), "2026-02-14");
        let secs = parse_reset_at(Some(&json!({ "seconds": 1789372522 }))).unwrap();
        assert_eq!(
            secs.format("%Y-%m-%dT%H:%M").to_string(),
            "2026-09-14T07:55"
        );
    }

    #[test]
    fn proto_fraction_becomes_used_percent() {
        // field 4, wire type 5 (32-bit), value 0.25
        let mut blob = vec![0x25];
        blob.extend_from_slice(&0.25f32.to_le_bytes());
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &blob);
        let used = used_percent_from_synced_blob(&encoded).unwrap();
        assert!((used - 75.0).abs() < 0.2);
    }

    #[test]
    fn parse_secret_blob_reads_json_or_raw_token() {
        let json = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-abcdefghijklmnopqrstuvwxyz"}}"#;
        assert_eq!(
            parse_secret_blob(json).as_deref(),
            Some("sk-ant-oat01-abcdefghijklmnopqrstuvwxyz")
        );
        assert!(parse_secret_blob("short").is_none());
        assert_eq!(
            parse_secret_blob("abcdefghijklmnopqrstuvwxyz1234").as_deref(),
            Some("abcdefghijklmnopqrstuvwxyz1234")
        );
    }

    #[test]
    fn claude_credential_files_include_dot_claude() {
        let paths = claude_credential_files();
        assert!(paths.iter().any(|path| {
            path.components()
                .any(|part| part.as_os_str() == std::ffi::OsStr::new(".claude"))
        }));
        assert!(paths.iter().any(
            |path| path.file_name().and_then(|name| name.to_str()) == Some(".credentials.json")
        ));
    }
}
