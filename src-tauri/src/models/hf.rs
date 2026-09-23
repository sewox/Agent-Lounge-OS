use serde::{Deserialize, Serialize};

pub const MODEL_PULL_EVENT: &str = "model-pull";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceProfile {
    pub total_ram_gb: f32,
    pub available_ram_gb: f32,
    pub usable_budget_gb: f32,
    pub arch: String,
    pub apple_silicon: bool,
    pub metal: bool,
    pub summary: String,
    pub recommended_upper: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HfModelOffer {
    pub id: String,
    pub hf_id: String,
    pub pull_name: String,
    pub name: String,
    pub family: String,
    pub params: String,
    pub estimated_ram_gb: f32,
    pub min_ram_gb: f32,
    pub downloads: u64,
    pub recommended: bool,
    pub installed: bool,
    pub heavy: bool,
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecommendedModels {
    pub device: DeviceProfile,
    pub offers: Vec<HfModelOffer>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PullProgress {
    pub hf_id: String,
    pub pull_name: String,
    pub status: String,
    pub digest: Option<String>,
    pub completed: u64,
    pub total: u64,
    pub done: bool,
    pub error: Option<String>,
}
