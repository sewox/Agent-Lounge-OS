use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::process::{Child, Command};

use super::lmr_runtime::ensure_lmr_runtime;
use super::probe::{
    http_endpoint, lounge_lmr_dir, lounge_ollama_host, lounge_ollama_models_dir,
    lounge_ollama_port, wait_until,
};
use crate::models::{ServiceHealth, ServiceId};

const SERVICE_NAME: &str = "LMR";

const HEALTH_TIMEOUT: Duration = Duration::from_secs(3);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct OllamaConfig {
    pub host: String,
    pub port: u16,
    pub binary: String,
    pub args: Vec<String>,
    pub models_dir: Option<PathBuf>,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            host: lounge_ollama_host(),
            port: lounge_ollama_port(),
            binary: "ollama".to_string(),
            args: vec!["serve".to_string()],
            models_dir: Some(lounge_ollama_models_dir()),
        }
    }
}

/// Kapalı devre süreç: loopback + Lounge portu + izole model deposu.
pub fn private_env(config: &OllamaConfig) -> Vec<(String, String)> {
    let mut env = vec![(
        "OLLAMA_HOST".into(),
        format!("{}:{}", config.host, config.port),
    )];
    if let Some(dir) = &config.models_dir {
        env.push(("OLLAMA_MODELS".into(), dir.display().to_string()));
    }
    env
}

pub struct OllamaService {
    config: OllamaConfig,
    child: Option<Child>,
    started_by_us: bool,
}

impl OllamaService {
    pub fn new() -> Self {
        Self::with_config(OllamaConfig::default())
    }

    pub fn with_config(config: OllamaConfig) -> Self {
        Self {
            config,
            child: None,
            started_by_us: false,
        }
    }

    pub fn endpoint(&self) -> String {
        http_endpoint(&self.config.host, self.config.port)
    }

    pub async fn is_healthy(&self) -> bool {
        self.fetch_models().await.is_ok()
    }

    pub async fn ensure(&mut self) -> ServiceHealth {
        match self.ensure_inner().await {
            Ok(health) => health,
            Err(err) => ServiceHealth::down(
                ServiceId::Ollama,
                SERVICE_NAME,
                self.endpoint(),
                err.to_string(),
            ),
        }
    }

    pub fn snapshot(
        &self,
        running: bool,
        detail: Option<String>,
        error: Option<String>,
    ) -> ServiceHealth {
        ServiceHealth {
            id: ServiceId::Ollama,
            name: SERVICE_NAME.into(),
            running,
            started_by_us: self.started_by_us,
            endpoint: self.endpoint(),
            detail,
            error,
        }
    }

    async fn ensure_inner(&mut self) -> Result<ServiceHealth> {
        self.reap_exited_child();

        if let Ok(models) = self.fetch_models().await {
            return Ok(self.snapshot(true, models_detail(&models), None));
        }

        let binary = ensure_lmr_runtime(&self.config.binary)
            .await
            .with_context(|| format!("LMR runtime yok. uç nokta: {}", self.endpoint()))?;

        if let Some(dir) = &self.config.models_dir {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("LMR model dizini oluşturulamadı: {}", dir.display()))?;
        }

        let mut command = Command::new(&binary);
        command
            .args(&self.config.args)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        if let Some(dir) = binary.parent() {
            command.current_dir(dir);
        }
        attach_lmr_log(&mut command);
        for (key, value) in private_env(&self.config) {
            command.env(key, value);
        }
        apply_no_window(&mut command);

        let child = command
            .spawn()
            .with_context(|| format!("LMR başlatılamadı: {}", binary.display()))?;
        self.child = Some(child);
        self.started_by_us = true;
        log::info!(
            "LMR spawn edildi: {} → {}",
            binary.display(),
            self.endpoint()
        );

        let endpoint = self.endpoint();
        let ready = wait_until(STARTUP_TIMEOUT, POLL_INTERVAL, || {
            let endpoint = endpoint.clone();
            async move { probe_ollama_models(&endpoint).await.is_ok() }
        })
        .await;
        if !ready {
            self.kill_child().await;
            anyhow::bail!(
                "LMR {timeout:?} içinde {endpoint} üzerinde ayağa kalkmadı",
                timeout = STARTUP_TIMEOUT,
                endpoint = self.endpoint()
            );
        }

        let models = self.fetch_models().await.unwrap_or_default();
        Ok(self.snapshot(true, models_detail(&models), None))
    }

    pub async fn list_models(&self) -> Result<Vec<String>> {
        probe_ollama_models(&self.endpoint()).await
    }

    async fn fetch_models(&self) -> Result<Vec<String>> {
        probe_ollama_models(&self.endpoint()).await
    }

    fn reap_exited_child(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                log::warn!("LMR süreci sonlandı: {status}");
                self.child = None;
                self.started_by_us = false;
            }
        }
    }

    async fn kill_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
        }
        self.started_by_us = false;
    }
}

impl Default for OllamaService {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Default, Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagModel>,
}

#[derive(Debug, Default, Deserialize)]
struct TagModel {
    name: Option<String>,
}

async fn probe_ollama_models(endpoint: &str) -> Result<Vec<String>> {
    let url = format!("{endpoint}/api/tags");
    let client = reqwest::Client::builder()
        .timeout(HEALTH_TIMEOUT)
        .build()
        .context("Ollama HTTP istemcisi kurulamadı")?;

    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Ollama sağlık kontrolü başarısız: {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!("Ollama HTTP {}", response.status());
    }

    let payload: TagsResponse = response.json().await.unwrap_or_default();
    Ok(payload
        .models
        .into_iter()
        .filter_map(|model| model.name)
        .collect())
}

fn models_detail(models: &[String]) -> Option<String> {
    if models.is_empty() {
        return Some("sunucu aktif".into());
    }
    Some(format!("modeller: {}", models.join(", ")))
}

pub const DEFAULT_EMBED_MODEL: &str = "nomic-embed-text";

pub fn embed_model() -> String {
    std::env::var("LOUNGE_EMBED_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_EMBED_MODEL.to_string())
}

/// Ollama `/api/embed` (yedek: `/api/embeddings`). Başarısızsa hata döner; çağıran lexical'e düşer.
pub async fn embed_text(endpoint: &str, model: &str, text: &str) -> Result<Vec<f32>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .context("Ollama embed istemcisi kurulamadı")?;

    let body = serde_json::json!({
        "model": model,
        "input": text,
        "prompt": text
    });

    for path in ["/api/embed", "/api/embeddings"] {
        let url = format!("{endpoint}{path}");
        let response = match client.post(&url).json(&body).send().await {
            Ok(response) => response,
            Err(_) => continue,
        };
        if !response.status().is_success() {
            continue;
        }
        let payload: serde_json::Value = match response.json().await {
            Ok(value) => value,
            Err(_) => continue,
        };
        if let Some(vector) = parse_embedding(&payload) {
            return Ok(vector);
        }
    }
    anyhow::bail!("Ollama embedding alınamadı ({model})")
}

fn parse_embedding(payload: &serde_json::Value) -> Option<Vec<f32>> {
    if let Some(values) = payload.get("embedding").and_then(|v| v.as_array()) {
        return values
            .iter()
            .map(|v| v.as_f64().map(|n| n as f32))
            .collect::<Option<Vec<_>>>();
    }
    payload
        .get("embeddings")
        .and_then(|v| v.as_array())
        .and_then(|rows| rows.first())
        .and_then(|row| row.as_array())
        .and_then(|values| {
            values
                .iter()
                .map(|v| v.as_f64().map(|n| n as f32))
                .collect::<Option<Vec<_>>>()
        })
}

pub async fn chat_json(
    endpoint: &str,
    model: &str,
    system: &str,
    user: &str,
) -> Result<serde_json::Value> {
    let url = format!("{endpoint}/api/chat");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("Ollama chat istemcisi kurulamadı")?;

    let body = serde_json::json!({
        "model": model,
        "stream": false,
        "format": "json",
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user}
        ]
    });

    let response = client
        .post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("Ollama chat başarısız: {url}"))?;

    if !response.status().is_success() {
        anyhow::bail!("Ollama chat HTTP {}", response.status());
    }

    let payload: serde_json::Value = response
        .json()
        .await
        .context("Ollama chat JSON okunamadı")?;
    let content = payload
        .pointer("/message/content")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("Ollama yanıtında message.content yok"))?;

    parse_llm_json(content)
}

pub fn parse_llm_json(raw: &str) -> Result<serde_json::Value> {
    let trimmed = raw.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|body| body.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    serde_json::from_str(unfenced).with_context(|| format!("LLM JSON parse edilemedi: {unfenced}"))
}

fn apply_no_window(_command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        _command.creation_flags(CREATE_NO_WINDOW);
    }
}

fn attach_lmr_log(command: &mut Command) {
    let path = lounge_lmr_dir().join("serve.log");
    let _ = std::fs::create_dir_all(lounge_lmr_dir());
    match std::fs::File::create(&path) {
        Ok(file) => match file.try_clone() {
            Ok(clone) => {
                command.stdout(Stdio::from(file));
                command.stderr(Stdio::from(clone));
            }
            Err(_) => {
                command.stdout(Stdio::from(file));
                command.stderr(Stdio::null());
            }
        },
        Err(_) => {
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn serve_tags(body: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 512];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        port
    }

    #[tokio::test]
    async fn treats_api_tags_as_healthy_like_echomind() {
        let port = serve_tags(r#"{"models":[{"name":"llama3.1:latest"}]}"#).await;
        let mut service = OllamaService::with_config(OllamaConfig {
            host: "127.0.0.1".into(),
            port,
            binary: "__missing_ollama__".into(),
            args: vec!["serve".into()],
            models_dir: None,
        });

        let health = service.ensure().await;
        assert!(health.running);
        assert!(!health.started_by_us);
        assert!(health.detail.unwrap().contains("llama3.1"));
        assert_eq!(health.endpoint, format!("http://127.0.0.1:{port}"));
    }

    #[tokio::test]
    async fn reports_missing_binary_when_unhealthy() {
        let mut service = OllamaService::with_config(OllamaConfig {
            host: "127.0.0.1".into(),
            port: 1,
            binary: "__missing_ollama__".into(),
            args: vec!["serve".into()],
            models_dir: None,
        });
        let health = service.ensure().await;
        assert!(!health.running);
        assert!(health.error.unwrap().contains("LMR runtime yok"));
    }

    #[test]
    fn parses_fenced_chat_json() {
        let value = parse_llm_json("```json\n{\"intent\":\"dispatch\"}\n```").unwrap();
        assert_eq!(value["intent"], "dispatch");
    }

    #[test]
    fn default_config_is_closed_circuit_not_system_11434() {
        let config = OllamaConfig::default();
        assert_ne!(config.port, super::super::probe::SYSTEM_OLLAMA_PORT);
        assert_eq!(config.host, "127.0.0.1");
        let env = private_env(&config);
        let host = env
            .iter()
            .find(|(key, _)| key == "OLLAMA_HOST")
            .map(|(_, value)| value.as_str())
            .unwrap();
        assert!(host.ends_with(&format!(":{}", config.port)));
        assert!(!host.ends_with(":11434"));
        assert!(env.iter().any(|(key, _)| key == "OLLAMA_MODELS"));
    }

    #[tokio::test]
    async fn does_not_treat_a_different_port_as_healthy() {
        let system_port = serve_tags(r#"{"models":[{"name":"system-only"}]}"#).await;
        let mut service = OllamaService::with_config(OllamaConfig {
            host: "127.0.0.1".into(),
            port: 1,
            binary: "__missing_ollama__".into(),
            args: vec!["serve".into()],
            models_dir: None,
        });
        let health = service.ensure().await;
        assert!(!health.running);
        assert_ne!(health.endpoint, format!("http://127.0.0.1:{system_port}"));
    }
}
