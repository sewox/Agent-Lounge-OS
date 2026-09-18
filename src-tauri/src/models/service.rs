use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceId {
    Ollama,
    Nats,
    MemoryBridge,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceHealth {
    pub id: ServiceId,
    pub name: String,
    pub running: bool,
    pub started_by_us: bool,
    pub endpoint: String,
    pub detail: Option<String>,
    pub error: Option<String>,
}

impl ServiceHealth {
    pub fn down(
        id: ServiceId,
        name: impl Into<String>,
        endpoint: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            running: false,
            started_by_us: false,
            endpoint: endpoint.into(),
            detail: None,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceReport {
    pub ollama: ServiceHealth,
    pub nats: ServiceHealth,
    pub memory: ServiceHealth,
}

impl ServiceReport {
    pub fn all_core_running(&self) -> bool {
        self.ollama.running && self.nats.running
    }
}
