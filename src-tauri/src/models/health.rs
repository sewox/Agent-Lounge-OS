//! Health-1b: versioned `lounge_health` report (agents, title/LMR, dead symbols, CBM, platform).

use serde::{Deserialize, Serialize};

/// Wire schema version for [`LoungeHealthReport`].
pub const LOUNGE_HEALTH_SCHEMA_VERSION: &str = "1.0.0";

/// lounge-mcp entry status (H3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMcpStatus {
    Ok,
    Missing,
    Stale,
    HashMismatch,
    NonCanonical,
    Unreadable,
    NotInstalled,
}

impl AgentMcpStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Missing => "missing",
            Self::Stale => "stale",
            Self::HashMismatch => "hash_mismatch",
            Self::NonCanonical => "non_canonical",
            Self::Unreadable => "unreadable",
            Self::NotInstalled => "not_installed",
        }
    }

    /// stale → warning; hash_mismatch → info; problems → warning; ok → ok.
    pub fn severity(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::HashMismatch => "info",
            Self::Stale
            | Self::Missing
            | Self::NonCanonical
            | Self::Unreadable
            | Self::NotInstalled => "warning",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentHealthEntry {
    pub client: String,
    pub status: AgentMcpStatus,
    pub severity: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TitleHealthReport {
    pub queue_backlog: u64,
    pub db_backlog: u64,
    pub give_ups: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_type: Option<String>,
    pub lmr_reachable: bool,
    pub model_installed: bool,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lmr_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe_error_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DeadSymbolHealthSummary {
    pub kritik: u64,
    pub potansiyel: u64,
    pub total: u64,
    pub ignored: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CbmHealthReport {
    /// Active CBM daemon mode (from CLI probe when available).
    pub daemon_mode: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daemon_presence: Option<String>,
    /// Detect-only: shared config shows Lounge 18749–18759 pollution.
    pub polluted_18749: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PlatformHealthReport {
    pub os: String,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distro_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distro_id_like: Option<String>,
    /// apt | dnf | yum | zypper | pacman | apk | none | unknown | n/a
    pub package_manager: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoungeHealthReport {
    pub schema_version: String,
    pub checked_at: String,
    pub platform: PlatformHealthReport,
    pub agents: Vec<AgentHealthEntry>,
    pub title: TitleHealthReport,
    pub dead_symbols: DeadSymbolHealthSummary,
    pub cbm: CbmHealthReport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_lounge_mcp_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_lounge_mcp_sha256: Option<String>,
}

impl Default for LoungeHealthReport {
    fn default() -> Self {
        Self {
            schema_version: LOUNGE_HEALTH_SCHEMA_VERSION.into(),
            checked_at: String::new(),
            platform: PlatformHealthReport::default(),
            agents: Vec::new(),
            title: TitleHealthReport::default(),
            dead_symbols: DeadSymbolHealthSummary::default(),
            cbm: CbmHealthReport::default(),
            expected_lounge_mcp_version: None,
            expected_lounge_mcp_sha256: None,
        }
    }
}
