//! Dış NATS worker kayıt defteri — register / heartbeat / unregister.

#![allow(deprecated)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use lounge_protocol::{validate_schema, SchemaKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::db::ExperienceStore;
use crate::models::{
    now_rfc3339, worker_tasks_subject, DiscoveredTool, WORKERS_HEARTBEAT, WORKERS_REGISTER,
    WORKERS_UNREGISTER,
};

/// Heartbeat gelmezse worker çevrimdışı sayılır.
pub const HEARTBEAT_TTL: Duration = Duration::from_secs(45);
const SWEEP_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerRegistration {
    pub bot_id: String,
    pub name: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub version: String,
    pub pid: u64,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegisteredWorker {
    pub bot_id: String,
    pub name: String,
    pub capabilities: Vec<String>,
    pub version: String,
    pub pid: u64,
    pub online: bool,
    pub last_heartbeat: String,
    pub tasks_subject: String,
}

#[derive(Debug, Clone)]
struct LiveWorker {
    name: String,
    capabilities: Vec<String>,
    version: String,
    pid: u64,
    last_seen: Instant,
    last_heartbeat: String,
    online: bool,
}

#[derive(Clone, Default)]
pub struct WorkerRegistry {
    inner: Arc<StdMutex<HashMap<String, LiveWorker>>>,
    store: Option<ExperienceStore>,
}

impl WorkerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_store(store: ExperienceStore) -> Self {
        Self {
            inner: Arc::new(StdMutex::new(HashMap::new())),
            store: Some(store),
        }
    }

    pub fn apply_registration(&self, reg: WorkerRegistration) -> Result<RegisteredWorker> {
        validate_schema(
            SchemaKind::WorkerRegistration,
            &serde_json::to_value(&reg).context("worker registration json")?,
        )
        .map_err(|err| anyhow::anyhow!(err))?;

        let bot_id = reg.bot_id.trim().to_ascii_lowercase();
        let tasks_subject = worker_tasks_subject(&bot_id)
            .ok_or_else(|| anyhow::anyhow!("geçersiz bot_id: {}", reg.bot_id))?;
        let now = now_rfc3339();
        let action = reg.action.trim().to_ascii_lowercase();

        let snapshot = {
            let mut guard = self.inner.lock().expect("worker registry lock");
            match action.as_str() {
                "unregister" => {
                    if let Some(existing) = guard.get_mut(&bot_id) {
                        existing.online = false;
                        existing.last_heartbeat = now.clone();
                        RegisteredWorker {
                            bot_id: bot_id.clone(),
                            name: existing.name.clone(),
                            capabilities: existing.capabilities.clone(),
                            version: existing.version.clone(),
                            pid: existing.pid,
                            online: false,
                            last_heartbeat: now.clone(),
                            tasks_subject: tasks_subject.clone(),
                        }
                    } else {
                        RegisteredWorker {
                            bot_id: bot_id.clone(),
                            name: reg.name.clone(),
                            capabilities: reg.capabilities.clone(),
                            version: reg.version.clone(),
                            pid: reg.pid,
                            online: false,
                            last_heartbeat: now.clone(),
                            tasks_subject: tasks_subject.clone(),
                        }
                    }
                }
                "register" | "heartbeat" => {
                    let entry = guard.entry(bot_id.clone()).or_insert_with(|| LiveWorker {
                        name: reg.name.clone(),
                        capabilities: reg.capabilities.clone(),
                        version: reg.version.clone(),
                        pid: reg.pid,
                        last_seen: Instant::now(),
                        last_heartbeat: now.clone(),
                        online: true,
                    });
                    entry.name = reg.name.clone();
                    entry.capabilities = reg.capabilities.clone();
                    entry.version = reg.version.clone();
                    entry.pid = reg.pid;
                    entry.last_seen = Instant::now();
                    entry.last_heartbeat = now.clone();
                    entry.online = true;
                    RegisteredWorker {
                        bot_id: bot_id.clone(),
                        name: entry.name.clone(),
                        capabilities: entry.capabilities.clone(),
                        version: entry.version.clone(),
                        pid: entry.pid,
                        online: true,
                        last_heartbeat: entry.last_heartbeat.clone(),
                        tasks_subject: tasks_subject.clone(),
                    }
                }
                other => anyhow::bail!("bilinmeyen worker action: {other}"),
            }
        };

        self.persist(&snapshot, action == "unregister")?;
        Ok(snapshot)
    }

    pub fn apply_json(&self, data: &[u8]) -> Result<RegisteredWorker> {
        let value: Value = serde_json::from_slice(data).context("worker payload json")?;
        // LoungeMessage zarfı geldiyse iç payload'ı kullan.
        let body = if value.get("bot_id").is_some() {
            value
        } else if let Some(payload) = value.get("payload") {
            payload.clone()
        } else {
            value
        };
        let reg: WorkerRegistration =
            serde_json::from_value(body).context("WorkerRegistration şeması")?;
        self.apply_registration(reg)
    }

    pub fn is_online(&self, bot_id: &str) -> bool {
        let id = bot_id.trim().to_ascii_lowercase();
        let mut guard = self.inner.lock().expect("worker registry lock");
        if let Some(worker) = guard.get_mut(&id) {
            if worker.online && worker.last_seen.elapsed() > HEARTBEAT_TTL {
                worker.online = false;
            }
            return worker.online;
        }
        false
    }

    pub fn has_worker(&self, bot_id: &str) -> bool {
        let id = bot_id.trim().to_ascii_lowercase();
        self.inner
            .lock()
            .expect("worker registry lock")
            .contains_key(&id)
    }

    pub fn list(&self) -> Vec<RegisteredWorker> {
        self.sweep_locked();
        let guard = self.inner.lock().expect("worker registry lock");
        let mut rows: Vec<_> = guard
            .iter()
            .filter_map(|(bot_id, worker)| {
                let tasks_subject = worker_tasks_subject(bot_id)?;
                Some(RegisteredWorker {
                    bot_id: bot_id.clone(),
                    name: worker.name.clone(),
                    capabilities: worker.capabilities.clone(),
                    version: worker.version.clone(),
                    pid: worker.pid,
                    online: worker.online,
                    last_heartbeat: worker.last_heartbeat.clone(),
                    tasks_subject,
                })
            })
            .collect();
        rows.sort_by(|a, b| a.bot_id.cmp(&b.bot_id));
        rows
    }

    pub fn list_online(&self) -> Vec<RegisteredWorker> {
        self.list().into_iter().filter(|w| w.online).collect()
    }

    fn sweep_locked(&self) {
        let mut guard = self.inner.lock().expect("worker registry lock");
        let mut went_offline = Vec::new();
        for (bot_id, worker) in guard.iter_mut() {
            if worker.online && worker.last_seen.elapsed() > HEARTBEAT_TTL {
                worker.online = false;
                worker.last_heartbeat = now_rfc3339();
                went_offline.push(RegisteredWorker {
                    bot_id: bot_id.clone(),
                    name: worker.name.clone(),
                    capabilities: worker.capabilities.clone(),
                    version: worker.version.clone(),
                    pid: worker.pid,
                    online: false,
                    last_heartbeat: worker.last_heartbeat.clone(),
                    tasks_subject: worker_tasks_subject(bot_id).unwrap_or_default(),
                });
            }
        }
        drop(guard);
        for snap in went_offline {
            let _ = self.persist(&snap, true);
        }
    }

    fn persist(&self, worker: &RegisteredWorker, offline: bool) -> Result<()> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let tool_id = format!("worker:{}", worker.bot_id);
        let mut tool = DiscoveredTool::new("nats_worker", &worker.bot_id, "worker");
        tool.id = tool_id.clone();
        tool.name = worker.name.clone();
        tool.endpoint = Some(worker.tasks_subject.clone());
        let online = worker.online && !offline;
        tool.detail = Some(format!(
            "NATS worker · v{} · caps={} · {}",
            worker.version,
            worker.capabilities.join(","),
            if online { "online" } else { "offline" }
        ));
        tool.available = online;
        tool.host_id = Some(worker.bot_id.clone());
        tool.host_ids = vec![worker.bot_id.clone()];
        // capabilities detail zaten detail alanında; host_id payload'a from_discovered ile gider.
        let store = store.clone();
        tauri::async_runtime::spawn(async move {
            if online {
                if let Err(err) = store.upsert_connected_tool(tool).await {
                    log::warn!("worker connected_tools yazılamadı: {err}");
                }
            } else {
                // Önce son durumu yaz, sonra pasifleştir.
                if let Err(err) = store.upsert_connected_tool(tool).await {
                    log::warn!("worker connected_tools yazılamadı: {err}");
                }
                if let Err(err) = store.deactivate_connected_tool(tool_id).await {
                    log::warn!("worker deactivate: {err}");
                }
            }
        });
        Ok(())
    }

    pub async fn listen(&self, nats_url: impl Into<String>) -> Result<()> {
        let url = nats_url.into();
        loop {
            match self.listen_once(&url).await {
                Ok(()) => log::warn!("worker registry NATS kapandı, yeniden bağlanılıyor"),
                Err(err) => log::error!("worker registry NATS: {err}"),
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn listen_once(&self, url: &str) -> Result<()> {
        let url_owned = url.to_string();
        let nc = tokio::task::spawn_blocking(move || nats::connect(&url_owned))
            .await
            .context("NATS connect join")?
            .context("NATS bağlantısı kurulamadı")?;

        let (tx, mut rx) = mpsc::channel::<(String, Vec<u8>)>(64);

        for subject in [WORKERS_REGISTER, WORKERS_HEARTBEAT, WORKERS_UNREGISTER] {
            let sub_nc = nc.clone();
            let tx = tx.clone();
            let subject = subject.to_string();
            tokio::task::spawn_blocking(move || {
                let sub = sub_nc.subscribe(&subject)?;
                for msg in sub.messages() {
                    if tx
                        .blocking_send((msg.subject.clone(), msg.data.to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
                Ok::<_, std::io::Error>(())
            });
        }
        drop(tx);

        log::info!(
            "worker registry dinliyor: {url} ({WORKERS_REGISTER}|{WORKERS_HEARTBEAT}|{WORKERS_UNREGISTER})"
        );

        let mut sweep = tokio::time::interval(SWEEP_INTERVAL);
        loop {
            tokio::select! {
                maybe = rx.recv() => {
                    match maybe {
                        Some((_subject, data)) => {
                            if let Err(err) = self.apply_json(&data) {
                                log::warn!("worker kayıt mesajı reddedildi: {err}");
                            }
                        }
                        None => break,
                    }
                }
                _ = sweep.tick() => {
                    self.sweep_locked();
                }
            }
        }
        Ok(())
    }
}

/// Yönlendirme yardımcısı — kayıtlı çevrimiçi worker için inbox konusu.
pub fn route_subject_for(registry: &WorkerRegistry, bot_id: &str) -> Option<String> {
    if registry.is_online(bot_id) {
        worker_tasks_subject(bot_id)
    } else {
        None
    }
}

/// Status snapshot — MCP / UI.
pub fn workers_status_json(registry: &WorkerRegistry) -> Value {
    let workers = registry.list();
    serde_json::json!(workers
        .into_iter()
        .map(|w| serde_json::json!({
            "bot_id": w.bot_id,
            "name": w.name,
            "capabilities": w.capabilities,
            "version": w.version,
            "pid": w.pid,
            "online": w.online,
            "last_heartbeat": w.last_heartbeat,
            "tasks_subject": w.tasks_subject,
        }))
        .collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(action: &str) -> WorkerRegistration {
        WorkerRegistration {
            bot_id: "grok-tester".into(),
            name: "Grok-Tester".into(),
            capabilities: vec!["echo".into(), "ping".into()],
            version: "0.1.0".into(),
            pid: 99,
            action: action.into(),
            created_at: Some("2026-09-18T12:00:00.000Z".into()),
        }
    }

    #[test]
    fn register_heartbeat_and_offline() {
        let reg = WorkerRegistry::new();
        let online = reg.apply_registration(sample("register")).unwrap();
        assert!(online.online);
        assert_eq!(online.tasks_subject, "lounge.tasks.grok-tester");
        assert!(reg.is_online("grok-tester"));
        assert!(reg.has_worker("grok-tester"));

        reg.apply_registration(sample("heartbeat")).unwrap();
        assert!(reg.is_online("Grok-Tester"));

        let off = reg.apply_registration(sample("unregister")).unwrap();
        assert!(!off.online);
        assert!(!reg.is_online("grok-tester"));
    }

    #[test]
    fn rejects_schema_violation() {
        let reg = WorkerRegistry::new();
        let bad = WorkerRegistration {
            bot_id: "Bad.Id".into(),
            name: "x".into(),
            capabilities: vec![],
            version: "1".into(),
            pid: 1,
            action: "register".into(),
            created_at: None,
        };
        assert!(reg.apply_registration(bad).is_err());
    }

    #[test]
    fn route_subject_only_when_online() {
        let reg = WorkerRegistry::new();
        assert!(route_subject_for(&reg, "grok-tester").is_none());
        reg.apply_registration(sample("register")).unwrap();
        assert_eq!(
            route_subject_for(&reg, "grok-tester").as_deref(),
            Some("lounge.tasks.grok-tester")
        );
        reg.apply_registration(sample("unregister")).unwrap();
        assert!(route_subject_for(&reg, "grok-tester").is_none());
    }

    #[test]
    fn status_json_lists_workers() {
        let reg = WorkerRegistry::new();
        reg.apply_registration(sample("register")).unwrap();
        let json = workers_status_json(&reg);
        assert_eq!(json[0]["bot_id"], "grok-tester");
        assert_eq!(json[0]["online"], true);
    }
}
