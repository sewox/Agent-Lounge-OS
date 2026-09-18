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
