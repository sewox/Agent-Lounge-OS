//! İstemci bazlı MCP araç zaman aşımı — [`ClientProfile`] tablosu.
//!
//! Ölçümler (2026-10-04/05, mcp-probe): antigravity-client 180 sn (progress
//! uzatmaz, cancel gönderir); cursor-vscode 120 sn (−32001), progress ile 300 sn;
//! claude-ai 240 sn (progress/cancel yok); claude-code ≥300 sn (sert sınır
//! bilinmiyor); Grok Bot ölçülemedi → bilinmeyen/bulut eşiği 45 sn.
//!
//! Global override üst sınırı profil başına `hard_limit − 20` (bilinmeyen: 170).

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use super::mcp_server::normalize_client_host;

/// Bilinmeyen / bulut (Grok Bot dahil) — Gemini 60 önermişti; tutarlılık için 45.
pub const DEFAULT_UNKNOWN_THRESHOLD_SECS: u64 = 45;
/// Bilinmeyen profil için override tavanı (sabit; hard_limit yok).
pub const UNKNOWN_OVERRIDE_CAP_SECS: u64 = 170;
/// Cursor progress heartbeat aralığı.
pub const CURSOR_PROGRESS_HEARTBEAT_SECS: u64 = 10;
/// Cursor progress ile Mod A üst bekleme (300 ölçüldü − 20 sn marj).
pub const CURSOR_PROGRESS_EXTENDED_SECS: u64 = 280;

/// Settings anahtarı — saniye cinsinden global override (tüm istemciler, profil tavanıyla kırpılır).
pub const SETTING_MCP_TIMEOUT_SECS: &str = "mcp.timeout_secs";
/// Ortam değişkeni — settings’i de ezer (en yüksek öncelik).
pub const ENV_MCP_TIMEOUT_SECS: &str = "LOUNGE_MCP_TIMEOUT_SECS";

/// Profil kaynak etiketi — ölçülmüş vs varsayılan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileSource {
    Measured,
    Assumed,
}

impl ProfileSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Measured => "measured",
            Self::Assumed => "assumed",
        }
    }
}

/// İstemci yetenek + eşik profili (`clientInfo.name` eşleşmesi).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientProfile {
    /// Teşhis adı (`antigravity-client`, `cursor-vscode`, …).
    pub name: &'static str,
    /// Sert istemci limiti (sn). `None` = ölçülmedi / bilinmiyor — 3600 yazma.
    pub hard_limit_secs: Option<u64>,
    /// Lounge Mod A eşiği (sn).
    pub threshold_secs: u64,
    /// Progress heartbeat istemci zaman aşımını uzatır mı?
    pub progress_extends: bool,
    /// `notifications/cancelled` (Stop) gönderir mi?
    pub sends_cancel: bool,
    pub source: ProfileSource,
    /// `progress_extends` iken progressToken varsa Mod A üst süre.
    pub progress_extended_secs: Option<u64>,
    /// Heartbeat aralığı (sn).
    pub progress_heartbeat_secs: Option<u64>,
}

impl ClientProfile {
    /// Kullanıcı/global override tavanı.
    pub fn override_cap_secs(&self) -> u64 {
        match self.hard_limit_secs {
            Some(hard) => hard.saturating_sub(20),
            None if self.name == "unknown" => UNKNOWN_OVERRIDE_CAP_SECS,
            // claude-code: sert sınır bilinmiyor — eşik üstüne çıkma.
            None => self.threshold_secs,
        }
    }

    /// Effective Mod A bekleme: progressToken + progress_extends → extended.
    pub fn effective_threshold_secs(&self, has_progress_token: bool) -> u64 {
        if has_progress_token && self.progress_extends {
            self.progress_extended_secs
                .unwrap_or(self.threshold_secs)
                .max(self.threshold_secs)
        } else {
            self.threshold_secs
        }
    }
}

/// Ölçülmüş / varsayılan profiller (tek kaynak tablo).
pub fn builtin_profiles() -> &'static [ClientProfile] {
    &BUILTIN_PROFILES
}

const BUILTIN_PROFILES: [ClientProfile; 5] = [
    ClientProfile {
        name: "antigravity-client",
        hard_limit_secs: Some(180),
        threshold_secs: 150,
        progress_extends: false,
        sends_cancel: true,
        source: ProfileSource::Measured,
        progress_extended_secs: None,
        progress_heartbeat_secs: None,
    },
    ClientProfile {
        name: "cursor-vscode",
        hard_limit_secs: Some(120),
        threshold_secs: 100,
        progress_extends: true,
        sends_cancel: false,
        source: ProfileSource::Measured,
        progress_extended_secs: Some(CURSOR_PROGRESS_EXTENDED_SECS),
        progress_heartbeat_secs: Some(CURSOR_PROGRESS_HEARTBEAT_SECS),
    },
    ClientProfile {
        name: "claude-ai",
        hard_limit_secs: Some(240),
        threshold_secs: 210,
        progress_extends: false,
        sends_cancel: false,
        source: ProfileSource::Measured,
        progress_extended_secs: None,
        progress_heartbeat_secs: None,
    },
    ClientProfile {
        name: "claude-code",
        hard_limit_secs: None, // ölçülmedi — 3600 yazma
        threshold_secs: 300,
        progress_extends: false,
        sends_cancel: false,
        source: ProfileSource::Assumed,
        progress_extended_secs: None,
        progress_heartbeat_secs: None,
    },
    ClientProfile {
        name: "unknown",
        hard_limit_secs: None,
        threshold_secs: DEFAULT_UNKNOWN_THRESHOLD_SECS,
        progress_extends: false,
        sends_cancel: false,
        source: ProfileSource::Assumed,
        progress_extended_secs: None,
        progress_heartbeat_secs: None,
    },
];

/// `clientInfo.name` → profil (normalize + ham ad eşleşmesi).
pub fn resolve_client_profile(client_name: &str) -> ClientProfile {
    let raw = client_name.trim().to_ascii_lowercase();
    let key = normalize_client_host(client_name);

    if raw.contains("antigravity") || key == "antigravity" || key == "antigravity_client" {
        return BUILTIN_PROFILES[0].clone();
    }
    if raw.contains("cursor") || key == "cursor" || key == "cursor_vscode" {
        return BUILTIN_PROFILES[1].clone();
    }
    // claude-code önce (içinde "claude" var).
    if raw.contains("claude-code")
        || raw.contains("claude_code")
        || key == "claude_code"
        || key == "claude-code"
    {
        return BUILTIN_PROFILES[3].clone();
    }
    // Claude Desktop ölçümü: clientInfo.name = claude-ai.
    if raw.contains("claude-ai")
        || raw.contains("claude_ai")
        || key == "claude_ai"
        || key == "claude_desktop"
        || raw.contains("claude desktop")
        || raw.contains("claude-desktop")
    {
        return BUILTIN_PROFILES[2].clone();
    }
    // Grok Bot / windsurf / vscode / zed / bilinmeyen → unknown (45 sn).
    BUILTIN_PROFILES[4].clone()
}

/// Oturum başına eşik yöneticisi. Tek kaynak: [`TimeoutManager::timeout_limit`].
pub struct TimeoutManager {
    /// Opsiyonel tablo ezmesi (test / ölçüm sonrası); yoksa [`resolve_client_profile`].
    overrides: RwLock<HashMap<String, Duration>>,
    /// Global override saniyesi (env / settings); profil tavanıyla uygulanır.
    global_override_secs: RwLock<Option<u64>>,
    settings_store: RwLock<Option<crate::db::ExperienceStore>>,
}

impl std::fmt::Debug for TimeoutManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeoutManager")
            .field(
                "global_override_secs",
                &*self
                    .global_override_secs
                    .read()
                    .expect("timeout override lock"),
            )
            .field(
                "has_settings_store",
                &self
                    .settings_store
                    .read()
                    .expect("settings store lock")
                    .is_some(),
            )
            .finish()
    }
}

impl Default for TimeoutManager {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl TimeoutManager {
    pub fn with_defaults() -> Self {
        let global = parse_secs_env_raw(ENV_MCP_TIMEOUT_SECS);
        Self {
            overrides: RwLock::new(HashMap::new()),
            global_override_secs: RwLock::new(global),
            settings_store: RwLock::new(None),
        }
    }

    pub fn attach_settings_store(&self, store: crate::db::ExperienceStore) {
        *self.settings_store.write().expect("settings store lock") = Some(store);
    }

    /// Settings / env global override (saniye). `None` veya 0 → override kaldır.
    /// Profil tavanı uygulama anında uygulanır ([`timeout_limit`]).
    pub fn set_global_override_secs(&self, secs: Option<u64>) {
        let mut guard = self
            .global_override_secs
            .write()
            .expect("timeout override lock");
        *guard = secs.filter(|&s| s > 0);
    }

    /// Tek istemci eşiğini test/ölçüm için ez.
    pub fn set_client_timeout(&self, client_name: &str, limit: Duration) {
        let key = normalize_client_host(client_name);
        let mut table = self.overrides.write().expect("timeout table lock");
        table.insert(key, limit);
    }

    pub fn profile_for(&self, client_name: &str) -> ClientProfile {
        resolve_client_profile(client_name)
    }

    /// Bu oturumun kullanacağı eşik. Env > settings > tablo/profil.
    /// Override profil `hard_limit−20` (veya unknown 170) ile kırpılır; uyarı loglanır.
    pub fn timeout_limit(&self, client_name: &str) -> Duration {
        self.timeout_limit_with_progress(client_name, false)
    }

    /// Progress token varken Cursor vb. uzatılmış eşik.
    pub fn timeout_limit_with_progress(
        &self,
        client_name: &str,
        has_progress_token: bool,
    ) -> Duration {
        let profile = resolve_client_profile(client_name);
        let base = profile.effective_threshold_secs(has_progress_token);

        if let Some(raw) = parse_secs_env_raw(ENV_MCP_TIMEOUT_SECS) {
            return Duration::from_secs(cap_override_for_profile(raw, &profile));
        }
        self.try_refresh_settings_cache();
        if let Some(raw) = *self
            .global_override_secs
            .read()
            .expect("timeout override lock")
        {
            return Duration::from_secs(cap_override_for_profile(raw, &profile));
        }
        let key = normalize_client_host(client_name);
        let table = self.overrides.read().expect("timeout table lock");
        if let Some(over) = table.get(&key).copied() {
            return over;
        }
        Duration::from_secs(base)
    }

    fn try_refresh_settings_cache(&self) {
        let Ok(store_guard) = self.settings_store.try_read() else {
            return;
        };
        let Some(store) = store_guard.as_ref() else {
            return;
        };
        let Ok(conn) = store.conn.try_lock() else {
            return;
        };
        let Ok(raw) = conn.query_row(
            "SELECT value_json FROM settings WHERE key = ?1",
            rusqlite::params![SETTING_MCP_TIMEOUT_SECS],
            |row| row.get::<_, String>(0),
        ) else {
            return;
        };
        if let Some(secs) = parse_timeout_setting(&raw) {
            *self
                .global_override_secs
                .write()
                .expect("timeout override lock") = Some(secs);
        }
    }

    pub fn unknown_default(&self) -> Duration {
        Duration::from_secs(DEFAULT_UNKNOWN_THRESHOLD_SECS)
    }

    /// Tanı / status çıktısı.
    pub fn snapshot(&self) -> serde_json::Value {
        let override_secs = *self
            .global_override_secs
            .read()
            .expect("timeout override lock");
        let mut clients = serde_json::Map::new();
        for p in builtin_profiles() {
            clients.insert(
                p.name.to_string(),
                serde_json::json!({
                    "threshold_secs": p.threshold_secs,
                    "hard_limit_secs": p.hard_limit_secs,
                    "progress_extends": p.progress_extends,
                    "sends_cancel": p.sends_cancel,
                    "source": p.source.as_str(),
                    "override_cap_secs": p.override_cap_secs(),
                    "progress_extended_secs": p.progress_extended_secs,
                }),
            );
        }
        serde_json::json!({
            "timeout_limit_source": if override_secs.is_some() { "global_override" } else { "client_profile" },
            "global_override_secs": override_secs,
            "unknown_default_secs": DEFAULT_UNKNOWN_THRESHOLD_SECS,
            "unknown_override_cap_secs": UNKNOWN_OVERRIDE_CAP_SECS,
            "profiles": clients,
            "note": "Cursor: progressToken varken 10sn heartbeat ile Mod A ≤280sn. Diğerlerinde progress süreyi uzatmaz. Bilinmeyen/Grok=45 (Gemini 60 demişti; tutarlılık)."
        })
    }
}

fn parse_secs_env_raw(key: &str) -> Option<u64> {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
}

fn cap_override_for_profile(secs: u64, profile: &ClientProfile) -> u64 {
    let cap = profile.override_cap_secs();
    if secs > cap {
        log::warn!(
            "timeout override {secs}s > profil {} tavanı {cap}s (hard_limit−20 veya unknown 170) — kırpıldı",
            profile.name
        );
        cap
    } else {
        secs
    }
}

/// Settings JSON değerinden saniye oku (`"45"` veya `45`).
pub fn parse_timeout_setting(raw: &str) -> Option<u64> {
    let trimmed = raw.trim().trim_matches('"');
    trimmed.parse::<u64>().ok().filter(|&n| n > 0)
}

/// Yanıt teşhis alanları.
pub fn profile_diag(client_name: &str, timeout_limit: Duration) -> serde_json::Value {
    let p = resolve_client_profile(client_name);
    serde_json::json!({
        "client_profile": p.name,
        "timeout_limit_secs": timeout_limit.as_secs(),
        "profile_source": p.source.as_str(),
        "progress_extends": p.progress_extends,
        "sends_cancel": p.sends_cancel,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn antigravity_threshold_150_hard_180() {
        let tm = TimeoutManager::with_defaults();
        let p = tm.profile_for("antigravity-client");
        assert_eq!(p.name, "antigravity-client");
        assert_eq!(p.threshold_secs, 150);
        assert_eq!(p.hard_limit_secs, Some(180));
        assert!(!p.progress_extends);
        assert!(p.sends_cancel);
        assert_eq!(p.source, ProfileSource::Measured);
        assert_eq!(
            tm.timeout_limit("antigravity-client"),
            Duration::from_secs(150)
        );
        assert_eq!(tm.timeout_limit("Antigravity"), Duration::from_secs(150));
    }

    #[test]
    fn cursor_threshold_100_progress_extends_280() {
        let tm = TimeoutManager::with_defaults();
        let p = tm.profile_for("cursor-vscode");
        assert_eq!(p.name, "cursor-vscode");
        assert_eq!(p.threshold_secs, 100);
        assert_eq!(p.hard_limit_secs, Some(120));
        assert!(p.progress_extends);
        assert!(!p.sends_cancel);
        assert_eq!(p.progress_extended_secs, Some(280));
        assert_eq!(tm.timeout_limit("Cursor"), Duration::from_secs(100));
        assert_eq!(
            tm.timeout_limit_with_progress("cursor-vscode", true),
            Duration::from_secs(280)
        );
        assert_eq!(
            tm.timeout_limit_with_progress("cursor-vscode", false),
            Duration::from_secs(100)
        );
    }

    #[test]
    fn claude_ai_threshold_210() {
        let tm = TimeoutManager::with_defaults();
        assert_eq!(tm.timeout_limit("claude-ai"), Duration::from_secs(210));
        assert_eq!(tm.timeout_limit("Claude Desktop"), Duration::from_secs(210));
        let p = tm.profile_for("claude-ai");
        assert_eq!(p.hard_limit_secs, Some(240));
        assert!(!p.sends_cancel);
        assert_eq!(p.source, ProfileSource::Measured);
    }

    #[test]
    fn claude_code_threshold_300_assumed_no_fake_hard() {
        let tm = TimeoutManager::with_defaults();
        let p = tm.profile_for("claude-code");
        assert_eq!(p.name, "claude-code");
        assert_eq!(p.threshold_secs, 300);
        assert_eq!(p.hard_limit_secs, None);
        assert_eq!(p.source, ProfileSource::Assumed);
        assert_eq!(tm.timeout_limit("claude-code"), Duration::from_secs(300));
    }

    #[test]
    fn unknown_and_grok_get_45() {
        let tm = TimeoutManager::with_defaults();
        assert_eq!(
            tm.timeout_limit("totally-unknown-ide"),
            Duration::from_secs(45)
        );
        assert_eq!(tm.timeout_limit("Grok Bot"), Duration::from_secs(45));
        assert_eq!(tm.profile_for("grok-bot").name, "unknown");
    }

    #[test]
    fn override_capped_per_profile_hard_minus_20() {
        let tm = TimeoutManager::with_defaults();
        tm.set_global_override_secs(Some(300));
        // antigravity: 180-20=160
        assert_eq!(tm.timeout_limit("antigravity"), Duration::from_secs(160));
        // cursor: 120-20=100
        assert_eq!(tm.timeout_limit("cursor"), Duration::from_secs(100));
        // claude-ai: 240-20=220
        assert_eq!(tm.timeout_limit("claude-ai"), Duration::from_secs(220));
        // claude-code: hard yok → threshold 300
        assert_eq!(tm.timeout_limit("claude-code"), Duration::from_secs(300));
        // unknown: 170
        assert_eq!(
            tm.timeout_limit("mystery-client"),
            Duration::from_secs(UNKNOWN_OVERRIDE_CAP_SECS)
        );
    }

    #[test]
    fn override_under_cap_wins() {
        let tm = TimeoutManager::with_defaults();
        tm.set_global_override_secs(Some(12));
        assert_eq!(tm.timeout_limit("antigravity"), Duration::from_secs(12));
        assert_eq!(tm.timeout_limit("Cursor"), Duration::from_secs(12));
        tm.set_global_override_secs(None);
        assert_eq!(tm.timeout_limit("antigravity"), Duration::from_secs(150));
    }

    #[test]
    fn table_update_is_easy() {
        let tm = TimeoutManager::with_defaults();
        tm.set_client_timeout("cursor", Duration::from_secs(55));
        assert_eq!(tm.timeout_limit("Cursor"), Duration::from_secs(55));
    }

    #[test]
    fn parse_timeout_setting_accepts_json_number_string() {
        assert_eq!(parse_timeout_setting("45"), Some(45));
        assert_eq!(parse_timeout_setting("\"90\""), Some(90));
        assert_eq!(parse_timeout_setting("0"), None);
        assert_eq!(parse_timeout_setting("abc"), None);
    }

    #[test]
    fn profile_diag_includes_name_and_threshold() {
        let d = profile_diag("cursor-vscode", Duration::from_secs(100));
        assert_eq!(d["client_profile"], "cursor-vscode");
        assert_eq!(d["timeout_limit_secs"], 100);
    }
}
