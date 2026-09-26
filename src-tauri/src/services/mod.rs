//! LMR (Lounge Model Runner), NATS ve C-binary yaşam döngüsü.

pub mod autodiscover;
pub mod graph_ui;
pub mod hardware;
pub mod hf_catalog;
pub mod lmr_runtime;
pub mod memory_bridge;
pub mod model_manager;
pub mod nats_manager;
pub mod ollama;
pub mod plugin;
mod probe;
pub mod quota_manager;
pub mod subscription_usage;
pub mod supervisor;
pub mod telemetry;

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::models::ServiceReport;

pub use graph_ui::{
    enable_graph_ui, graph_ui_status, load_port_from_store, on_main_window_closed,
    open_or_focus_graph_window, persist_port, resolve_cbm_project_name, GraphUiState,
    GraphUiStatus, GRAPH_WINDOW_LABEL,
};
pub use memory_bridge::{
    probe_ui_config, MemoryBridge, MemoryBridgeConfig, TransportMode, DEFAULT_GRAPH_UI_PORT,
};
pub use model_manager::{LayaEnginePhase, LayaEngineStatus, ModelManager, LAYA_ENGINE_EVENT};
pub use nats_manager::{spawn_event_pump, NatsConfig, NatsService};
pub use ollama::{
    chat_json, embed_model, embed_text, parse_llm_json, private_env, OllamaConfig, OllamaService,
    DEFAULT_EMBED_MODEL,
};
pub use plugin::{lounge_workspace, plugin_health, scan_plugin_catalog, PluginCatalog};
pub use probe::{
    lounge_laya_dir, lounge_ollama_endpoint, nats_monitor_endpoint, system_ollama_endpoint,
    LOUNGE_OLLAMA_PORT, SYSTEM_OLLAMA_PORT,
};
pub use quota_manager::{
    api_keys_from_store, collect_quota_state, collect_quota_state_with_keys, evaluate_assignment,
    is_quota_approval, limit_policy_percent, lmr_endpoint_up, lmr_runtime_up, normalize_lmr_agent,
    quota_alert_envelope, quota_approval_request, quota_blocked_for, quota_exhausted_for,
    quota_matches_agent, spawn_quota_pump, QuotaAlertPayload, QUOTA_ALERT_PROMPT,
    QUOTA_CONTINUE_LOCAL_LABEL,
};
pub use supervisor::{max_restart_attempts, next_backoff, spawn_supervisor};
pub use telemetry::{
    build_agent_efficiency_report, record_dead_snapshot, record_whisper_injection,
    AgentEfficiencyReport, EfficiencyReportQuery,
};

/// Paylaşılan, thread-safe servis yöneticisi (EchoMind `Arc<Mutex<T>>` kalıbı).
pub type SharedServices = Arc<Mutex<ServiceManager>>;

/// Yerel Ollama ve NATS süreçlerini denetler; yoksa başlatır.
pub struct ServiceManager {
    ollama: OllamaService,
    nats: NatsService,
    memory: MemoryBridge,
}

impl ServiceManager {
    pub fn new() -> Self {
        let memory = MemoryBridge::discover().unwrap_or_else(|err| {
            log::warn!("{err}");
            MemoryBridge::from_binary("codebase-memory-mcp")
        });
        Self::with_memory(memory)
    }

    pub fn with_memory(memory: MemoryBridge) -> Self {
        Self {
            ollama: OllamaService::new(),
            nats: NatsService::new(),
            memory,
        }
    }

    pub fn shared() -> SharedServices {
        Arc::new(Mutex::new(Self::new()))
    }

    pub fn shared_with_memory(memory: MemoryBridge) -> SharedServices {
        Arc::new(Mutex::new(Self::with_memory(memory)))
    }

    /// Test / stub: gerçek daemon olmadan health + recovery yollarını doğrula.
    #[cfg(test)]
    pub(crate) fn for_test(ollama: OllamaService, nats: NatsService, memory: MemoryBridge) -> Self {
        Self {
            ollama,
            nats,
            memory,
        }
    }

    pub fn memory(&self) -> &MemoryBridge {
        &self.memory
    }

    pub fn nats_url(&self) -> String {
        self.nats.endpoint()
    }

    pub fn nats_monitor_url(&self) -> String {
        self.nats.monitor_url()
    }

    pub fn ollama_endpoint(&self) -> String {
        self.ollama.endpoint()
    }

    pub async fn ollama_models(&self) -> anyhow::Result<Vec<String>> {
        self.ollama.list_models().await
    }

    pub async fn pull_hf_model(
        &mut self,
        app: &tauri::AppHandle,
        hf_id: &str,
    ) -> anyhow::Result<String> {
        let health = self.ollama.ensure().await;
        if !health.running {
            anyhow::bail!(
                "{}",
                health.error.unwrap_or_else(|| "LMR ayakta değil".into())
            );
        }
        self.ollama.pull_hf_model(app, hf_id).await
    }

    pub async fn snapshot(&self) -> ServiceReport {
        let ollama_running = self.ollama.is_healthy().await;
        let nats_running = self.nats.is_healthy().await;
        ServiceReport {
            ollama: self.ollama.snapshot(ollama_running, None, None),
            nats: self.nats.snapshot(nats_running, None, None),
            memory: self.memory.diagnose(),
            plugin: plugin_snapshot(),
        }
    }

    /// LMR'yi kapalı devrede ayağa kaldırır; host Ollama :11434 örneğine dokunmaz.
    pub async fn ensure_all(&mut self) -> ServiceReport {
        let (ollama, nats) = tokio::join!(self.ollama.ensure(), self.nats.ensure());
        if let Some(error) = ollama.error.as_deref() {
            log::error!("LMR: {error}");
        }
        if let Some(error) = nats.error.as_deref() {
            log::error!("NATS: {error}");
        }

        ServiceReport {
            ollama,
            nats,
            memory: self.memory.diagnose(),
            plugin: plugin_snapshot(),
        }
    }
}

fn plugin_snapshot() -> crate::models::ServiceHealth {
    plugin_health(&scan_plugin_catalog(&lounge_workspace()))
}

impl Default for ServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::nats_manager::NatsConfig;
    use crate::services::ollama::OllamaConfig;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn ensure_all_reports_both_services() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();

        let mut manager = ServiceManager {
            ollama: OllamaService::with_config(OllamaConfig {
                host: "127.0.0.1".into(),
                port: 1,
                binary: "__missing_ollama__".into(),
                args: vec!["serve".into()],
                models_dir: None,
            }),
            nats: NatsService::with_config(NatsConfig {
                host: "127.0.0.1".into(),
                port,
                binary: "__missing_nats__".into(),
                args: vec!["-p".into(), port.to_string()],
                ..NatsConfig::default()
            }),
            memory: MemoryBridge::from_binary("/tmp/missing-codebase-memory-mcp"),
        };

        let report = manager.ensure_all().await;
        assert!(!report.ollama.running);
        assert!(report.nats.running);
        assert!(!report.memory.running);
        assert!(report.plugin.running);
        assert!(!report.all_core_running());
    }
}
