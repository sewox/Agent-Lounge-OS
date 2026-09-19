//! Ollama, NATS ve C-binary (codebase-memory-mcp) yaşam döngüsü.

pub mod autodiscover;
pub mod memory_bridge;
pub mod nats_manager;
pub mod ollama;
mod probe;

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::models::ServiceReport;

pub use memory_bridge::MemoryBridge;
pub use nats_manager::{NatsConfig, NatsService};
pub use ollama::{
    chat_json, embed_model, embed_text, parse_llm_json, OllamaConfig, OllamaService,
    DEFAULT_EMBED_MODEL,
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

        Self {
            ollama: OllamaService::new(),
            nats: NatsService::new(),
            memory,
        }
    }

    pub fn shared() -> SharedServices {
        Arc::new(Mutex::new(Self::new()))
    }

    pub fn memory(&self) -> &MemoryBridge {
        &self.memory
    }

    pub fn nats_url(&self) -> String {
        self.nats.endpoint()
    }

    pub fn ollama_endpoint(&self) -> String {
        self.ollama.endpoint()
    }

    pub async fn ollama_models(&self) -> anyhow::Result<Vec<String>> {
        self.ollama.list_models().await
    }

    pub async fn snapshot(&self) -> ServiceReport {
        let ollama_running = self.ollama.is_healthy().await;
        let nats_running = self.nats.is_healthy().await;
        ServiceReport {
            ollama: self.ollama.snapshot(ollama_running, None, None),
            nats: self.nats.snapshot(nats_running, None, None),
            memory: self.memory.diagnose(),
        }
    }

    /// Ollama ve NATS yerelde yoksa başlatır; mevcut süreçlere dokunmaz.
    pub async fn ensure_all(&mut self) -> ServiceReport {
        let (ollama, nats) = tokio::join!(self.ollama.ensure(), self.nats.ensure());
        if let Some(error) = ollama.error.as_deref() {
            log::error!("Ollama: {error}");
        }
        if let Some(error) = nats.error.as_deref() {
            log::error!("NATS: {error}");
        }

        ServiceReport {
            ollama,
            nats,
            memory: self.memory.diagnose(),
        }
    }
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
            }),
            nats: NatsService::with_config(NatsConfig {
                host: "127.0.0.1".into(),
                port,
                binary: "__missing_nats__".into(),
                args: vec!["-p".into(), port.to_string()],
            }),
            memory: MemoryBridge::from_binary("/tmp/missing-codebase-memory-mcp"),
        };

        let report = manager.ensure_all().await;
        assert!(!report.ollama.running);
        assert!(report.nats.running);
        assert!(!report.memory.running);
        assert!(!report.all_core_running());
    }
}
