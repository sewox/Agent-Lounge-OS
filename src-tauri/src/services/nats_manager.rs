#![allow(deprecated)]

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use lounge_protocol::{LoungeMessage, UI_EVENT, WILDCARD};
use tauri::{AppHandle, Emitter, Manager};
use tokio::process::{Child, Command};

use super::probe::{
    endpoint, find_executable, lounge_nats_dir, tcp_ready, wait_until, DEFAULT_NATS_HOST,
    DEFAULT_NATS_HTTP_PORT, DEFAULT_NATS_PORT,
};
use crate::kernel::{DecisionGate, GuardedCommand};
use crate::models::{ServiceHealth, ServiceId};

const HEALTH_TIMEOUT: Duration = Duration::from_millis(400);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(150);
const EVENT_RECONNECT: Duration = Duration::from_secs(2);

/// Event pump NATS konusu — `lounge.>` (tek seviye altındaki tüm `lounge.*` değil, `>` ile tüm derinlik).
pub(crate) const EVENT_PUMP_SUBJECT: &str = WILDCARD;

pub fn default_nats_url() -> String {
    format!("nats://{}:{}", DEFAULT_NATS_HOST, DEFAULT_NATS_PORT)
}

/// `127.0.0.1:4222` üzerindeki `lounge.>` trafiğini dinler, her mesajı `nats-event` olarak UI'ya fırlatır.
pub fn spawn_event_pump(app: AppHandle) {
    spawn_event_pump_url(app, default_nats_url());
}

pub fn spawn_event_pump_url(app: AppHandle, nats_url: impl Into<String>) {
    let nats_url = nats_url.into();
    tauri::async_runtime::spawn(async move {
        loop {
            let app = app.clone();
            let url = nats_url.clone();
            match tokio::task::spawn_blocking(move || listen_once(&app, &url)).await {
                Ok(Ok(())) => {
                    log::warn!("NATS event pump bağlantısı kapandı, yeniden bağlanılıyor");
                }
                Ok(Err(err)) => log::warn!("NATS event pump: {err}"),
                Err(err) => log::warn!("NATS event pump join: {err}"),
            }
            tokio::time::sleep(EVENT_RECONNECT).await;
        }
    });
}

pub(crate) fn listen_once(app: &AppHandle, url: &str) -> Result<()> {
    // `nats::Connection` Inner Drop client.shutdown() çağırır. Subscription Client
    // tutsa bile bağlantı kapanır ve `messages()` hemen biter — `_nc` pump boyunca yaşar.
    let (_nc, sub) = connect_and_subscribe(url)?;
    log::info!("NATS event pump dinliyor: {url} ({EVENT_PUMP_SUBJECT})");
    for msg in sub.messages() {
        let envelope = LoungeMessage::from_nats(&msg.subject, &msg.data);
        if let Some(gate) = app.try_state::<DecisionGate>() {
            gate.infer_async(&envelope);
        }
        emit_nats_event(app, &envelope);
    }
    Ok(())
}

pub(crate) fn connect_and_subscribe(url: &str) -> Result<(nats::Connection, nats::Subscription)> {
    let nc = nats::connect(url).with_context(|| format!("NATS bağlanamadı: {url}"))?;
    let sub = nc
        .subscribe(EVENT_PUMP_SUBJECT)
        .with_context(|| format!("subscribe {EVENT_PUMP_SUBJECT} başarısız"))?;
    Ok((nc, sub))
}

fn emit_nats_event(app: &AppHandle, envelope: &LoungeMessage) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(err) = window.emit(UI_EVENT, envelope) {
            log::debug!("tauri::Window::emit {UI_EVENT}: {err}");
        }
        return;
    }
    if let Err(err) = app.emit(UI_EVENT, envelope) {
        log::debug!("main window yok, app emit {UI_EVENT}: {err}");
    }
}

#[derive(Debug, Clone)]
pub struct NatsConfig {
    pub host: String,
    pub port: u16,
    pub http_port: u16,
    pub binary: String,
    pub args: Vec<String>,
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_NATS_HOST.to_string(),
            port: DEFAULT_NATS_PORT,
            http_port: DEFAULT_NATS_HTTP_PORT,
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

    pub fn monitor_url(&self) -> String {
        format!(
            "http://{}",
            endpoint(&self.config.host, self.config.http_port)
        )
    }

    pub async fn is_healthy(&self) -> bool {
        tcp_ready(&self.config.host, self.config.port, HEALTH_TIMEOUT).await
    }

    pub async fn monitor_ready(&self) -> bool {
        if self.config.http_port == 0 {
            return true;
        }
        tcp_ready(&self.config.host, self.config.http_port, HEALTH_TIMEOUT).await
    }

    pub async fn is_fully_healthy(&self) -> bool {
        self.is_healthy().await && self.monitor_ready().await
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

        let client_ok = self.is_healthy().await;
        let monitor_ok = self.monitor_ready().await;
        if client_ok && monitor_ok {
            return Ok(self.snapshot(true, Some("tcp kabul ediyor".into()), None));
        }

        if client_ok && !monitor_ok {
            let killed = kill_nats_on_port(self.config.port);
            if killed == 0 {
                return Ok(self.snapshot(true, Some("tcp kabul ediyor".into()), None));
            }
            log::error!(
                "kritik servis down: NATS HTTP monitor ({}) — {} nats-server geri alındı",
                self.monitor_url(),
                killed
            );
            self.kill_child().await;
            let host = self.config.host.clone();
            let port = self.config.port;
            let _ = wait_until(
                Duration::from_secs(2),
                Duration::from_millis(80),
                move || {
                    let host = host.clone();
                    async move { !tcp_ready(&host, port, HEALTH_TIMEOUT).await }
                },
            )
            .await;
        }

        match self.spawn_and_wait(true).await {
            Ok(health) => Ok(health),
            Err(err) if self.config.http_port > 0 => {
                log::warn!("NATS HTTP monitor açılamadı, yalnızca client port: {err}");
                self.spawn_and_wait(false).await
            }
            Err(err) => Err(err),
        }
    }

    async fn spawn_and_wait(&mut self, with_monitor: bool) -> Result<ServiceHealth> {
        let binary = find_executable(&self.config.binary).ok_or_else(|| {
            anyhow::anyhow!(
                "nats-server bulunamadı (PATH). Yerel bus: {}",
                self.endpoint()
            )
        })?;
        let args = nats_server_args(&self.config, with_monitor);

        let mut command = Command::new(&binary);
        command.args(&args).stdin(Stdio::null()).kill_on_drop(true);
        attach_nats_log(&mut command);
        apply_no_window(&mut command);

        let child = command
            .spawn()
            .with_context(|| format!("NATS başlatılamadı: {}", binary.display()))?;
        self.child = Some(child);
        self.started_by_us = true;
        log::info!(
            "NATS spawn edildi: {} → {} (monitor={})",
            binary.display(),
            self.endpoint(),
            with_monitor
        );

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
                log::error!("kritik servis down: NATS süreci sonlandı ({status})");
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
        // tokio::process::Command exposes creation_flags on Windows directly.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        _command.creation_flags(CREATE_NO_WINDOW);
    }
}

pub(crate) fn nats_server_args(config: &NatsConfig, with_monitor: bool) -> Vec<String> {
    let mut args = vec![
        "-a".to_string(),
        config.host.clone(),
        "-p".to_string(),
        config.port.to_string(),
    ];
    if with_monitor && config.http_port > 0 {
        args.push("-m".to_string());
        args.push(config.http_port.to_string());
    }
    args.extend(config.args.iter().cloned());
    args
}

fn attach_nats_log(command: &mut Command) {
    let dir = lounge_nats_dir();
    let path = dir.join("nats-server.log");
    let _ = std::fs::create_dir_all(&dir);
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
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

fn kill_nats_on_port(port: u16) -> usize {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let pids = listen_pids(port);
    if pids.is_empty() {
        return 0;
    }
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let mut killed = 0;
    for pid in pids {
        let Some(proc) = sys.process(Pid::from_u32(pid)) else {
            continue;
        };
        let name = proc.name().to_string_lossy().to_ascii_lowercase();
        if !name.contains("nats") {
            continue;
        }
        if proc.kill() {
            killed += 1;
            log::warn!("NATS recovery: pid {pid} ({name}) :{port} üzerinde sonlandırıldı");
        }
    }
    killed
}

fn listen_pids(port: u16) -> Vec<u32> {
    #[cfg(windows)]
    {
        windows_listen_pids(port)
    }
    #[cfg(not(windows))]
    {
        unix_listen_pids(port)
    }
}

#[cfg(not(windows))]
fn unix_listen_pids(port: u16) -> Vec<u32> {
    let output = GuardedCommand::new("lsof")
        .args(["-nP", "-t", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
        .internal_daemon()
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

#[cfg(windows)]
fn windows_listen_pids(port: u16) -> Vec<u32> {
    let output = GuardedCommand::new("netstat")
        .args(["-ano", "-p", "tcp"])
        .internal_daemon()
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    // Match local address tokens that end with `:{port}` — substring
    // `contains(":1")` falsely hits `:135`, `:139`, `:445`, etc.
    let needle = format!(":{port}");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| {
            let upper = line.to_ascii_uppercase();
            if !upper.contains("LISTEN") {
                return false;
            }
            line.split_whitespace()
                .any(|token| token.ends_with(&needle))
        })
        .filter_map(|line| line.split_whitespace().last()?.parse().ok())
        .collect()
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
            http_port: 0,
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
            ..NatsConfig::default()
        });
        let health = service.ensure().await;
        assert!(!health.running);
        assert!(health.error.unwrap().contains("nats-server bulunamadı"));
    }

    #[test]
    fn event_name_is_nats_event() {
        assert_eq!(UI_EVENT, "nats-event");
        assert_eq!(WILDCARD, "lounge.>");
        assert_eq!(default_nats_url(), "nats://127.0.0.1:4222");
    }

    #[test]
    fn event_pump_subscribe_subject_is_wildcard() {
        assert_eq!(EVENT_PUMP_SUBJECT, "lounge.>");
        assert_eq!(EVENT_PUMP_SUBJECT, WILDCARD);
        assert!(
            EVENT_PUMP_SUBJECT.ends_with('>'),
            "pump must use NATS wildcard, got {EVENT_PUMP_SUBJECT}"
        );
    }

    #[test]
    fn connect_and_subscribe_does_not_panic_when_nats_down() {
        let err = connect_and_subscribe("nats://127.0.0.1:1").expect_err("closed port");
        let text = err.to_string();
        assert!(
            text.contains("NATS bağlanamadı") || text.contains("connection") || text.contains("1"),
            "{text}"
        );
    }

    #[test]
    fn event_pump_receives_non_bus_message_on_wildcard() {
        let Some((url, mut child)) = spawn_ephemeral_nats() else {
            eprintln!("skip: nats-server bulunamadı");
            return;
        };
        let result = (|| -> Result<()> {
            let (nc, sub) = connect_and_subscribe(&url)?;
            let payload = br#"{"id":"pump-infer","type":"task","source_agent":"test"}"#;
            nc.publish("lounge.task.requested", &payload[..])
                .context("publish task")?;
            nc.flush().context("flush")?;
            let msg = sub
                .next_timeout(Duration::from_secs(2))
                .context("wildcard lounge.> görev mesajı gelmedi")?;
            assert_eq!(msg.subject, "lounge.task.requested");
            let envelope = LoungeMessage::from_nats(&msg.subject, &msg.data);
            assert!(
                !crate::kernel::decision_engine::skip_bus_subject(&envelope.subject),
                "non-bus subject should trigger infer"
            );
            Ok(())
        })();
        let _ = child.kill();
        let _ = child.wait();
        result.expect("event pump wildcard subscribe");
    }

    fn spawn_ephemeral_nats() -> Option<(String, std::process::Child)> {
        let binary = find_executable("nats-server")?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").ok()?;
        let port = listener.local_addr().ok()?.port();
        drop(listener);
        let mut command = GuardedCommand::new(binary)
            .args(["-a", "127.0.0.1", "-p", &port.to_string()])
            .internal_daemon()
            .into_std_command()
            .ok()?;
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let url = format!("nats://127.0.0.1:{port}");
        for _ in 0..80 {
            if nats::connect(&url).is_ok() {
                return Some((url, child));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
        None
    }

    #[test]
    fn spawn_args_include_http_monitor() {
        let config = NatsConfig::default();
        let args = nats_server_args(&config, true);
        assert!(args.windows(2).any(|pair| pair == ["-m", "8222"]));
        assert!(args.windows(2).any(|pair| pair == ["-p", "4222"]));
        let without = nats_server_args(&config, false);
        assert!(!without.iter().any(|arg| arg == "-m"));
    }

    #[test]
    fn unused_port_has_no_nats_listeners() {
        assert!(listen_pids(1).is_empty());
        assert_eq!(kill_nats_on_port(1), 0);
    }
}
