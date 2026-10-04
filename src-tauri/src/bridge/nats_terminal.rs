//! Paylaşılan NATS terminal abonesi — çağrı başına connect/thread yok.
//!
//! `lounge.task.completed` / `lounge.task.failed` → `broadcast` task_id.
//! Abone kapanırsa alıcılar `Closed` görür; select! kolu devre dışı kalır.
//! İlk connect başarısız olursa aynı görev geri çekilmeli yeniden dener.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;

use crate::models::{TASK_COMPLETED, TASK_FAILED};

const BROADCAST_CAP: usize = 512;

#[derive(Debug)]
pub struct NatsTerminalHub {
    nats_url: String,
    tx: broadcast::Sender<String>,
    started: AtomicBool,
}

impl NatsTerminalHub {
    pub fn new(nats_url: impl Into<String>) -> Arc<Self> {
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        Arc::new(Self {
            nats_url: nats_url.into(),
            tx,
            started: AtomicBool::new(false),
        })
    }

    /// Test / skip_nats — hiç bağlanma.
    pub fn inert() -> Arc<Self> {
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        Arc::new(Self {
            nats_url: String::new(),
            tx,
            started: AtomicBool::new(true), // ensure_started no-op
        })
    }

    pub fn subscribe(self: &Arc<Self>) -> broadcast::Receiver<String> {
        self.ensure_started();
        self.tx.subscribe()
    }

    fn ensure_started(self: &Arc<Self>) {
        if self.nats_url.is_empty() {
            return;
        }
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let url = self.nats_url.clone();
        let tx = self.tx.clone();
        let started = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let mut backoff = Duration::from_secs(1);
            let nc = loop {
                #[allow(deprecated)]
                match nats::connect(&url) {
                    Ok(nc) => break nc,
                    Err(err) => {
                        log::warn!(
                            "NatsTerminalHub connect failed ({err}); retry in {:?}",
                            backoff
                        );
                        // Allow a later subscribe to re-spawn if this task exits unexpectedly.
                        started.started.store(false, Ordering::SeqCst);
                        std::thread::sleep(backoff);
                        if started.started.swap(true, Ordering::SeqCst) {
                            // Another ensure_started won — exit this worker.
                            return;
                        }
                        backoff = (backoff * 2).min(Duration::from_secs(60));
                    }
                }
            };
            for subject in [TASK_COMPLETED, TASK_FAILED] {
                let sub_nc = nc.clone();
                let tx = tx.clone();
                let subject = subject.to_string();
                std::thread::Builder::new()
                    .name(format!("nats-term-{subject}"))
                    .spawn(move || {
                        let Ok(sub) = sub_nc.subscribe(&subject) else {
                            return;
                        };
                        for msg in sub.messages() {
                            if let Some(id) = extract_task_id(&msg.data) {
                                let _ = tx.send(id);
                            }
                        }
                    })
                    .ok();
            }
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        });
    }
}

fn extract_task_id(data: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(data).ok()?;
    value
        .get("id")
        .or_else(|| value.get("task_id"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_id_fields() {
        assert_eq!(extract_task_id(br#"{"id":"a1"}"#).as_deref(), Some("a1"));
        assert_eq!(
            extract_task_id(br#"{"task_id":"b2"}"#).as_deref(),
            Some("b2")
        );
        assert_eq!(extract_task_id(br#"{}"#), None);
    }
}
