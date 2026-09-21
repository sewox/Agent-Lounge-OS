//! Kritik yerel servisler (NATS, LMR) düşerse log + yeniden başlatma.

use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use tokio::time::sleep;

use super::SharedServices;
use crate::models::{ServiceReport, SERVICE_EVENT};

const TICK: Duration = Duration::from_secs(2);
const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

pub fn spawn_supervisor(app: AppHandle, services: SharedServices) {
    tauri::async_runtime::spawn(async move {
        run_supervisor(app, services).await;
    });
}

async fn run_supervisor(app: AppHandle, services: SharedServices) {
    let mut nats = Backoff::new();
    let mut lmr = Backoff::new();
    let mut last: Option<CoreHealth> = None;
    loop {
        let report = {
            let mut manager = services.lock().await;
            supervise_once(&mut manager, &mut nats, &mut lmr).await
        };
        let core = CoreHealth::from(&report);
        if last.as_ref() != Some(&core) {
            emit_service_status(&app, &report);
            last = Some(core);
        }
        sleep(TICK).await;
    }
}

async fn supervise_once(
    manager: &mut super::ServiceManager,
    nats: &mut Backoff,
    lmr: &mut Backoff,
) -> ServiceReport {
    let nats_health = recover_nats(manager, nats).await;
    let ollama_health = recover_lmr(manager, lmr).await;
    ServiceReport {
        ollama: ollama_health,
        nats: nats_health,
        memory: manager.memory().diagnose(),
        plugin: super::plugin_snapshot(),
    }
}

async fn recover_nats(
    manager: &mut super::ServiceManager,
    backoff: &mut Backoff,
) -> crate::models::ServiceHealth {
    if manager.nats.is_fully_healthy().await {
        backoff.reset();
        return manager
            .nats
            .snapshot(true, Some("tcp kabul ediyor".into()), None);
    }
    if !backoff.ready() {
        return crate::models::ServiceHealth::down(
            crate::models::ServiceId::Nats,
            "NATS",
            manager.nats.endpoint(),
            format!("yeniden deneme {}s", backoff.remaining_secs()),
        );
    }
    log::error!(
        "kritik servis down: NATS ({}) — recovery başlıyor",
        manager.nats.endpoint()
    );
    let health = manager.nats.ensure().await;
    if health.running {
        log::info!("NATS recovery: ayağa kalktı ({})", health.endpoint);
        backoff.reset();
    } else {
        let reason = health
            .error
            .clone()
            .unwrap_or_else(|| "bilinmeyen hata".into());
        log::error!("NATS recovery başarısız: {reason}");
        backoff.fail();
    }
    health
}

async fn recover_lmr(
    manager: &mut super::ServiceManager,
    backoff: &mut Backoff,
) -> crate::models::ServiceHealth {
    if manager.ollama.is_healthy().await {
        backoff.reset();
        return manager.ollama.snapshot(true, None, None);
    }
    if !backoff.ready() {
        return crate::models::ServiceHealth::down(
            crate::models::ServiceId::Ollama,
            "LMR",
            manager.ollama.endpoint(),
            format!("yeniden deneme {}s", backoff.remaining_secs()),
        );
    }
    log::error!(
        "kritik servis down: LMR ({}) — recovery başlıyor",
        manager.ollama.endpoint()
    );
    let health = manager.ollama.ensure().await;
    if health.running {
        log::info!("LMR recovery: ayağa kalktı ({})", health.endpoint);
        backoff.reset();
    } else {
        let reason = health
            .error
            .clone()
            .unwrap_or_else(|| "bilinmeyen hata".into());
        log::error!("LMR recovery başarısız: {reason}");
        backoff.fail();
    }
    health
}

fn emit_service_status(app: &AppHandle, report: &ServiceReport) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(err) = window.emit(SERVICE_EVENT, report) {
            log::debug!("{SERVICE_EVENT} window emit: {err}");
        }
        return;
    }
    if let Err(err) = app.emit(SERVICE_EVENT, report) {
        log::debug!("{SERVICE_EVENT} app emit: {err}");
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CoreHealth {
    nats: bool,
    lmr: bool,
}

impl CoreHealth {
    fn from(report: &ServiceReport) -> Self {
        Self {
            nats: report.nats.running,
            lmr: report.ollama.running,
        }
    }
}

struct Backoff {
    next_at: Instant,
    current: Duration,
}

impl Backoff {
    fn new() -> Self {
        Self {
            next_at: Instant::now(),
            current: BACKOFF_MIN,
        }
    }

    fn ready(&self) -> bool {
        Instant::now() >= self.next_at
    }

    fn remaining_secs(&self) -> u64 {
        self.next_at
            .saturating_duration_since(Instant::now())
            .as_secs()
            .max(1)
    }

    fn reset(&mut self) {
        self.current = BACKOFF_MIN;
        self.next_at = Instant::now();
    }

    fn fail(&mut self) {
        self.next_at = Instant::now() + self.current;
        self.current = next_backoff(self.current);
    }
}

pub fn next_backoff(current: Duration) -> Duration {
    current.saturating_mul(2).clamp(BACKOFF_MIN, BACKOFF_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_until_cap() {
        assert_eq!(next_backoff(Duration::from_secs(2)), Duration::from_secs(4));
        assert_eq!(
            next_backoff(Duration::from_secs(16)),
            Duration::from_secs(30)
        );
        assert_eq!(
            next_backoff(Duration::from_secs(30)),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn service_event_name() {
        assert_eq!(SERVICE_EVENT, "service-status");
    }
}
