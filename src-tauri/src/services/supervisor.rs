//! Kritik yerel servisler (NATS, LMR) için periyodik health check + auto-restart.
//!
//! Host sistem Ollama (:11434) izlenmez; yalnızca ServiceManager’ın sahip olduğu
//! Lounge LMR (loopback private runtime) ve NATS yeniden başlatılır.

use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use tokio::time::sleep;

use super::SharedServices;
use crate::models::{ServiceHealth, ServiceId, ServiceReport, SERVICE_EVENT};

const TICK: Duration = Duration::from_secs(2);
const BACKOFF_MIN: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(30);
/// Üstel backoff sonrası üst sınır; aşılınca auto-restart durur (manuel/health gelene kadar).
const MAX_RESTART_ATTEMPTS: u32 = 5;

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
        // Degraded iken her tick emit: UI countdown / Service Degraded banner güncellenir.
        if last.as_ref() != Some(&core) || core.degraded() {
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

async fn recover_nats(manager: &mut super::ServiceManager, backoff: &mut Backoff) -> ServiceHealth {
    if manager.nats.is_fully_healthy().await {
        backoff.reset();
        return manager.nats.snapshot(true, Some("health ok".into()), None);
    }

    if let Some(health) = degraded_wait(ServiceId::Nats, "NATS", manager.nats.endpoint(), backoff) {
        return health;
    }

    log::error!(
        "kritik servis down: NATS ({}) — recovery başlıyor",
        manager.nats.endpoint()
    );
    let health = manager.nats.ensure().await;
    finalize_recovery("NATS", health, backoff)
}

async fn recover_lmr(manager: &mut super::ServiceManager, backoff: &mut Backoff) -> ServiceHealth {
    if manager.ollama.is_healthy().await {
        backoff.reset();
        return manager
            .ollama
            .snapshot(true, Some("health ok".into()), None);
    }

    if let Some(health) =
        degraded_wait(ServiceId::Ollama, "LMR", manager.ollama.endpoint(), backoff)
    {
        return health;
    }

    log::error!(
        "kritik servis down: LMR ({}) — recovery başlıyor",
        manager.ollama.endpoint()
    );
    let health = manager.ollama.ensure().await;
    finalize_recovery("LMR", health, backoff)
}

fn degraded_wait(
    id: ServiceId,
    label: &str,
    endpoint: String,
    backoff: &Backoff,
) -> Option<ServiceHealth> {
    if backoff.exhausted() {
        return Some(ServiceHealth::down(
            id,
            label,
            endpoint,
            format!(
                "Service Degraded — auto-restart limiti aşıldı ({MAX_RESTART_ATTEMPTS} deneme)"
            ),
        ));
    }
    if !backoff.ready() {
        return Some(ServiceHealth::down(
            id,
            label,
            endpoint,
            format!(
                "Service Degraded — yeniden deneme {}s (deneme {}/{MAX_RESTART_ATTEMPTS})",
                backoff.remaining_secs(),
                backoff.attempts() + 1
            ),
        ));
    }
    None
}

fn finalize_recovery(
    label: &str,
    mut health: ServiceHealth,
    backoff: &mut Backoff,
) -> ServiceHealth {
    if health.running {
        log::info!("{label} recovery: ayağa kalktı ({})", health.endpoint);
        backoff.reset();
        return health;
    }
    let reason = health
        .error
        .clone()
        .unwrap_or_else(|| "bilinmeyen hata".into());
    log::error!("{label} recovery başarısız: {reason}");
    backoff.fail();
    health.error = Some(if backoff.exhausted() {
        format!(
            "Service Degraded — auto-restart limiti aşıldı ({MAX_RESTART_ATTEMPTS} deneme): {reason}"
        )
    } else {
        format!(
            "Service Degraded — restart denemesi başarısız, {}s sonra (deneme {}/{MAX_RESTART_ATTEMPTS}): {reason}",
            backoff.remaining_secs(),
            backoff.attempts(),
        )
    });
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

    fn degraded(&self) -> bool {
        !(self.nats && self.lmr)
    }
}

struct Backoff {
    next_at: Instant,
    current: Duration,
    attempts: u32,
    exhausted: bool,
}

impl Backoff {
    fn new() -> Self {
        Self {
            next_at: Instant::now(),
            current: BACKOFF_MIN,
            attempts: 0,
            exhausted: false,
        }
    }

    fn attempts(&self) -> u32 {
        self.attempts
    }

    fn exhausted(&self) -> bool {
        self.exhausted
    }

    fn ready(&self) -> bool {
        !self.exhausted && Instant::now() >= self.next_at
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
        self.attempts = 0;
        self.exhausted = false;
    }

    fn fail(&mut self) {
        self.attempts = self.attempts.saturating_add(1);
        if self.attempts >= MAX_RESTART_ATTEMPTS {
            self.exhausted = true;
            self.next_at = Instant::now() + BACKOFF_MAX;
            return;
        }
        self.next_at = Instant::now() + self.current;
        self.current = next_backoff(self.current);
    }
}

pub fn next_backoff(current: Duration) -> Duration {
    current.saturating_mul(2).clamp(BACKOFF_MIN, BACKOFF_MAX)
}

pub fn max_restart_attempts() -> u32 {
    MAX_RESTART_ATTEMPTS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::memory_bridge::MemoryBridge;
    use crate::services::nats_manager::{NatsConfig, NatsService};
    use crate::services::ollama::{OllamaConfig, OllamaService};
    use crate::services::ServiceManager;

    /// Gerçek crash yok: ensure sonuçlarını sayarak restart denemesini doğrula.
    fn probe_restart_attempt(healthy: bool, backoff: &mut Backoff, ensure_ok: bool) -> (bool, u32) {
        if healthy {
            backoff.reset();
            return (true, 0);
        }
        if backoff.exhausted() || !backoff.ready() {
            return (false, 0);
        }
        if ensure_ok {
            backoff.reset();
            (true, 1)
        } else {
            backoff.fail();
            (false, 1)
        }
    }

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
    fn backoff_caps_restart_attempts() {
        let mut backoff = Backoff::new();
        assert!(backoff.ready());
        for i in 1..=MAX_RESTART_ATTEMPTS {
            backoff.fail();
            assert_eq!(backoff.attempts(), i);
            if i < MAX_RESTART_ATTEMPTS {
                assert!(!backoff.exhausted());
                assert!(!backoff.ready());
            }
        }
        assert!(backoff.exhausted());
        assert!(!backoff.ready());
        backoff.reset();
        assert!(!backoff.exhausted());
        assert_eq!(backoff.attempts(), 0);
        assert!(backoff.ready());
    }

    #[test]
    fn core_health_marks_degraded() {
        let report = ServiceReport {
            ollama: ServiceHealth::down(ServiceId::Ollama, "LMR", "http://127.0.0.1:18790", "down"),
            nats: ServiceHealth {
                id: ServiceId::Nats,
                name: "NATS".into(),
                running: true,
                started_by_us: false,
                endpoint: "nats://127.0.0.1:4222".into(),
                detail: None,
                error: None,
            },
            memory: ServiceHealth::down(ServiceId::MemoryBridge, "Memory", "", "n/a"),
            plugin: ServiceHealth::down(ServiceId::Plugin, "Plugin", "", "n/a"),
        };
        let core = CoreHealth::from(&report);
        assert!(core.degraded());
        assert!(report.core_degraded());
        assert_eq!(report.degraded_core_names(), vec!["LMR"]);
    }

    #[test]
    fn service_event_name() {
        assert_eq!(SERVICE_EVENT, "service-status");
    }

    #[test]
    fn probe_restart_attempts_without_crash() {
        let mut backoff = Backoff::new();
        let (up, calls) = probe_restart_attempt(false, &mut backoff, false);
        assert!(!up);
        assert_eq!(calls, 1);
        assert_eq!(backoff.attempts(), 1);

        // Backoff penceresinde ikinci deneme ensure çağırmaz.
        let (up2, calls2) = probe_restart_attempt(false, &mut backoff, false);
        assert!(!up2);
        assert_eq!(calls2, 0);

        // Sağlık gelince degraded temizlenir.
        let (up3, calls3) = probe_restart_attempt(true, &mut backoff, false);
        assert!(up3);
        assert_eq!(calls3, 0);
        assert_eq!(backoff.attempts(), 0);
    }

    #[test]
    fn probe_respects_retry_cap() {
        let mut backoff = Backoff::new();
        for _ in 0..MAX_RESTART_ATTEMPTS {
            // ready yapmak için fail sonrası next_at’i geçmişe al
            let _ = probe_restart_attempt(false, &mut backoff, false);
            backoff.next_at = Instant::now();
        }
        assert!(backoff.exhausted());
        let (up, calls) = probe_restart_attempt(false, &mut backoff, true);
        assert!(!up);
        assert_eq!(calls, 0);
    }

    #[tokio::test]
    async fn supervise_once_marks_degraded_without_real_crash() {
        let mut manager = ServiceManager::for_test(
            ollama_stub_missing(),
            nats_stub_listening().await,
            MemoryBridge::from_binary("/tmp/missing-codebase-memory-mcp"),
        );
        let mut nats = Backoff::new();
        let mut lmr = Backoff::new();

        let report = supervise_once(&mut manager, &mut nats, &mut lmr).await;
        assert!(report.core_degraded());
        assert!(report.degraded_core_names().contains(&"LMR"));
        assert!(!report.ollama.running);
        assert!(report
            .ollama
            .error
            .as_deref()
            .unwrap_or("")
            .contains("Service Degraded"));
        assert!(lmr.attempts() >= 1);
        // NATS TCP açık + http_port=0 → healthy, restart denemesi yok
        assert!(report.nats.running);
        assert_eq!(nats.attempts(), 0);
    }

    fn ollama_stub_missing() -> OllamaService {
        OllamaService::with_config(OllamaConfig {
            host: "127.0.0.1".into(),
            port: 1,
            binary: "__missing_lounge_lmr__".into(),
            args: vec!["serve".into()],
            models_dir: None,
        })
    }

    async fn nats_stub_listening() -> NatsService {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        std::mem::forget(listener);
        NatsService::with_config(NatsConfig {
            host: "127.0.0.1".into(),
            port,
            http_port: 0,
            binary: "__missing_nats__".into(),
            args: vec![],
        })
    }
}
