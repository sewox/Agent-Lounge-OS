use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, Command};

use super::hf_catalog::{
    gguf_filename_for, hf_gguf_resolve_url, is_hf_redirect_block, is_installed,
    normalize_pull_name, strip_hf_prefix, with_quant_tag,
};
use super::lmr_runtime::ensure_lmr_runtime;
use super::probe::{
    http_endpoint, lounge_lmr_dir, lounge_ollama_host, lounge_ollama_models_dir,
    lounge_ollama_port, wait_until,
};
use crate::models::{PullProgress, ServiceHealth, ServiceId, MODEL_PULL_EVENT};

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

    pub async fn pull_hf_model(&self, app: &AppHandle, hf_id: &str) -> Result<String> {
        self.pull_hf_model_with(hf_id, |progress| emit_pull(app, &progress))
            .await
    }

    pub async fn pull_hf_model_with<F>(&self, hf_id: &str, mut on_progress: F) -> Result<String>
    where
        F: FnMut(PullProgress),
    {
        match pull_hf_model_from(&self.endpoint(), hf_id, &mut on_progress).await {
            Ok(name) => Ok(name),
            Err(err) if is_hf_redirect_block(&err.to_string()) => {
                log::warn!(
                    "LMR Hugging Face CDN yönlendirmesini reddetti, GGUF doğrudan indirilecek: {err}"
                );
                import_hf_gguf(&self.config, hf_id, &mut on_progress).await
            }
            Err(err) => Err(err),
        }
    }

    async fn fetch_models(&self) -> Result<Vec<String>> {
        probe_ollama_models(&self.endpoint()).await
    }

    fn reap_exited_child(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                log::error!("kritik servis down: LMR süreci sonlandı ({status})");
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

pub async fn pull_hf_model_from<F>(
    endpoint: &str,
    hf_id: &str,
    on_progress: &mut F,
) -> Result<String>
where
    F: FnMut(PullProgress),
{
    let pull_name = normalize_pull_name(hf_id)?;
    let hf_id = pull_name
        .strip_prefix("hf.co/")
        .unwrap_or(pull_name.as_str())
        .to_string();
    let mut progress = PullProgress {
        hf_id: hf_id.clone(),
        pull_name: pull_name.clone(),
        status: "çekiliyor".into(),
        ..PullProgress::default()
    };
    on_progress(progress.clone());

    let url = format!("{}/api/pull", endpoint.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .context("LMR pull istemcisi kurulamadı")?;
    let response = client
        .post(&url)
        .json(&serde_json::json!({
            "model": pull_name,
            "name": pull_name,
            "stream": true
        }))
        .send()
        .await
        .with_context(|| format!("LMR pull başarısız: {url}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        progress.done = true;
        progress.error = Some(format!("LMR pull HTTP {status}: {body}"));
        on_progress(progress.clone());
        bail!(
            "LMR pull HTTP {status}: {}",
            progress.error.clone().unwrap_or_default()
        );
    }

    let mut buffer = String::new();
    let mut stream = response;
    let mut succeeded = false;
    loop {
        match stream.chunk().await {
            Ok(Some(chunk)) => {
                buffer.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(idx) = buffer.find('\n') {
                    let line = buffer[..idx].trim().to_string();
                    buffer.drain(..=idx);
                    if line.is_empty() {
                        continue;
                    }
                    match apply_pull_line(&mut progress, &line) {
                        ApplyPull::Continue => on_progress(progress.clone()),
                        ApplyPull::Success => {
                            succeeded = true;
                            progress.done = true;
                            progress.status = "success".into();
                            on_progress(progress.clone());
                        }
                        ApplyPull::Failed(err) => {
                            progress.done = true;
                            progress.error = Some(err.clone());
                            on_progress(progress.clone());
                            bail!("{err}");
                        }
                    }
                }
            }
            Ok(None) => break,
            Err(err) => {
                progress.done = true;
                progress.error = Some(err.to_string());
                on_progress(progress.clone());
                return Err(err.into());
            }
        }
        if succeeded {
            break;
        }
    }

    if !succeeded {
        if !buffer.trim().is_empty() {
            match apply_pull_line(&mut progress, buffer.trim()) {
                ApplyPull::Success => succeeded = true,
                ApplyPull::Failed(err) => {
                    progress.done = true;
                    progress.error = Some(err.clone());
                    on_progress(progress.clone());
                    bail!("{err}");
                }
                ApplyPull::Continue => {}
            }
        }
        if !succeeded {
            progress.done = true;
            progress.error = Some("LMR pull tamamlanmadı".into());
            on_progress(progress.clone());
            bail!("LMR pull tamamlanmadı");
        }
    }

    let tags = probe_ollama_models(endpoint).await.unwrap_or_default();
    Ok(resolve_installed_name(&hf_id, &pull_name, &tags))
}

#[derive(Debug)]
enum ApplyPull {
    Continue,
    Success,
    Failed(String),
}

fn apply_pull_line(progress: &mut PullProgress, line: &str) -> ApplyPull {
    let value: serde_json::Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(_) => return ApplyPull::Continue,
    };
    if let Some(err) = value.get("error").and_then(|v| v.as_str()) {
        if !err.is_empty() {
            return ApplyPull::Failed(err.to_string());
        }
    }
    if let Some(status) = value.get("status").and_then(|v| v.as_str()) {
        progress.status = status.to_string();
        if status.eq_ignore_ascii_case("success") {
            return ApplyPull::Success;
        }
    }
    if let Some(digest) = value.get("digest").and_then(|v| v.as_str()) {
        progress.digest = Some(digest.to_string());
    }
    if let Some(total) = value.get("total").and_then(|v| v.as_u64()) {
        progress.total = total;
    }
    if let Some(completed) = value.get("completed").and_then(|v| v.as_u64()) {
        progress.completed = completed;
    }
    ApplyPull::Continue
}

fn resolve_installed_name(hf_id: &str, pull_name: &str, tags: &[String]) -> String {
    if is_installed(hf_id, pull_name, tags) {
        tags.iter()
            .find(|tag| is_installed(hf_id, pull_name, std::slice::from_ref(*tag)))
            .cloned()
            .unwrap_or_else(|| pull_name.to_string())
    } else {
        pull_name.to_string()
    }
}

async fn import_hf_gguf<F>(
    config: &OllamaConfig,
    hf_id: &str,
    on_progress: &mut F,
) -> Result<String>
where
    F: FnMut(PullProgress),
{
    let repo = strip_hf_prefix(hf_id).to_string();
    let pull_name = with_quant_tag(&normalize_pull_name(&repo)?, "Q4_K_M");
    let filename = gguf_filename_for(&repo);
    let mut progress = PullProgress {
        hf_id: repo.clone(),
        pull_name: pull_name.clone(),
        status: format!("Hugging Face GGUF indiriliyor ({filename})"),
        ..PullProgress::default()
    };
    on_progress(progress.clone());

    let imports = lounge_lmr_dir().join("imports");
    tokio::fs::create_dir_all(&imports)
        .await
        .with_context(|| format!("import dizini oluşturulamadı: {}", imports.display()))?;
    let dest = imports.join(&filename);
    download_hf_gguf(&repo, &filename, &dest, &mut progress, on_progress).await?;

    progress.status = "LMR'ye aktarılıyor (ollama create)".into();
    on_progress(progress.clone());
    ollama_create_from_gguf(config, &pull_name, &dest).await?;
    let _ = tokio::fs::remove_file(&dest).await;

    let tags = probe_ollama_models(&http_endpoint(&config.host, config.port))
        .await
        .unwrap_or_default();
    progress.done = true;
    progress.status = "success".into();
    on_progress(progress.clone());
    Ok(resolve_installed_name(&repo, &pull_name, &tags))
}

async fn download_hf_gguf<F>(
    hf_id: &str,
    filename: &str,
    dest: &Path,
    progress: &mut PullProgress,
    on_progress: &mut F,
) -> Result<()>
where
    F: FnMut(PullProgress),
{
    let url = hf_gguf_resolve_url(hf_id, filename);
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(20))
        .user_agent("Agent-Lounge-OS/0.1 (LMR)")
        .build()
        .context("HF indirme istemcisi kurulamadı")?;
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("Hugging Face GGUF alınamadı: {url}"))?;
    if !response.status().is_success() {
        bail!("Hugging Face GGUF HTTP {} ({url})", response.status());
    }
    progress.total = response.content_length().unwrap_or(0);
    progress.completed = 0;
    on_progress(progress.clone());

    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("GGUF yazılamadı: {}", dest.display()))?;
    let mut stream = response;
    while let Some(chunk) = stream.chunk().await.context("GGUF indirme kesildi")? {
        file.write_all(&chunk).await?;
        progress.completed = progress.completed.saturating_add(chunk.len() as u64);
        on_progress(progress.clone());
    }
    file.flush().await?;
    if progress.total == 0 {
        progress.total = progress.completed;
    }
    Ok(())
}

async fn ollama_create_from_gguf(config: &OllamaConfig, model: &str, gguf: &Path) -> Result<()> {
    let binary = ensure_lmr_runtime(&config.binary).await?;
    let gguf = gguf
        .canonicalize()
        .with_context(|| format!("GGUF yolu çözülemedi: {}", gguf.display()))?;
    let modelfile_path = gguf.with_extension("Modelfile");
    tokio::fs::write(&modelfile_path, format!("FROM {}\n", gguf.display()))
        .await
        .context("Modelfile yazılamadı")?;

    let mut command = Command::new(&binary);
    command
        .arg("create")
        .arg(model)
        .arg("-f")
        .arg(&modelfile_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in private_env(config) {
        command.env(key, value);
    }
    apply_no_window(&mut command);
    let output = command
        .output()
        .await
        .with_context(|| format!("ollama create başlatılamadı: {}", binary.display()))?;
    let _ = tokio::fs::remove_file(&modelfile_path).await;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "LMR create başarısız ({}): {}",
            output.status,
            [stderr.trim(), stdout.trim()]
                .into_iter()
                .find(|s| !s.is_empty())
                .unwrap_or("çıktı yok")
        );
    }
    Ok(())
}

fn emit_pull(app: &AppHandle, progress: &PullProgress) {
    match app.get_webview_window("main") {
        Some(window) => {
            if let Err(err) = window.emit(MODEL_PULL_EVENT, progress) {
                log::debug!("{MODEL_PULL_EVENT} emit: {err}");
            }
        }
        None => {
            if let Err(err) = app.emit(MODEL_PULL_EVENT, progress) {
                log::debug!("{MODEL_PULL_EVENT} emit: {err}");
            }
        }
    }
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
        // tokio::process::Command exposes creation_flags on Windows directly.
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
    async fn pull_rejects_empty_id_before_http() {
        let err = pull_hf_model_from("http://127.0.0.1:1", "", &mut |_| {})
            .await
            .unwrap_err();
        assert!(err.to_string().contains("boş"));
    }

    #[tokio::test]
    async fn pull_name_must_start_with_hf_co() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let body = std::str::from_utf8(&buf).unwrap_or("");
                assert!(body.contains("hf.co/bartowski/Llama-3.2-1B-Instruct-GGUF"));
                let ndjson = "{\"status\":\"pulling manifest\"}\n{\"status\":\"success\"}\n";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ndjson}",
                    ndjson.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        let name = pull_hf_model_from(
            &format!("http://127.0.0.1:{port}"),
            "bartowski/Llama-3.2-1B-Instruct-GGUF",
            &mut |_| {},
        )
        .await
        .unwrap();
        assert!(name.starts_with("hf.co/"));
    }

    #[test]
    fn pull_progress_parses_ndjson_status() {
        let mut progress = PullProgress::default();
        apply_pull_line(
            &mut progress,
            r#"{"status":"downloading","digest":"sha256:abc","total":100,"completed":40}"#,
        );
        assert_eq!(progress.status, "downloading");
        assert_eq!(progress.completed, 40);
        assert_eq!(progress.total, 100);
        let result = apply_pull_line(&mut progress, r#"{"status":"success"}"#);
        assert!(matches!(result, ApplyPull::Success));
    }

    #[test]
    fn pull_error_detects_blocked_cdn_redirect() {
        let mut progress = PullProgress::default();
        let result = apply_pull_line(
            &mut progress,
            r#"{"error":"Head \"https://us.aws.cdn.hf.co/xet-bridge-us/abc\": blocked redirect to a different host"}"#,
        );
        match result {
            ApplyPull::Failed(err) => assert!(is_hf_redirect_block(&err)),
            other => panic!("expected Failed, got {other:?}"),
        }
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
