use serde::{Deserialize, Serialize};

pub const SERVICE_EVENT: &str = "service-status";

/// Machine-readable optional-runtime marker (LMR / NATS not on disk).
pub const AVAIL_NOT_INSTALLED: &str = "not_installed";

/// Supervisor auto-restart exhausted (`MAX_RESTART_ATTEMPTS`).
pub const CODE_RESTART_EXHAUSTED: &str = "restart_exhausted";
/// Supervisor waiting / mid retry.
pub const CODE_RESTART_RETRYING: &str = "restart_retrying";

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
    /// `"not_installed"` when the optional binary is absent — not a crash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<String>,
    /// Machine-readable supervisor phase (`restart_exhausted` / `restart_retrying`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
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
            availability: None,
            code: None,
        }
    }

    pub fn not_installed(
        id: ServiceId,
        name: impl Into<String>,
        endpoint: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            running: false,
            started_by_us: false,
            endpoint: endpoint.into(),
            detail: Some(detail.into()),
            error: None,
            availability: Some(AVAIL_NOT_INSTALLED.into()),
            code: None,
        }
    }

    pub fn is_not_installed(&self) -> bool {
        self.availability.as_deref() == Some(AVAIL_NOT_INSTALLED)
    }

    pub fn with_restart_code(mut self, code: &str, detail: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self.detail = Some(detail.into());
        self
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

    /// True when a *required-to-be-up* core daemon crashed or hit restart limits.
    /// Missing optional binaries (LMR / NATS not installed) are not degraded.
    pub fn core_degraded(&self) -> bool {
        is_core_failure(&self.ollama) || is_core_failure(&self.nats)
    }

    /// UI banner: crashed core services only (excludes not_installed).
    pub fn degraded_core_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if is_core_failure(&self.ollama) {
            names.push("LMR");
        }
        if is_core_failure(&self.nats) {
            names.push("NATS");
        }
        names
    }

    /// Optional runtimes absent on a fresh install (install guidance, not alarms).
    pub fn optional_missing_names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.ollama.is_not_installed() {
            names.push("LMR");
        }
        if self.nats.is_not_installed() {
            names.push("NATS");
        }
        names
    }
}

fn is_core_failure(health: &ServiceHealth) -> bool {
    !health.running && !health.is_not_installed()
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
            availability: None,
            code: None,
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

    #[test]
    fn not_installed_lmr_is_not_core_degraded() {
        let report = ServiceReport {
            ollama: ServiceHealth::not_installed(
                ServiceId::Ollama,
                "LMR",
                "http://127.0.0.1:18790",
                "optional — place ollama binary in data/lmr",
            ),
            nats: health(ServiceId::Nats, "NATS", true),
            memory: health(ServiceId::MemoryBridge, "Memory", true),
            plugin: health(ServiceId::Plugin, "Plugin", true),
        };
        assert!(!report.core_degraded());
        assert!(report.degraded_core_names().is_empty());
        assert_eq!(report.optional_missing_names(), vec!["LMR"]);
        assert!(report.ollama.is_not_installed());
    }
}
