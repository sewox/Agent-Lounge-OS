#![allow(deprecated)]

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::models::{
    NatsUiEvent, EXPERIENCE_REPORTED, TASK_ASSIGNED, TASK_COMPLETED, TASK_FAILED, TASK_REQUESTED,
};

const WILDCARD: &str = "lounge.>";

pub async fn listen_lounge_wildcard(app: AppHandle, nats_url: String) {
    loop {
        if let Err(err) = listen_once(&app, &nats_url).await {
            log::warn!("NATS UI hub: {err}");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn listen_once(app: &AppHandle, nats_url: &str) -> Result<()> {
    let url = nats_url.to_string();
    let nc = tokio::task::spawn_blocking(move || nats::connect(&url))
        .await
        .context("NATS hub connect join")?
        .context("NATS hub bağlanamadı")?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<nats::Message>(256);
    let listener = nc.clone();
    tokio::task::spawn_blocking(move || {
        let sub = listener.subscribe(WILDCARD)?;
        for msg in sub.messages() {
            if tx.blocking_send(msg).is_err() {
                break;
            }
        }
        Ok::<_, std::io::Error>(())
    });

    log::info!("NATS UI hub dinliyor: {nats_url} ({WILDCARD})");

    while let Some(msg) = rx.recv().await {
        let event = to_ui_event(&msg);
        if let Err(err) = app.emit("lounge://nats", event) {
            log::debug!("tauri emit lounge://nats: {err}");
        }
    }
    Ok(())
}

fn to_ui_event(msg: &nats::Message) -> NatsUiEvent {
    let subject = msg.subject.clone();
    let bytes = msg.data.len();
    let payload_kb = format!("{:.1}kb", bytes as f32 / 1024.0);
    let (from, to) = route_from_payload(&msg.data, &subject);
    NatsUiEvent {
        id: Uuid::new_v4().to_string(),
        time: Utc::now().format("%H:%M:%S%.3f").to_string(),
        subject: subject.clone(),
        from,
        to,
        payload: payload_kb,
        state: state_for(&subject),
    }
}

fn route_from_payload(data: &[u8], subject: &str) -> (String, String) {
    let json: serde_json::Value = serde_json::from_slice(data).unwrap_or(serde_json::Value::Null);
    let from = json
        .get("source_agent")
        .or_else(|| json.get("agent"))
        .or_else(|| json.pointer("/task/source_agent"))
        .and_then(|v| v.as_str())
        .unwrap_or("nats")
        .to_string();
    let to = json
        .get("target_agent")
        .or_else(|| json.pointer("/task/target_agent"))
        .and_then(|v| v.as_str())
        .unwrap_or(match subject {
            TASK_ASSIGNED => "agent",
            TASK_COMPLETED | TASK_FAILED | EXPERIENCE_REPORTED => "nats",
            TASK_REQUESTED => "dispatcher",
            _ => "kernel",
        })
        .to_string();
    (from, to)
}

fn state_for(subject: &str) -> String {
    if subject.ends_with(".failed") {
        "error".into()
    } else if subject.ends_with(".requested") {
        "queued".into()
    } else {
        "ok".into()
    }
}
