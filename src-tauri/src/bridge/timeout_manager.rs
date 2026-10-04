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

/// Settings anahtarı — saniye cinsinden global override (tüm istemciler).
pub const SETTING_MCP_TIMEOUT_SECS: &str = "mcp.timeout_secs";
/// Ortam değişkeni — settings’i de ezer (en yüksek öncelik).
pub const ENV_MCP_TIMEOUT_SECS: &str = "LOUNGE_MCP_TIMEOUT_SECS";

/// Oturum başına eşik yöneticisi. Tek kaynak: [`TimeoutManager::timeout_limit`].
#[derive(Debug)]
pub struct TimeoutManager {
    /// `normalize_client_host` anahtarı → süre.
    table: RwLock<HashMap<String, Duration>>,
    /// Global override (env / settings); `None` → tabloya bak.
    global_override: RwLock<Option<Duration>>,
    unknown_default: Duration,
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

        let global = parse_secs_env(ENV_MCP_TIMEOUT_SECS);
        Self {
            table: RwLock::new(table),
            global_override: RwLock::new(global),
            unknown_default: DEFAULT_UNKNOWN_TIMEOUT,
        }
    }

    /// Settings / env global override (saniye). `None` veya 0 → override kaldır.
    pub fn set_global_override_secs(&self, secs: Option<u64>) {
        let mut guard = self.global_override.write().expect("timeout override lock");
        *guard = secs.filter(|&s| s > 0).map(Duration::from_secs);
    }

    /// Tek istemci eşiğini güncelle (ölçüm sonrası tablo güncellemesi).
    pub fn set_client_timeout(&self, client_name: &str, limit: Duration) {
        let key = normalize_client_host(client_name);
        let mut table = self.table.write().expect("timeout table lock");
        table.insert(key, limit);
    }

    /// Bu oturumun kullanacağı tek eşik değişkeni.
    pub fn timeout_limit(&self, client_name: &str) -> Duration {
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
        .map(Duration::from_secs)
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
