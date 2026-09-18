use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::process::{Child, Command};

use super::probe::{
    endpoint, find_executable, tcp_ready, wait_until, DEFAULT_NATS_HOST, DEFAULT_NATS_PORT,
};
use crate::models::{ServiceHealth, ServiceId};

const HEALTH_TIMEOUT: Duration = Duration::from_millis(400);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(150);

#[derive(Debug, Clone)]
pub struct NatsConfig {
    pub host: String,
    pub port: u16,
    pub binary: String,
    pub args: Vec<String>,
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_NATS_HOST.to_string(),
            port: DEFAULT_NATS_PORT,
            binary: "nats-server".to_string(),
            args: Vec::new(),
        }
    }
}

pub struct NatsService {
    config: NatsConfig,
    child: Option<Child>,
    started_by_us: bool,
}

impl NatsService {
    pub fn new() -> Self {
        Self::with_config(NatsConfig::default())
    }

    pub fn with_config(config: NatsConfig) -> Self {
        Self {
            config,
            child: None,
            started_by_us: false,
        }
    }

    pub fn endpoint(&self) -> String {
        format!("nats://{}", endpoint(&self.config.host, self.config.port))
    }

    pub async fn is_healthy(&self) -> bool {
        tcp_ready(&self.config.host, self.config.port, HEALTH_TIMEOUT).await
    }

    pub async fn ensure(&mut self) -> ServiceHealth {
        match self.ensure_inner().await {
            Ok(health) => health,
            Err(err) => {
                ServiceHealth::down(ServiceId::Nats, "NATS", self.endpoint(), err.to_string())
            }
        }
    }

    pub fn snapshot(
        &self,
        running: bool,
        detail: Option<String>,
        error: Option<String>,
    ) -> ServiceHealth {
        ServiceHealth {
            id: ServiceId::Nats,
            name: "NATS".into(),
            running,
            started_by_us: self.started_by_us,
            endpoint: self.endpoint(),
            detail,
            error,
        }
    }

    async fn ensure_inner(&mut self) -> Result<ServiceHealth> {
        self.reap_exited_child();

        if self.is_healthy().await {
            return Ok(self.snapshot(true, Some("tcp kabul ediyor".into()), None));
        }

        let binary = find_executable(&self.config.binary).ok_or_else(|| {
            anyhow::anyhow!(
                "nats-server bulunamadı (PATH). Yerel bus: {}",
                self.endpoint()
            )
        })?;

        let mut args = vec![
            "-a".to_string(),
            self.config.host.clone(),
            "-p".to_string(),
            self.config.port.to_string(),
        ];
        args.extend(self.config.args.iter().cloned());

        let mut command = Command::new(&binary);
        command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        apply_no_window(&mut command);

        let child = command
            .spawn()
            .with_context(|| format!("NATS başlatılamadı: {}", binary.display()))?;
        self.child = Some(child);
        self.started_by_us = true;
        log::info!("NATS spawn edildi: {}", binary.display());

        let host = self.config.host.clone();
        let port = self.config.port;
        let ready = wait_until(STARTUP_TIMEOUT, POLL_INTERVAL, move || {
            let host = host.clone();
            async move { tcp_ready(&host, port, HEALTH_TIMEOUT).await }
        })
        .await;
        if !ready {
            self.kill_child().await;
            anyhow::bail!(
                "NATS {timeout:?} içinde {endpoint} üzerinde ayağa kalkmadı",
                timeout = STARTUP_TIMEOUT,
                endpoint = self.endpoint()
            );
        }

        Ok(self.snapshot(true, Some("kernel tarafından başlatıldı".into()), None))
    }

    fn reap_exited_child(&mut self) {
        if let Some(child) = self.child.as_mut() {
            if let Ok(Some(status)) = child.try_wait() {
                log::warn!("NATS süreci sonlandı: {status}");
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

impl Default for NatsService {
    fn default() -> Self {
        Self::new()
    }
}

fn apply_no_window(_command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        _command.creation_flags(CREATE_NO_WINDOW);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn skips_spawn_when_port_already_open() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut service = NatsService::with_config(NatsConfig {
            host: "127.0.0.1".into(),
            port,
            binary: "__missing_nats__".into(),
            args: vec!["-p".into(), port.to_string()],
        });

        let health = service.ensure().await;
        assert!(health.running);
        assert!(!health.started_by_us);
        assert!(health.error.is_none());
    }

    #[tokio::test]
    async fn reports_missing_binary_when_port_closed() {
        let mut service = NatsService::with_config(NatsConfig {
            host: "127.0.0.1".into(),
            port: 1,
            binary: "__missing_nats__".into(),
            args: vec!["-p".into(), "1".into()],
        });
        let health = service.ensure().await;
        assert!(!health.running);
        assert!(health.error.unwrap().contains("nats-server bulunamadı"));
    }
}
