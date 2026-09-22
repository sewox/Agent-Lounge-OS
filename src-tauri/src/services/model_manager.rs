//! Laya ağırlık yöneticisi: Hugging Face → OS app-support, SHA-256, ilerleme.
//!
//! Üretim dizini (kullanıcı kimliği **AgentLounge**, Tauri `productName` değil):
//! - macOS: `~/Library/Application Support/AgentLounge/models/laya`
//! - Linux: `$XDG_DATA_HOME/AgentLounge/models/laya` veya `~/.local/share/AgentLounge/models/laya`
//! - Windows: `%APPDATA%\AgentLounge\models\laya`
//!
//! `LOUNGE_LAYA_DIR` tam Laya dizinini ezer. Repo `data/laya` yalnızca mevcut
//! yerel kopya varsa dev fallback’tir; varsayılan indirme hedefi app-support’tur.

#![allow(deprecated)]

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use hf_hub::api::sync::ApiBuilder;
use hf_hub::api::Progress;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use super::nats_manager::default_nats_url;
use super::probe::repo_root_from_crate;
use crate::models::{INFRA_STATUS, KERNEL_AGENT};

/// Hugging Face model kimliği — ModernBERT-large + option-marker head.
pub const HF_REPO: &str = "convaiinnovations/laya";
pub const LAYA_ENGINE_EVENT: &str = "laya-engine";
pub const APP_SUPPORT_NAME: &str = "AgentLounge";
pub const MANIFEST_NAME: &str = "checksums.json";

pub const ARTIFACTS: &[&str] = &[
    "model.safetensors",
    "tokenizer/tokenizer.json",
    "encoder/config.json",
    "rl_agent_config.json",
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayaEnginePhase {
    Downloading,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LayaEngineStatus {
    pub phase: LayaEnginePhase,
    pub label: String,
    pub message: String,
    pub file: Option<String>,
    pub completed: u64,
    pub total: u64,
    pub path: String,
    pub error: Option<String>,
}

impl LayaEngineStatus {
    pub fn downloading(dir: &Path, file: Option<&str>, completed: u64, total: u64) -> Self {
        let file_name = file.unwrap_or("ağırlıklar");
        Self {
            phase: LayaEnginePhase::Downloading,
            label: "Laya Engine: Downloading".into(),
            message: format!("{file_name} indiriliyor"),
            file: file.map(str::to_string),
            completed,
            total,
            path: dir.display().to_string(),
            error: None,
        }
    }

    pub fn ready(dir: &Path) -> Self {
        Self {
            phase: LayaEnginePhase::Ready,
            label: "Laya Engine: Ready".into(),
            message: "Model dosyaları doğrulandı.".into(),
            file: None,
            completed: 1,
            total: 1,
            path: dir.display().to_string(),
            error: None,
        }
    }

    pub fn failed(dir: &Path, err: impl Into<String>) -> Self {
        let error = err.into();
        Self {
            phase: LayaEnginePhase::Failed,
            label: "Laya Engine: Failed".into(),
            message: "Model indirilemedi veya bütünlük doğrulaması başarısız.".into(),
            file: None,
            completed: 0,
            total: 0,
            path: dir.display().to_string(),
            error: Some(error),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.phase == LayaEnginePhase::Ready
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ChecksumManifest {
    repo: String,
    source: String,
    files: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct HubInfo {
    #[serde(default)]
    siblings: Vec<HubSibling>,
}

#[derive(Debug, Deserialize)]
struct HubSibling {
    rfilename: String,
    #[serde(default)]
    lfs: Option<HubLfs>,
}

#[derive(Debug, Deserialize)]
struct HubLfs {
    #[serde(default)]
    sha256: Option<String>,
}

/// OS / env girdileri — birim testlerde macOS/Linux/Windows yolları aynı hostta.
#[derive(Debug, Clone)]
pub struct PathEnv {
    pub os: String,
    pub home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub override_dir: Option<PathBuf>,
    pub repo_root: PathBuf,
}

impl PathEnv {
    pub fn from_process() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            home: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
            xdg_data_home: std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
            appdata: std::env::var_os("APPDATA").map(PathBuf::from),
            override_dir: env_nonempty("LOUNGE_LAYA_DIR").map(PathBuf::from),
            repo_root: repo_root_from_crate(),
        }
    }
}

/// Paylaşılan, kilitli indirme + durum.
#[derive(Clone)]
pub struct ModelManager {
    status: Arc<StdMutex<LayaEngineStatus>>,
    gate: Arc<StdMutex<()>>,
    nats_url: Arc<String>,
}

impl ModelManager {
    pub fn new() -> Self {
        Self::with_nats_url(default_nats_url())
    }

    pub fn with_nats_url(nats_url: impl Into<String>) -> Self {
        let dir = laya_dir();
        let initial = match verify_existing(&dir) {
            Ok(()) => LayaEngineStatus::ready(&dir),
            Err(_) => LayaEngineStatus::downloading(&dir, None, 0, 0),
        };
        Self {
            status: Arc::new(StdMutex::new(initial)),
            gate: Arc::new(StdMutex::new(())),
            nats_url: Arc::new(nats_url.into()),
        }
    }

    pub fn status(&self) -> LayaEngineStatus {
        self.status.lock().expect("laya engine status").clone()
    }

    fn set_status(&self, status: LayaEngineStatus) {
        *self.status.lock().expect("laya engine status") = status;
    }

    /// Dosyaları indirir / doğrular. RAM’e model yüklemez.
    pub fn ensure(&self, app: Option<&AppHandle>) -> LayaEngineStatus {
        let _busy = self.gate.lock().expect("laya engine gate");
        if self.status().is_ready() && verify_existing(&laya_dir()).is_ok() {
            let ready = LayaEngineStatus::ready(&laya_dir());
            self.set_status(ready.clone());
            emit_engine(app, Some(self.nats_url.as_str()), &ready);
            return ready;
        }
        let dir = laya_dir();
        let downloading = LayaEngineStatus::downloading(&dir, None, 0, 0);
        self.set_status(downloading.clone());
        emit_engine(app, Some(self.nats_url.as_str()), &downloading);
        let sink = EngineSink {
            app: app.cloned(),
            nats_url: self.nats_url.clone(),
            status: self.status.clone(),
        };
        match ensure_artifacts_with_sink(&dir, !skip_download(), &sink) {
            Ok(()) => {
                let ready = LayaEngineStatus::ready(&dir);
                self.set_status(ready.clone());
                emit_engine(app, Some(self.nats_url.as_str()), &ready);
                ready
            }
            Err(err) => {
                let failed = LayaEngineStatus::failed(&dir, err.to_string());
                self.set_status(failed.clone());
                emit_engine(app, Some(self.nats_url.as_str()), &failed);
                failed
            }
        }
    }
}

impl Default for ModelManager {
    fn default() -> Self {
        Self::new()
    }
}

struct EngineSink {
    app: Option<AppHandle>,
    nats_url: Arc<String>,
    status: Arc<StdMutex<LayaEngineStatus>>,
}

impl EngineSink {
    fn push(&self, next: LayaEngineStatus) {
        *self.status.lock().expect("laya engine status") = next.clone();
        emit_engine(self.app.as_ref(), Some(self.nats_url.as_str()), &next);
    }
}

struct HubProgress {
    sink: EngineSink,
    dir: PathBuf,
    file: String,
    completed: u64,
    total: u64,
    last_emit: Instant,
}

impl Progress for HubProgress {
    fn init(&mut self, size: usize, filename: &str) {
        self.file = filename.to_string();
        self.total = size as u64;
        self.completed = 0;
        self.last_emit = Instant::now();
        self.sink.push(LayaEngineStatus::downloading(
            &self.dir,
            Some(&self.file),
            0,
            self.total,
        ));
    }

    fn update(&mut self, size: usize) {
        self.completed = self.completed.saturating_add(size as u64);
        if self.last_emit.elapsed() < Duration::from_millis(250) && self.completed < self.total {
            return;
        }
        self.last_emit = Instant::now();
        self.sink.push(LayaEngineStatus::downloading(
            &self.dir,
            Some(&self.file),
            self.completed,
            self.total,
        ));
    }

    fn finish(&mut self) {
        self.completed = self.total;
        self.sink.push(LayaEngineStatus::downloading(
            &self.dir,
            Some(&self.file),
            self.completed,
            self.total,
        ));
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub fn skip_download() -> bool {
    skip_download_value(std::env::var("LOUNGE_LAYA_SKIP_DOWNLOAD").ok().as_deref())
}

pub fn skip_download_value(raw: Option<&str>) -> bool {
    matches!(raw, Some("1") | Some("true") | Some("TRUE"))
}

/// `AgentLounge/models` kökü (Laya alt dizini hariç).
pub fn models_root_for(env: &PathEnv) -> PathBuf {
    match env.os.as_str() {
        "windows" => env
            .appdata
            .clone()
            .or_else(|| env.home.clone())
            .unwrap_or_else(|| PathBuf::from(r"C:\Users\Default\AppData\Roaming"))
            .join(APP_SUPPORT_NAME)
            .join("models"),
        "linux" => {
            let base = env.xdg_data_home.clone().unwrap_or_else(|| {
                env.home
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("/home"))
                    .join(".local/share")
            });
            base.join(APP_SUPPORT_NAME).join("models")
        }
        _ => env
            .home
            .clone()
            .unwrap_or_else(|| PathBuf::from("/Users"))
            .join("Library/Application Support")
            .join(APP_SUPPORT_NAME)
            .join("models"),
    }
}

pub fn resolve_laya_dir(env: &PathEnv) -> PathBuf {
    if let Some(over) = &env.override_dir {
        return over.clone();
    }
    let prod = models_root_for(env).join("laya");
    if artifacts_present(&prod) {
        return prod;
    }
    let dev = env.repo_root.join("data/laya");
    if artifacts_present(&dev) {
        return dev;
    }
    prod
}

pub fn laya_dir() -> PathBuf {
    resolve_laya_dir(&PathEnv::from_process())
}

pub fn artifacts_present(dir: &Path) -> bool {
    ARTIFACTS.iter().all(|rel| dir.join(rel).is_file())
}

pub fn infra_status_envelope(status: &LayaEngineStatus) -> lounge_protocol::LoungeMessage {
    let payload = serde_json::to_value(status).unwrap_or(serde_json::json!({}));
    lounge_protocol::LoungeMessage::new(INFRA_STATUS, KERNEL_AGENT, payload)
}

fn publish_infra_status(nats_url: &str, status: &LayaEngineStatus) {
    if nats_url.trim().is_empty() {
        return;
    }
    let envelope = infra_status_envelope(status);
    let subject = envelope.subject.clone();
    let Ok(bytes) = serde_json::to_vec(&envelope) else {
        return;
    };
    let url = nats_url.to_string();
    let _ = std::thread::Builder::new()
        .name("lounge-infra-status".into())
        .spawn(move || match nats::connect(&url) {
            Ok(nc) => {
                if let Err(err) = nc.publish(&subject, bytes) {
                    log::debug!("{INFRA_STATUS} publish: {err}");
                }
            }
            Err(err) => log::debug!("{INFRA_STATUS} nats: {err}"),
        });
}

pub fn emit_engine(app: Option<&AppHandle>, nats_url: Option<&str>, status: &LayaEngineStatus) {
    if let Some(url) = nats_url {
        publish_infra_status(url, status);
    }
    let Some(app) = app else {
        return;
    };
    if let Some(window) = app.get_webview_window("main") {
        if let Err(err) = window.emit(LAYA_ENGINE_EVENT, status) {
            log::debug!("{LAYA_ENGINE_EVENT} window emit: {err}");
        }
        return;
    }
    if let Err(err) = app.emit(LAYA_ENGINE_EVENT, status) {
        log::debug!("{LAYA_ENGINE_EVENT} app emit: {err}");
    }
}

pub fn ensure_artifacts(dir: &Path, download: bool) -> Result<()> {
    ensure_artifacts_with_sink(
        dir,
        download,
        &EngineSink {
            app: None,
            nats_url: Arc::new(String::new()),
            status: Arc::new(StdMutex::new(LayaEngineStatus::downloading(
                dir, None, 0, 0,
            ))),
        },
    )
}

fn ensure_artifacts_with_sink(dir: &Path, download: bool, sink: &EngineSink) -> Result<()> {
    if artifacts_present(dir) {
        match verify_dir(dir, None, true) {
            Ok(()) => return Ok(()),
            Err(err) => {
                log::warn!("Laya checksum: {err}");
                if !download || skip_download() {
                    return Err(err);
                }
            }
        }
    }
    if !download || skip_download() {
        anyhow::bail!("Laya ağırlıkları yok: {}", dir.display());
    }
    std::fs::create_dir_all(dir.join("tokenizer")).context("Laya tokenizer dizini")?;
    std::fs::create_dir_all(dir.join("encoder")).context("Laya encoder dizini")?;
    let hub_hashes = fetch_hub_hashes(dir).unwrap_or_default();
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..2 {
        match download_artifacts(dir, sink, &hub_hashes) {
            Ok(()) => {
                verify_dir(dir, Some(&hub_hashes), true)?;
                return Ok(());
            }
            Err(err) => {
                last_err = Some(err);
                log::warn!(
                    "Laya indirme denemesi {}: {}",
                    attempt + 1,
                    last_err.as_ref().unwrap()
                );
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Laya indirme başarısız")))
}

fn verify_existing(dir: &Path) -> Result<()> {
    if !artifacts_present(dir) {
        anyhow::bail!("Laya ağırlıkları yok: {}", dir.display());
    }
    verify_dir(dir, None, true)
}

pub fn verify_dir(
    dir: &Path,
    hub_hashes: Option<&HashMap<String, String>>,
    persist: bool,
) -> Result<()> {
    let computed = hash_artifacts(dir)?;
    let manifest_path = dir.join(MANIFEST_NAME);
    if manifest_path.is_file() {
        let expected = load_manifest(&manifest_path)?;
        compare_hashes(dir, &expected.files, &computed)?;
        return Ok(());
    }
    if let Some(hub) = hub_hashes.filter(|map| !map.is_empty()) {
        let usable: HashMap<String, String> = ARTIFACTS
            .iter()
            .filter_map(|rel| {
                hub.get(*rel)
                    .cloned()
                    .map(|hash| ((*rel).to_string(), hash))
            })
            .collect();
        if !usable.is_empty() {
            compare_hashes(dir, &usable, &computed)?;
            if persist {
                write_manifest(dir, "hub", &merge_expected(&usable, &computed))?;
            }
            return Ok(());
        }
    }
    if persist {
        write_manifest(dir, "computed", &computed)?;
    }
    Ok(())
}

fn merge_expected(
    hub: &HashMap<String, String>,
    computed: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut out = computed.clone();
    for (key, value) in hub {
        out.insert(key.clone(), value.clone());
    }
    out
}

fn compare_hashes(
    dir: &Path,
    expected: &HashMap<String, String>,
    computed: &HashMap<String, String>,
) -> Result<()> {
    let mut mismatched = Vec::new();
    for rel in ARTIFACTS {
        let Some(want) = expected.get(*rel) else {
            continue;
        };
        let got = computed
            .get(*rel)
            .ok_or_else(|| anyhow::anyhow!("Laya checksum eksik: {rel}"))?;
        if !want.eq_ignore_ascii_case(got) {
            mismatched.push(*rel);
            let path = dir.join(rel);
            if path.is_file() {
                std::fs::remove_file(&path).with_context(|| {
                    format!("bozuk Laya dosyası silinemedi: {}", path.display())
                })?;
            }
        }
    }
    if !mismatched.is_empty() {
        anyhow::bail!(
            "Laya checksum uyuşmazlığı (sha256): {}",
            mismatched.join(", ")
        );
    }
    Ok(())
}

fn hash_artifacts(dir: &Path) -> Result<HashMap<String, String>> {
    let mut files = HashMap::new();
    for rel in ARTIFACTS {
        let path = dir.join(rel);
        if !path.is_file() {
            anyhow::bail!("Laya ağırlıkları yok: {}", path.display());
        }
        files.insert((*rel).to_string(), sha256_file(&path)?);
    }
    Ok(files)
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("oku {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn load_manifest(path: &Path) -> Result<ChecksumManifest> {
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("manifest {}", path.display()))?;
    serde_json::from_str(&raw).context("Laya checksum manifest")
}

fn write_manifest(dir: &Path, source: &str, files: &HashMap<String, String>) -> Result<()> {
    let manifest = ChecksumManifest {
        repo: HF_REPO.into(),
        source: source.into(),
        files: files.clone(),
    };
    let path = dir.join(MANIFEST_NAME);
    let json = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(&path, json).with_context(|| format!("yaz {}", path.display()))?;
    Ok(())
}

fn fetch_hub_hashes(dir: &Path) -> Result<HashMap<String, String>> {
    let api = hub_api(dir)?;
    let repo = api.model(HF_REPO.into());
    let response = repo
        .info_request()
        .query("blobs", "true")
        .call()
        .map_err(|err| anyhow::anyhow!("HF info: {err}"))?;
    let info: HubInfo = response
        .into_json()
        .map_err(|err| anyhow::anyhow!("HF info json: {err}"))?;
    let mut hashes = HashMap::new();
    for sibling in info.siblings {
        if !ARTIFACTS.contains(&sibling.rfilename.as_str()) {
            continue;
        }
        if let Some(sha) = sibling.lfs.and_then(|lfs| lfs.sha256) {
            if !sha.is_empty() {
                hashes.insert(sibling.rfilename, sha);
            }
        }
    }
    for rel in ARTIFACTS {
        if hashes.contains_key(*rel) {
            continue;
        }
        if let Some(hash) = fetch_sidecar_hash(&repo, rel) {
            hashes.insert((*rel).to_string(), hash);
        }
    }
    Ok(hashes)
}

fn fetch_sidecar_hash(repo: &hf_hub::api::sync::ApiRepo, rel: &str) -> Option<String> {
    let name = format!("{rel}.sha256");
    let path = repo.get(&name).ok()?;
    let raw = std::fs::read_to_string(path).ok()?;
    parse_sha256_line(&raw)
}

fn parse_sha256_line(raw: &str) -> Option<String> {
    let token = raw
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"');
    if token.len() == 64 && token.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(token.to_ascii_lowercase())
    } else {
        None
    }
}

fn hub_api(dir: &Path) -> Result<hf_hub::api::sync::Api> {
    ApiBuilder::new()
        .with_progress(false)
        .with_cache_dir(dir.join(".hf"))
        .build()
        .context("Hugging Face API")
}

fn download_artifacts(
    dir: &Path,
    sink: &EngineSink,
    hub_hashes: &HashMap<String, String>,
) -> Result<()> {
    let api = hub_api(dir)?;
    let repo = api.model(HF_REPO.into());
    for rel in ARTIFACTS {
        let dest = dir.join(rel);
        if dest.is_file() {
            if let Ok(hash) = sha256_file(&dest) {
                let hash_ok = match hub_hashes.get(*rel) {
                    None => true,
                    Some(want) => want.eq_ignore_ascii_case(&hash),
                };
                if hash_ok {
                    continue;
                }
                std::fs::remove_file(&dest).ok();
            }
        }
        sink.push(LayaEngineStatus::downloading(dir, Some(rel), 0, 0));
        let progress = HubProgress {
            sink: EngineSink {
                app: sink.app.clone(),
                nats_url: sink.nats_url.clone(),
                status: sink.status.clone(),
            },
            dir: dir.to_path_buf(),
            file: (*rel).to_string(),
            completed: 0,
            total: 0,
            last_emit: Instant::now(),
        };
        let src = repo
            .download_with_progress(rel, progress)
            .with_context(|| format!("HF get {rel}"))?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&src, &dest)
            .with_context(|| format!("kopyala {} → {}", src.display(), dest.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_laya() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laya-mm-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("tokenizer")).unwrap();
        std::fs::create_dir_all(dir.join("encoder")).unwrap();
        dir
    }

    fn write_artifacts(dir: &Path, payload: &[u8]) {
        for rel in ARTIFACTS {
            let path = dir.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, payload).unwrap();
        }
    }

    #[test]
    fn resolves_macos_linux_windows_app_support() {
        let macos = PathEnv {
            os: "macos".into(),
            home: Some(PathBuf::from("/Users/demo")),
            xdg_data_home: None,
            appdata: None,
            override_dir: None,
            repo_root: PathBuf::from("/repo"),
        };
        assert_eq!(
            resolve_laya_dir(&macos),
            PathBuf::from("/Users/demo/Library/Application Support/AgentLounge/models/laya")
        );

        let linux_xdg = PathEnv {
            os: "linux".into(),
            home: Some(PathBuf::from("/home/demo")),
            xdg_data_home: Some(PathBuf::from("/custom/share")),
            appdata: None,
            override_dir: None,
            repo_root: PathBuf::from("/repo"),
        };
        assert_eq!(
            resolve_laya_dir(&linux_xdg),
            PathBuf::from("/custom/share/AgentLounge/models/laya")
        );

        let linux_home = PathEnv {
            os: "linux".into(),
            home: Some(PathBuf::from("/home/demo")),
            xdg_data_home: None,
            appdata: None,
            override_dir: None,
            repo_root: PathBuf::from("/repo"),
        };
        assert_eq!(
            resolve_laya_dir(&linux_home),
            PathBuf::from("/home/demo/.local/share/AgentLounge/models/laya")
        );

        let windows = PathEnv {
            os: "windows".into(),
            home: Some(PathBuf::from(r"C:\Users\demo")),
            xdg_data_home: None,
            appdata: Some(PathBuf::from(r"C:\Users\demo\AppData\Roaming")),
            override_dir: None,
            repo_root: PathBuf::from(r"C:\repo"),
        };
        assert_eq!(
            resolve_laya_dir(&windows),
            PathBuf::from(r"C:\Users\demo\AppData\Roaming")
                .join("AgentLounge")
                .join("models")
                .join("laya")
        );
    }

    #[test]
    fn env_override_wins_over_app_support() {
        let env = PathEnv {
            os: "macos".into(),
            home: Some(PathBuf::from("/Users/demo")),
            xdg_data_home: None,
            appdata: None,
            override_dir: Some(PathBuf::from("/tmp/custom-laya")),
            repo_root: PathBuf::from("/repo"),
        };
        assert_eq!(resolve_laya_dir(&env), PathBuf::from("/tmp/custom-laya"));
    }

    #[test]
    fn checksum_mismatch_deletes_and_fails() {
        let dir = temp_laya();
        write_artifacts(&dir, b"good-weights");
        verify_dir(&dir, None, true).unwrap();
        std::fs::write(dir.join("model.safetensors"), b"tampered").unwrap();
        let err = verify_dir(&dir, None, true).unwrap_err().to_string();
        assert!(err.to_lowercase().contains("checksum"));
        assert!(!dir.join("model.safetensors").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skip_download_does_not_fetch_missing_files() {
        let dir = temp_laya();
        let err = ensure_artifacts(&dir, false).unwrap_err().to_string();
        assert!(err.contains("ağırlıkları yok"));
        assert!(!dir.join("model.safetensors").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn skip_download_flag_reads_env() {
        assert!(skip_download_value(Some("1")));
        assert!(skip_download_value(Some("true")));
        assert!(skip_download_value(Some("TRUE")));
        assert!(!skip_download_value(Some("0")));
        assert!(!skip_download_value(None));
    }

    #[test]
    fn first_verify_persists_manifest() {
        let dir = temp_laya();
        write_artifacts(&dir, b"seed");
        verify_dir(&dir, None, true).unwrap();
        assert!(dir.join(MANIFEST_NAME).is_file());
        verify_dir(&dir, None, true).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_sidecar_sha256() {
        assert_eq!(
            parse_sha256_line(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  model.safetensors\n"
            )
            .unwrap()
            .len(),
            64
        );
        assert!(parse_sha256_line("not-a-hash").is_none());
    }

    #[test]
    fn engine_labels_match_fleet_copy() {
        let dir = Path::new("/tmp/laya");
        assert_eq!(
            LayaEngineStatus::downloading(dir, Some("model.safetensors"), 1, 2).label,
            "Laya Engine: Downloading"
        );
        assert_eq!(LayaEngineStatus::ready(dir).label, "Laya Engine: Ready");
        assert_eq!(
            LayaEngineStatus::failed(dir, "x").label,
            "Laya Engine: Failed"
        );
    }

    #[test]
    fn infra_envelope_ready_label_without_nats() {
        let status = LayaEngineStatus::ready(Path::new("/tmp/laya"));
        assert_eq!(status.label, "Laya Engine: Ready");
        let envelope = infra_status_envelope(&status);
        assert_eq!(envelope.subject, INFRA_STATUS);
        assert_eq!(envelope.source_agent, KERNEL_AGENT);
        assert_eq!(envelope.payload["label"], "Laya Engine: Ready");
        assert_eq!(envelope.payload["phase"], "ready");
        emit_engine(None, None, &status);
        emit_engine(None, Some(""), &status);
    }

    #[test]
    fn uses_dev_fallback_when_repo_copy_exists() {
        let repo = temp_laya();
        let data = repo.join("data/laya");
        write_artifacts(&data, b"dev");
        let env = PathEnv {
            os: "macos".into(),
            home: Some(PathBuf::from("/Users/nobody-laya-test")),
            xdg_data_home: None,
            appdata: None,
            override_dir: None,
            repo_root: repo.clone(),
        };
        assert_eq!(resolve_laya_dir(&env), data);
        let _ = std::fs::remove_dir_all(&repo);
    }
}
