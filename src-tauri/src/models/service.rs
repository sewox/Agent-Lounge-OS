use serde::{Deserialize, Serialize};

pub const SERVICE_EVENT: &str = "service-status";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceId {
    Ollama,
    Nats,
    MemoryBridge,
    Plugin,
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
    pub plugin: ServiceHealth,
}

impl ServiceReport {
    pub fn all_core_running(&self) -> bool {
        self.ollama.running && self.nats.running
    }

    /// NATS veya LMR (Lounge Model Runner) düşmüşse çekirdek degraded.
    pub fn core_degraded(&self) -> bool {
        !self.all_core_running()
    }

    /// UI banner için düşmüş çekirdek servis adları (LMR / NATS).
    pub fn degraded_core_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if !self.ollama.running {
            names.push("LMR");
        }
        if !self.nats.running {
            names.push("NATS");
        }
        names
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health(id: ServiceId, name: &str, running: bool) -> ServiceHealth {
        ServiceHealth {
            id,
            name: name.into(),
            running,
            started_by_us: false,
            endpoint: "http://127.0.0.1:0".into(),
            detail: None,
            error: if running { None } else { Some("down".into()) },
        }
    }

    #[test]
    fn degraded_detection_lists_down_core_services() {
        let report = ServiceReport {
            ollama: health(ServiceId::Ollama, "LMR", false),
            nats: health(ServiceId::Nats, "NATS", true),
            memory: health(ServiceId::MemoryBridge, "Memory", true),
            plugin: health(ServiceId::Plugin, "Plugin", true),
        };
        assert!(report.core_degraded());
        assert_eq!(report.degraded_core_names(), vec!["LMR"]);

        let both = ServiceReport {
            ollama: health(ServiceId::Ollama, "LMR", false),
            nats: health(ServiceId::Nats, "NATS", false),
            memory: health(ServiceId::MemoryBridge, "Memory", true),
            plugin: health(ServiceId::Plugin, "Plugin", true),
        };
        assert_eq!(both.degraded_core_names(), vec!["LMR", "NATS"]);
        assert!(!both.all_core_running());
    }
}
