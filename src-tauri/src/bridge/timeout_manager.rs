//! İstemci bazlı MCP araç zaman aşımı eşiği (`timeout_limit`).
//!
//! Ölçüm (Antigravity IDE, 2026-10-04): sert istemci limiti **180 sn**; progress
//! süreyi uzatmıyor. Antigravity varsayılanı 150 sn (30 sn marj). Cursor / Claude
//! Desktop / diğerleri için geçici 45 sn; bilinmeyen istemci için güvenli düşük
//! varsayılan. Tablo kolay güncellenir; global override env veya settings.

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::Duration;

use super::mcp_server::normalize_client_host;

/// Bilinmeyen / ölçülmemiş istemci — güvenli düşük varsayılan.
pub const DEFAULT_UNKNOWN_TIMEOUT: Duration = Duration::from_secs(30);
/// Cursor / Claude Desktop / Claude Code / Grok — geçici varsayılan (ölçüm bekleniyor).
pub const DEFAULT_GUI_TIMEOUT: Duration = Duration::from_secs(45);
/// Antigravity: 180 sn sert limit − 30 sn marj.
pub const DEFAULT_ANTIGRAVITY_TIMEOUT: Duration = Duration::from_secs(150);
/// Global override üst sınırı (istemci hard-timeout 180 sn).
pub const MAX_TIMEOUT_OVERRIDE_SECS: u64 = 180;

/// Settings anahtarı — saniye cinsinden global override (tüm istemciler).
pub const SETTING_MCP_TIMEOUT_SECS: &str = "mcp.timeout_secs";
/// Ortam değişkeni — settings’i de ezer (en yüksek öncelik).
pub const ENV_MCP_TIMEOUT_SECS: &str = "LOUNGE_MCP_TIMEOUT_SECS";

/// Oturum başına eşik yöneticisi. Tek kaynak: [`TimeoutManager::timeout_limit`].
pub struct TimeoutManager {
    /// `normalize_client_host` anahtarı → süre.
    table: RwLock<HashMap<String, Duration>>,
    /// Global override (env / settings); `None` → tabloya bak.
    global_override: RwLock<Option<Duration>>,
    unknown_default: Duration,
    /// Settings canlı okuma için store (opsiyonel).
    settings_store: RwLock<Option<crate::db::ExperienceStore>>,
}

impl std::fmt::Debug for TimeoutManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeoutManager")
            .field("unknown_default", &self.unknown_default)
            .field(
                "global_override",
                &*self.global_override.read().expect("timeout override lock"),
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
        let mut table = HashMap::new();
        table.insert("antigravity".into(), DEFAULT_ANTIGRAVITY_TIMEOUT);
        // Antigravity clientInfo.name ölçümde `antigravity-client`.
        table.insert("antigravity_client".into(), DEFAULT_ANTIGRAVITY_TIMEOUT);
        table.insert("cursor".into(), DEFAULT_GUI_TIMEOUT);
        table.insert("claude_desktop".into(), DEFAULT_GUI_TIMEOUT);
        table.insert("claude_code".into(), DEFAULT_GUI_TIMEOUT);
        table.insert("grok_bot".into(), DEFAULT_GUI_TIMEOUT);
        // Geçici: henüz ölçülmemiş GUI’ler aynı kovada.
        table.insert("windsurf".into(), DEFAULT_GUI_TIMEOUT);
        table.insert("vscode".into(), DEFAULT_GUI_TIMEOUT);
        table.insert("zed".into(), DEFAULT_GUI_TIMEOUT);

        let global = parse_secs_env(ENV_MCP_TIMEOUT_SECS).map(cap_override);
        Self {
            table: RwLock::new(table),
            global_override: RwLock::new(global),
            unknown_default: DEFAULT_UNKNOWN_TIMEOUT,
            settings_store: RwLock::new(None),
        }
    }

    /// Settings canlı okuma — `timeout_limit` her çağrıda settings’i yeniler.
    pub fn attach_settings_store(&self, store: crate::db::ExperienceStore) {
        *self.settings_store.write().expect("settings store lock") = Some(store);
    }

    /// Settings / env global override (saniye). `None` veya 0 → override kaldır.
    /// 180 sn üstü kırpılır ve uyarılır.
    pub fn set_global_override_secs(&self, secs: Option<u64>) {
        let mut guard = self.global_override.write().expect("timeout override lock");
        *guard = secs.filter(|&s| s > 0).map(cap_override_secs).map(Duration::from_secs);
    }

    /// Tek istemci eşiğini güncelle (ölçüm sonrası tablo güncellemesi).
    pub fn set_client_timeout(&self, client_name: &str, limit: Duration) {
        let key = normalize_client_host(client_name);
        let mut table = self.table.write().expect("timeout table lock");
        table.insert(key, limit);
    }

    /// Bu oturumun kullanacağı tek eşik değişkeni.
    /// Env > settings (canlı) > tablo. Override 180 sn ile sınırlı.
    pub fn timeout_limit(&self, client_name: &str) -> Duration {
        // Env her çağrıda — process env nadiren değişir ama settings’ten yüksek öncelik.
        if let Some(over) = parse_secs_env(ENV_MCP_TIMEOUT_SECS).map(cap_override) {
            return over;
        }
        // Settings canlı yenile.
        if let Some(store) = self.settings_store.read().expect("settings store lock").as_ref() {
            if let Ok(conn) = store.conn.lock() {
                if let Ok(raw) = conn.query_row(
                    "SELECT value_json FROM settings WHERE key = ?1",
                    rusqlite::params![SETTING_MCP_TIMEOUT_SECS],
                    |row| row.get::<_, String>(0),
                ) {
                    if let Some(secs) = parse_timeout_setting(&raw) {
                        let capped = cap_override_secs(secs);
                        if capped != secs {
                            log::warn!(
                                "mcp.timeout_secs={secs} > {MAX_TIMEOUT_OVERRIDE_SECS}; {MAX_TIMEOUT_OVERRIDE_SECS}sn’ye kırpıldı"
                            );
                        }
                        return Duration::from_secs(capped);
                    }
                }
            }
        }
        if let Some(over) = *self.global_override.read().expect("timeout override lock") {
            return over;
        }
        let key = normalize_client_host(client_name);
        let table = self.table.read().expect("timeout table lock");
        table.get(&key).copied().unwrap_or(self.unknown_default)
    }

    pub fn unknown_default(&self) -> Duration {
        self.unknown_default
    }

    /// Tanı / status çıktısı.
    pub fn snapshot(&self) -> serde_json::Value {
        let table = self.table.read().expect("timeout table lock");
        let override_secs = self
            .global_override
            .read()
            .expect("timeout override lock")
            .map(|d| d.as_secs());
        let mut clients = serde_json::Map::new();
        for (k, v) in table.iter() {
            clients.insert(k.clone(), serde_json::json!(v.as_secs()));
        }
        serde_json::json!({
            "timeout_limit_source": if override_secs.is_some() { "global_override" } else { "client_table" },
            "global_override_secs": override_secs,
            "unknown_default_secs": self.unknown_default.as_secs(),
            "clients_secs": clients,
            "note": "Progress notifications do not extend the limit; return before client hard timeout."
        })
    }
}

fn parse_secs_env(key: &str) -> Option<Duration> {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map(cap_override_secs)
        .map(Duration::from_secs)
}

fn cap_override(d: Duration) -> Duration {
    Duration::from_secs(cap_override_secs(d.as_secs()))
}

fn cap_override_secs(secs: u64) -> u64 {
    if secs > MAX_TIMEOUT_OVERRIDE_SECS {
        log::warn!(
            "timeout override {secs}s > {MAX_TIMEOUT_OVERRIDE_SECS}s — kırpıldı"
        );
        MAX_TIMEOUT_OVERRIDE_SECS
    } else {
        secs
    }
}

/// Settings JSON değerinden saniye oku (`"45"` veya `45`).
pub fn parse_timeout_setting(raw: &str) -> Option<u64> {
    let trimmed = raw.trim().trim_matches('"');
    trimmed.parse::<u64>().ok().filter(|&n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn antigravity_gets_150s() {
        let tm = TimeoutManager::with_defaults();
        assert_eq!(
            tm.timeout_limit("antigravity-client"),
            DEFAULT_ANTIGRAVITY_TIMEOUT
        );
        assert_eq!(tm.timeout_limit("Antigravity"), DEFAULT_ANTIGRAVITY_TIMEOUT);
    }

    #[test]
    fn cursor_and_claude_get_45s() {
        let tm = TimeoutManager::with_defaults();
        assert_eq!(tm.timeout_limit("Cursor"), DEFAULT_GUI_TIMEOUT);
        assert_eq!(tm.timeout_limit("Claude Desktop"), DEFAULT_GUI_TIMEOUT);
    }

    #[test]
    fn unknown_gets_safe_low_default() {
        let tm = TimeoutManager::with_defaults();
        assert_eq!(
            tm.timeout_limit("totally-unknown-ide"),
            DEFAULT_UNKNOWN_TIMEOUT
        );
    }

    #[test]
    fn global_override_wins() {
        let tm = TimeoutManager::with_defaults();
        tm.set_global_override_secs(Some(12));
        assert_eq!(tm.timeout_limit("antigravity"), Duration::from_secs(12));
        assert_eq!(tm.timeout_limit("Cursor"), Duration::from_secs(12));
        tm.set_global_override_secs(None);
        assert_eq!(tm.timeout_limit("antigravity"), DEFAULT_ANTIGRAVITY_TIMEOUT);
    }

    #[test]
    fn override_capped_at_180() {
        let tm = TimeoutManager::with_defaults();
        tm.set_global_override_secs(Some(300));
        assert_eq!(
            tm.timeout_limit("antigravity"),
            Duration::from_secs(MAX_TIMEOUT_OVERRIDE_SECS)
        );
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
}
