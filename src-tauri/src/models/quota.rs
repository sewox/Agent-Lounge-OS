use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolQuota {
    pub id: String,
    pub tool: String,
    pub kind: String,
    pub unit: String,
    pub used: String,
    pub remaining: String,
    pub reset: String,
    pub percent: Option<f32>,
    pub tone: String,
    pub label: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub exhausted: bool,
    #[serde(default)]
    pub access_mode: String,
    #[serde(default)]
    pub host_id: Option<String>,
}

impl Default for ToolQuota {
    fn default() -> Self {
        Self {
            id: String::new(),
            tool: String::new(),
            kind: "local".into(),
            unit: String::new(),
            used: String::new(),
            remaining: String::new(),
            reset: String::new(),
            percent: None,
            tone: "ok".into(),
            label: String::new(),
            source: String::new(),
            exhausted: false,
            access_mode: "local".into(),
            host_id: None,
        }
    }
}

impl ToolQuota {
    pub fn is_exhausted(&self) -> bool {
        self.exhausted || self.percent.map(|value| value >= 100.0).unwrap_or(false)
    }

    pub fn with_mode(mut self, mode: &str, host: Option<&str>) -> Self {
        self.access_mode = mode.to_string();
        self.kind = mode.to_string();
        self.host_id = host.map(str::to_string);
        self
    }
}

pub const AMBER_THRESHOLD: f32 = 80.0;
/// Limit Policy: bu dolulukta (veya exhausted) ücretli ajan ataması engellenir.
pub const LIMIT_POLICY_PERCENT: f32 = 90.0;
pub const QUOTA_EVENT: &str = "quota-update";

/// Atama öncesi kota kararı: allow | block(reason, tool, percent).
#[derive(Debug, Clone, PartialEq)]
pub enum QuotaVerdict {
    Allow,
    Block {
        reason: String,
        tool: String,
        percent: Option<f32>,
    },
}

impl QuotaVerdict {
    pub fn is_blocked(&self) -> bool {
        matches!(self, Self::Block { .. })
    }

    pub fn tool(&self) -> Option<&str> {
        match self {
            Self::Block { tool, .. } => Some(tool.as_str()),
            Self::Allow => None,
        }
    }

    pub fn percent(&self) -> Option<f32> {
        match self {
            Self::Block { percent, .. } => *percent,
            Self::Allow => None,
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Block { reason, .. } => Some(reason.as_str()),
            Self::Allow => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuotaState {
    pub scanned_at: String,
    pub amber_alert: bool,
    pub amber_tools: Vec<String>,
    pub quotas: Vec<ToolQuota>,
}

impl QuotaState {
    pub fn from_quotas(quotas: Vec<ToolQuota>) -> Self {
        let amber_tools: Vec<String> = quotas
            .iter()
            .filter(|row| {
                row.percent
                    .map(|value| value >= AMBER_THRESHOLD)
                    .unwrap_or(false)
            })
            .map(|row| row.id.clone())
            .collect();
        Self {
            scanned_at: crate::models::now_rfc3339(),
            amber_alert: !amber_tools.is_empty(),
            amber_tools,
            quotas,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NatsUiEvent {
    pub id: String,
    pub time: String,
    pub subject: String,
    pub from: String,
    pub to: String,
    pub payload: String,
    pub state: String,
}
