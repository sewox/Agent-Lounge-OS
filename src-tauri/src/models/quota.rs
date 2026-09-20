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
}

impl ToolQuota {
    pub fn is_exhausted(&self) -> bool {
        self.exhausted || self.percent.map(|value| value >= 100.0).unwrap_or(false)
    }
}

pub const AMBER_THRESHOLD: f32 = 80.0;
pub const QUOTA_EVENT: &str = "quota-state";

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
