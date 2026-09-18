#![allow(deprecated)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use lounge_protocol::{LoungeMessage, BUS_CONNECTED, BUS_HEARTBEAT, BUS_PROBE, UI_EVENT, WILDCARD};
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;

const RECONNECT: Duration = Duration::from_secs(2);
const HEARTBEAT: Duration = Duration::from_secs(10);

/// NATS `lounge.>` sinir sistemi. Gelen zarfları Tauri event olarak UI'a fırlatır.
#[derive(Clone)]
pub struct BusManager {
    nats_url: String,
    app: AppHandle,
    ticks: std::sync::Arc<AtomicU64>,
}

impl BusManager {
    pub fn new(nats_url: impl Into<String>, app: AppHandle) -> Self {
        Self {
            nats_url: nats_url.into(),
            app,
            ticks: std::sync::Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn nats_url(&self) -> &str {
        &self.nats_url
    }

    pub async fn run(&self) {
        loop {
            if let Err(err) = self.serve().await {
                log::warn!("NATS bus: {err}");
            } else {
                log::warn!("NATS bus bağlantısı kapandı, yeniden bağlanılıyor");
            }
            tokio::time::sleep(RECONNECT).await;
        }
    }

    pub async fn probe(&self) -> Result<LoungeMessage> {
        let tick = self.ticks.fetch_add(1, Ordering::Relaxed) + 1;
        let msg = LoungeMessage::new(
            BUS_PROBE,
            "lounge-bus",
            serde_json::json!({ "tick": tick, "kind": "probe" }),
        );
        self.publish(&msg).await?;
        Ok(msg)
    }

    pub async fn publish(&self, msg: &LoungeMessage) -> Result<()> {
        let url = self.nats_url.clone();
        let subject = msg.subject.clone();
        let bytes = serde_json::to_vec(msg).context("LoungeMessage serialize")?;
        tokio::task::spawn_blocking(move || {
            let nc = nats::connect(&url)?;
            nc.publish(&subject, bytes)
        })
        .await
        .context("bus publish join")?
        .context("bus publish")?;
        Ok(())
    }

    async fn serve(&self) -> Result<()> {
        let url = self.nats_url.clone();
        let nc = tokio::task::spawn_blocking(move || nats::connect(&url))
            .await
            .context("NATS bus connect join")?
            .context("NATS bus bağlanamadı")?;

        let (tx, mut rx) = mpsc::channel::<nats::Message>(256);
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
        let listener = nc.clone();
        tokio::task::spawn_blocking(move || match listener.subscribe(WILDCARD) {
            Ok(sub) => {
                let _ = ready_tx.send(Ok(()));
                for msg in sub.messages() {
                    if tx.blocking_send(msg).is_err() {
                        break;
                    }
                }
                Ok(())
            }
            Err(err) => {
                let _ = ready_tx.send(Err(err.to_string()));
                Err(err)
            }
        });
        ready_rx
            .await
            .context("bus subscribe ready")?
            .map_err(|err| anyhow::anyhow!(err))?;

        log::info!("NATS bus dinliyor: {} ({WILDCARD})", self.nats_url);

        let connected = LoungeMessage::new(
            BUS_CONNECTED,
            "lounge-bus",
            serde_json::json!({ "url": self.nats_url, "wildcard": WILDCARD }),
        );
        publish_on(&nc, &connected).await?;

        let heartbeat_nc = nc.clone();
        let ticks = self.ticks.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(HEARTBEAT).await;
                let tick = ticks.fetch_add(1, Ordering::Relaxed) + 1;
                let msg = LoungeMessage::new(
                    BUS_HEARTBEAT,
                    "lounge-bus",
                    serde_json::json!({ "tick": tick }),
                );
                if publish_on(&heartbeat_nc, &msg).await.is_err() {
                    break;
                }
            }
        });

        while let Some(raw) = rx.recv().await {
            let envelope = LoungeMessage::from_nats(&raw.subject, &raw.data);
            if let Err(err) = self.app.emit(UI_EVENT, &envelope) {
                log::debug!("tauri emit {UI_EVENT}: {err}");
            }
        }
        Ok(())
    }
}

async fn publish_on(nc: &nats::Connection, msg: &LoungeMessage) -> Result<()> {
    let nc = nc.clone();
    let subject = msg.subject.clone();
    let bytes = serde_json::to_vec(msg)?;
    tokio::task::spawn_blocking(move || nc.publish(&subject, bytes))
        .await
        .context("bus nc publish join")?
        .context("bus nc publish")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use lounge_protocol::LoungeMessage;

    #[test]
    fn wraps_dispatcher_payloads_for_the_stream() {
        let raw = serde_json::json!({
            "id": "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
            "type": "task",
            "source_agent": "cursor",
            "summary": "probe",
            "project_id": "agent-lounge-os",
            "created_at": "2026-09-18T13:00:00.000Z"
        });
        let msg =
            LoungeMessage::from_nats("lounge.task.requested", &serde_json::to_vec(&raw).unwrap());
        assert_eq!(msg.source_agent, "cursor");
        assert_eq!(msg.subject, "lounge.task.requested");
        assert_eq!(msg.state(), "queued");
    }
}
