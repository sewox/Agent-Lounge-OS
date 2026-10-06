#![allow(deprecated)]

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};
use lounge_protocol::{LoungeMessage, UI_EVENT, WILDCARD};
use tauri::{AppHandle, Emitter, Manager};
use tokio::process::{Child, Command};

use super::lounge_auth::{
    activate_nats_auth, auth_required, connect as nats_connect, current_credentials,
    deactivate_nats_auth, delete_nats_server_conf, ensure_session_credentials,
    nats_server_conf_path, rotate_session_credentials, write_nats_server_conf, NatsCredentials,
};
use super::probe::{
    endpoint, find_executable, listen_pids, lounge_nats_dir, tcp_ready, wait_until,
    DEFAULT_NATS_HOST, DEFAULT_NATS_HTTP_PORT, DEFAULT_NATS_PORT,
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
    let nc = nats_connect(url).with_context(|| format!("NATS bağlanamadı: {url}"))?;
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
    /// When set, nats-server is started with `-c nats-server.conf` (bcrypt auth).
    pub credentials: Option<NatsCredentials>,
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_NATS_HOST.to_string(),
            port: DEFAULT_NATS_PORT,
            http_port: DEFAULT_NATS_HTTP_PORT,
            binary: "nats-server".to_string(),
            args: Vec::new(),
            credentials: None,
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

    pub fn credentials(&self) -> Option<&NatsCredentials> {
        self.config.credentials.as_ref()
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

    /// `nats-server` on PATH or an absolute path that exists (optional local bus).
    pub fn runtime_installed(&self) -> bool {
        resolve_nats_binary(&self.config.binary).is_some()
    }

    pub fn not_installed_health(&self) -> ServiceHealth {
        ServiceHealth::not_installed(
            ServiceId::Nats,
            "NATS",
            self.endpoint(),
            "optional — install nats-server on PATH for the local bus",
        )
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
            availability: None,
            code: None,
        }
    }

    async fn ensure_inner(&mut self) -> Result<ServiceHealth> {
        self.reap_exited_child();
        self.prepare_auth_credentials()?;

        let mut force_respawn = false;
        let client_ok = self.is_healthy().await;
        let monitor_ok = self.monitor_ready().await;
        if client_ok && monitor_ok {
            match &self.config.credentials {
                None => {
                    // Auth bypass / legacy: reuse open TCP without NATS handshake probe
                    // (a bare listener is not a NATS server — connect would hang).
                    deactivate_nats_auth();
                    return Ok(self.snapshot(true, Some("tcp kabul ediyor".into()), None));
                }
                Some(_) if listening_nats_has_pass_flag(self.config.port) => {
                    // Legacy argv leak: do not adopt — rotate once, kill, restart with conf.
                    log::warn!(
                        "NATS :{} argv contains --pass — rotating credentials and restarting with conf",
                        self.config.port
                    );
                    let creds = rotate_session_credentials(&self.endpoint())?;
                    self.config.credentials = Some(creds);
                    let _ = kill_nats_on_port(self.config.port);
                    self.kill_child().await;
                    wait_port_free(&self.config.host, self.config.port).await;
                    force_respawn = true;
                }
                Some(_) if self.auth_connect_ok() => {
                    self.mark_auth_active();
                    return Ok(self.snapshot(true, Some("tcp kabul ediyor".into()), None));
                }
                Some(_) => {
                    log::warn!(
                        "NATS :{} açık ama oturum kimlik bilgileriyle bağlanılamadı — yeniden başlatılıyor",
                        self.config.port
                    );
                    let _ = kill_nats_on_port(self.config.port);
                    self.kill_child().await;
                    wait_port_free(&self.config.host, self.config.port).await;
                    force_respawn = true;
                }
            }
        }

        if client_ok && !monitor_ok && !force_respawn {
            let killed = kill_nats_on_port(self.config.port);
            if killed == 0 {
                // Nothing nats-like to kill on the port. Adopt open TCP unless we must
                // migrate a legacy `--pass` argv server.
                if self.config.credentials.is_some()
                    && listening_nats_has_pass_flag(self.config.port)
                {
                    log::warn!(
                        "NATS :{} monitor yok ve argv --pass — rotate + restart",
                        self.config.port
                    );
                    let creds = rotate_session_credentials(&self.endpoint())?;
                    self.config.credentials = Some(creds);
                    let _ = kill_nats_on_port(self.config.port);
                    self.kill_child().await;
                    wait_port_free(&self.config.host, self.config.port).await;
                    // Fall through to spawn_and_wait below.
                } else {
                    if self.config.credentials.is_none() {
                        deactivate_nats_auth();
                    } else if self.auth_connect_ok() {
                        self.mark_auth_active();
                    }
                    return Ok(self.snapshot(true, Some("tcp kabul ediyor".into()), None));
                }
            } else {
                log::error!(
                    "kritik servis down: NATS HTTP monitor ({}) — {} nats-server geri alındı",
                    self.monitor_url(),
                    killed
                );
                self.kill_child().await;
                wait_port_free(&self.config.host, self.config.port).await;
            }
        }

        if !self.runtime_installed() {
            return Ok(self.not_installed_health());
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

    fn prepare_auth_credentials(&mut self) -> Result<()> {
        if !auth_required() {
            self.config.credentials = None;
            deactivate_nats_auth();
            return Ok(());
        }
        let creds = ensure_session_credentials(&self.endpoint())?
            .or_else(current_credentials)
            .context("LOUNGE_AUTH_REQUIRED but failed to mint NATS credentials")?;
        self.config.credentials = Some(creds);
        Ok(())
    }

    fn auth_connect_ok(&self) -> bool {
        let url = self.endpoint();
        let Some(creds) = &self.config.credentials else {
            return false;
        };
        // Fail fast — do not hang on a non-NATS TCP listener.
        crate::services::lounge_auth::connect_nats_timeout(
            &url,
            Some((&creds.user, &creds.password)),
            Duration::from_secs(5),
        )
        .is_ok()
    }

    fn mark_auth_active(&self) {
        if let Some(creds) = self.config.credentials.clone() {
            activate_nats_auth(creds);
        } else {
            deactivate_nats_auth();
        }
    }

    async fn spawn_and_wait(&mut self, with_monitor: bool) -> Result<ServiceHealth> {
        let binary = resolve_nats_binary(&self.config.binary).ok_or_else(|| {
            anyhow::anyhow!(
                "nats-server bulunamadı (PATH). Yerel bus: {}",
                self.endpoint()
            )
        })?;
        // Rewrite conf on every launch when auth is enabled.
        if let Some(creds) = &self.config.credentials {
            write_nats_server_conf(creds)?;
        }
        let args = nats_server_args(&self.config, with_monitor);

        let std_cmd = GuardedCommand::new(&binary)
            .args(&args)
            .internal_daemon()
            .into_std_command()
            .with_context(|| format!("NATS gate başarısız: {}", binary.display()))?;
        let mut command = Command::from(std_cmd);
        command.stdin(Stdio::null()).kill_on_drop(true);
        attach_nats_log(&mut command);
        apply_no_window(&mut command);

        let child = command
            .spawn()
            .with_context(|| format!("NATS başlatılamadı: {}", binary.display()))?;
        self.child = Some(child);
        self.started_by_us = true;
        log::info!(
            "NATS spawn edildi: {} → {} (monitor={} auth={})",
            binary.display(),
            self.endpoint(),
            with_monitor,
            self.config.credentials.is_some()
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

        // Confirm auth handshake before marking the bus verified.
        if self.config.credentials.is_some() && !self.auth_connect_ok() {
            self.kill_child().await;
            anyhow::bail!("NATS ayağa kalktı ama kimlik doğrulamalı bağlantı başarısız");
        }
        self.mark_auth_active();

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
        let started_by_us = self.started_by_us;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
        }
        self.started_by_us = false;
        if started_by_us {
            delete_nats_server_conf();
        }
    }
}

impl Default for NatsService {
    fn default() -> Self {
        Self::new()
    }
}

async fn wait_port_free(host: &str, port: u16) {
    let host = host.to_string();
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

fn apply_no_window(_command: &mut Command) {
    #[cfg(windows)]
    {
        // tokio::process::Command exposes creation_flags on Windows directly.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        _command.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Absolute path that exists, else PATH lookup (`find_executable`).
fn resolve_nats_binary(requested: &str) -> Option<std::path::PathBuf> {
    let requested = requested.trim();
    if requested.is_empty() {
        return None;
    }
    let path = std::path::PathBuf::from(requested);
    if path.is_file() {
        return Some(path);
    }
    find_executable(requested)
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
    if config.credentials.is_some() {
        args.push("-c".to_string());
        args.push(nats_server_conf_path().display().to_string());
    }
    args.extend(config.args.iter().cloned());
    args
}

/// True when a nats-server listening on `port` has `--pass` in its argv (legacy leak).
fn listening_nats_has_pass_flag(port: u16) -> bool {
    let pids = listen_pids(port);
    for pid in pids {
        let cmd = process_cmdline(pid);
        if cmd.is_empty() {
            continue;
        }
        // Confirm it looks like nats before treating --pass as our leak signal.
        let looks_like_nats = cmd.iter().any(|a| {
            let lower = a.to_ascii_lowercase();
            lower.contains("nats-server") || lower.ends_with("nats-server.exe")
        });
        if looks_like_nats && process_cmd_has_pass(&cmd) {
            return true;
        }
    }
    false
}

fn process_cmd_has_pass(cmd: &[impl AsRef<std::ffi::OsStr>]) -> bool {
    cmd.iter().any(|a| {
        let s = a.as_ref().to_string_lossy();
        s == "--pass" || s.starts_with("--pass=")
    })
}

/// Collect argv strings for a live PID (cross-platform).
///
/// Linux prefers `/proc/{pid}/cmdline`. macOS/Windows fall back to `ps` /
/// `Get-CimInstance Win32_Process` because sysinfo often returns an empty
/// `cmd()` under CI (SIP / limited process info).
fn process_cmdline(pid: u32) -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) {
            let parts: Vec<String> = raw
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect();
            if !parts.is_empty() {
                return parts;
            }
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        let via_ps = cmdline_via_ps(pid);
        if !via_ps.is_empty() {
            return via_ps;
        }
    }

    #[cfg(windows)]
    {
        let via_cim = cmdline_via_win32_cim(pid);
        if !via_cim.is_empty() {
            return via_cim;
        }
    }

    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
        true,
        ProcessRefreshKind::everything(),
    );
    let Some(proc) = sys.process(Pid::from_u32(pid)) else {
        return Vec::new();
    };
    proc.cmd()
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect()
}

/// Tokenize a single process command-line string (handles quoted exe paths).
fn tokenize_command_line(line: &str) -> Vec<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    // Prefer shell-word splitting so `"C:\Program Files\…"` stays one token.
    let words = crate::kernel::policy_gate::shell_words(trimmed);
    if !words.is_empty() {
        return words;
    }
    trimmed.split_whitespace().map(str::to_string).collect()
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn cmdline_via_ps(pid: u32) -> Vec<String> {
    let output = GuardedCommand::new("ps")
        .args(["-p", &pid.to_string(), "-ww", "-o", "args="])
        .internal_daemon()
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let line = String::from_utf8_lossy(&output.stdout);
    tokenize_command_line(line.trim())
}

#[cfg(windows)]
fn cmdline_via_win32_cim(pid: u32) -> Vec<String> {
    // Filter by ProcessId so we only tokenize the target (counts/values never logged).
    let script = format!(
        "(Get-CimInstance Win32_Process -Filter \"ProcessId = {pid}\").CommandLine"
    );
    let output = GuardedCommand::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .internal_daemon()
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let line = String::from_utf8_lossy(&output.stdout);
    tokenize_command_line(line.trim())
}

/// Count how many process cmdlines contain `needle` (value never logged).
#[cfg(test)]
fn count_cmdline_matches(needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    #[cfg(target_os = "linux")]
    {
        let mut matches = 0usize;
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let Some(pid_str) = name.to_str() else {
                    continue;
                };
                if !pid_str.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                if let Ok(raw) = std::fs::read(entry.path().join("cmdline")) {
                    let joined = String::from_utf8_lossy(&raw);
                    if joined.contains(needle) {
                        matches += 1;
                    }
                }
            }
        }
        return matches;
    }
    #[cfg(target_os = "macos")]
    {
        let output = GuardedCommand::new("ps")
            .args(["-axww", "-o", "args="])
            .internal_daemon()
            .output();
        let Ok(output) = output else {
            return 0;
        };
        if !output.status.success() {
            return 0;
        }
        return String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains(needle))
            .count();
    }
    #[cfg(windows)]
    {
        let script = r#"Get-CimInstance Win32_Process | Select-Object -ExpandProperty CommandLine"#;
        let output = GuardedCommand::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .internal_daemon()
            .output();
        let Ok(output) = output else {
            return 0;
        };
        if !output.status.success() {
            return 0;
        }
        return String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains(needle))
            .count();
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::everything(),
        );
        let mut matches = 0usize;
        for proc in sys.processes().values() {
            for arg in proc.cmd() {
                if arg.to_string_lossy().contains(needle) {
                    matches += 1;
                    break;
                }
            }
        }
        matches
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn skips_spawn_when_port_already_open() {
        let _guard = super::super::lounge_auth::TestAuthGuard::new();
        let prev = std::env::var_os(super::super::lounge_auth::LOUNGE_AUTH_REQUIRED_ENV);
        unsafe {
            std::env::set_var(super::super::lounge_auth::LOUNGE_AUTH_REQUIRED_ENV, "false");
        }
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut service = NatsService::with_config(NatsConfig {
            host: "127.0.0.1".into(),
            port,
            http_port: 0,
            binary: "__missing_nats__".into(),
            args: vec!["-p".into(), port.to_string()],
            credentials: None,
        });

        let health = service.ensure().await;
        assert!(health.running);
        assert!(!health.started_by_us);
        assert!(health.error.is_none());
        unsafe {
            match prev {
                Some(v) => {
                    std::env::set_var(super::super::lounge_auth::LOUNGE_AUTH_REQUIRED_ENV, v)
                }
                None => std::env::remove_var(super::super::lounge_auth::LOUNGE_AUTH_REQUIRED_ENV),
            }
        }
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
        assert!(health.is_not_installed());
        assert!(health.error.is_none());
        assert!(health.detail.as_deref().unwrap_or("").contains("optional"));
    }

    #[test]
    fn resolve_nats_binary_accepts_absolute_existing_path() {
        let dir = std::env::temp_dir().join(format!("lounge-nats-resolve-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("nats-server");
        std::fs::write(&path, b"stub").expect("write stub");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
        let resolved = resolve_nats_binary(&path.display().to_string());
        assert_eq!(resolved.as_deref(), Some(path.as_path()));
        assert!(resolve_nats_binary("__missing_nats_binary_xyz__").is_none());
        let _ = std::fs::remove_dir_all(&dir);
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
        // Fail loud when nats-server is missing — no silent green.
        // CI installs the binary (ci.yml / qa-cross-platform.yml); local: see docs/qa/rust-opt-in-tests.md.
        let (url, mut child) = spawn_ephemeral_nats().unwrap_or_else(|err| {
            panic!(
                "nats-server required for event_pump_receives_non_bus_message_on_wildcard: {err}"
            );
        });
        let result = (|| -> Result<()> {
            let (nc, sub) = connect_and_subscribe(&url)?;
            let payload = br#"{"id":"pump-infer","type":"task","source_agent":"test"}"#;
            nc.publish("lounge.task.requested", &payload[..])
                .context("publish task")?;
            nc.flush().context("flush")?;
            let msg = sub
                .next_timeout(Duration::from_secs(5))
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
    fn spawn_args_use_conf_not_user_pass_when_credentials_set() {
        let config = NatsConfig {
            credentials: Some(NatsCredentials {
                user: "lounge_u".into(),
                password: "secret-must-not-appear".into(),
            }),
            ..NatsConfig::default()
        };
        let args = nats_server_args(&config, false);
        assert!(
            !args.iter().any(|a| a == "--user" || a == "--pass"),
            "argv must not contain --user/--pass"
        );
        assert!(
            !args
                .iter()
                .any(|a| a == "lounge_u" || a == "secret-must-not-appear"),
            "argv must not contain credential values"
        );
        assert!(
            args.windows(2).any(|pair| pair[0] == "-c"),
            "argv must include -c <conf>: {args:?}"
        );
        let conf_idx = args.iter().position(|a| a == "-c").unwrap();
        assert!(
            args[conf_idx + 1].ends_with("nats-server.conf"),
            "conf path: {}",
            args[conf_idx + 1]
        );
    }

    #[test]
    fn nats_auth_rejects_unauthenticated_when_required() {
        let creds = NatsCredentials {
            user: format!("u_{}", uuid::Uuid::new_v4().simple()),
            password: format!("p_{}", uuid::Uuid::new_v4().simple()),
        };
        let (url, mut child) = spawn_ephemeral_nats_with_auth(&creds).unwrap_or_else(|err| {
            panic!(
                "nats-server required for nats_auth_rejects_unauthenticated_when_required: {err}"
            );
        });
        // Child cmdline must not contain password (match count only).
        let cmdline = process_cmdline(child.id());
        let pass_hits = cmdline
            .iter()
            .filter(|a| a.contains(&creds.password))
            .count();
        let user_flag = cmdline.iter().any(|a| a == "--user" || a == "--pass");
        assert_eq!(pass_hits, 0, "password must not appear in nats-server argv");
        assert!(!user_flag, "--user/--pass must not appear in argv");
        assert!(
            cmdline.iter().any(|a| a == "-c"),
            "expected -c in argv: {cmdline:?}"
        );

        let unauth =
            super::super::lounge_auth::connect_nats_timeout(&url, None, Duration::from_secs(8));
        assert!(
            unauth.is_err(),
            "unauthenticated connect must be rejected when nats-server requires user/pass"
        );
        let auth = super::super::lounge_auth::connect_nats_timeout(
            &url,
            Some((&creds.user, &creds.password)),
            Duration::from_secs(8),
        );
        assert!(auth.is_ok(), "authenticated connect must succeed: {auth:?}");
        let _ = child.kill();
        let _ = child.wait();
    }

    #[tokio::test]
    async fn nats_service_starts_with_auth_and_activates_verified_bus() {
        let _guard = super::super::lounge_auth::TestAuthGuard::new();
        let temp =
            std::env::temp_dir().join(format!("lounge-nats-auth-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let creds_path = temp.join("session.creds.json");
        let prev_auth = std::env::var_os(super::super::lounge_auth::LOUNGE_AUTH_REQUIRED_ENV);
        let prev_creds_file =
            std::env::var_os(super::super::lounge_auth::LOUNGE_NATS_CREDS_FILE_ENV);
        let prev_user = std::env::var_os(super::super::lounge_auth::LOUNGE_NATS_USER_ENV);
        let prev_pass = std::env::var_os(super::super::lounge_auth::LOUNGE_NATS_PASS_ENV);
        unsafe {
            std::env::set_var(super::super::lounge_auth::LOUNGE_AUTH_REQUIRED_ENV, "true");
            std::env::set_var(
                super::super::lounge_auth::LOUNGE_NATS_CREDS_FILE_ENV,
                &creds_path,
            );
            std::env::remove_var(super::super::lounge_auth::LOUNGE_NATS_USER_ENV);
            std::env::remove_var(super::super::lounge_auth::LOUNGE_NATS_PASS_ENV);
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);

        let mut service = NatsService::with_config(NatsConfig {
            host: "127.0.0.1".into(),
            port,
            http_port: 0,
            binary: "nats-server".into(),
            args: Vec::new(),
            credentials: None,
        });
        let health = service.ensure().await;
        assert!(
            health.running,
            "expected nats auth start: {:?}",
            health.error
        );
        assert!(service.credentials().is_some());
        assert!(crate::services::lounge_auth::nats_auth_active());
        assert!(crate::services::lounge_auth::nats_ingress_source_verified());

        let url = service.endpoint();
        assert!(
            super::super::lounge_auth::connect_nats_timeout(&url, None, Duration::from_secs(5))
                .is_err(),
            "plain connect must fail against auth-required server"
        );
        let creds = service
            .credentials()
            .expect("service must have credentials");
        assert!(
            super::super::lounge_auth::connect_nats_timeout(
                &url,
                Some((&creds.user, &creds.password)),
                Duration::from_secs(5),
            )
            .is_ok(),
            "authenticated connect must succeed with service credentials"
        );
        assert!(nats_connect(&url).is_ok());

        // Spawned child cmdline: no password, has -c (match counts only).
        if let Some(child) = service.child.as_ref() {
            let pid = child.id().expect("child pid");
            let cmd = process_cmdline(pid);
            let pass_hits = cmd.iter().filter(|a| a.contains(&creds.password)).count();
            assert_eq!(pass_hits, 0, "password must not appear in child argv");
            assert!(cmd.iter().any(|a| a == "-c"), "expected -c: {cmd:?}");
            assert!(!cmd.iter().any(|a| a == "--pass" || a == "--user"));
        }
        let conf = temp.join("nats-server.conf");
        assert!(conf.is_file(), "conf must be rewritten on launch");
        let conf_body = std::fs::read_to_string(&conf).unwrap();
        assert!(!conf_body.contains(&creds.password));
        assert!(conf_body.contains("$2a$"));

        service.kill_child().await;
        assert!(
            !conf.is_file(),
            "conf must be deleted on clean shutdown when started_by_us"
        );
        crate::services::lounge_auth::deactivate_nats_auth();
        unsafe {
            match prev_auth {
                Some(v) => {
                    std::env::set_var(super::super::lounge_auth::LOUNGE_AUTH_REQUIRED_ENV, v)
                }
                None => std::env::remove_var(super::super::lounge_auth::LOUNGE_AUTH_REQUIRED_ENV),
            }
            match prev_creds_file {
                Some(v) => {
                    std::env::set_var(super::super::lounge_auth::LOUNGE_NATS_CREDS_FILE_ENV, v)
                }
                None => std::env::remove_var(super::super::lounge_auth::LOUNGE_NATS_CREDS_FILE_ENV),
            }
            match prev_user {
                Some(v) => std::env::set_var(super::super::lounge_auth::LOUNGE_NATS_USER_ENV, v),
                None => std::env::remove_var(super::super::lounge_auth::LOUNGE_NATS_USER_ENV),
            }
            match prev_pass {
                Some(v) => std::env::set_var(super::super::lounge_auth::LOUNGE_NATS_PASS_ENV, v),
                None => std::env::remove_var(super::super::lounge_auth::LOUNGE_NATS_PASS_ENV),
            }
        }
        let _ = std::fs::remove_dir_all(temp);
    }

    fn spawn_ephemeral_nats() -> Result<(String, std::process::Child)> {
        spawn_ephemeral_nats_with_auth_opt(None)
    }

    fn spawn_ephemeral_nats_with_auth(
        creds: &NatsCredentials,
    ) -> Result<(String, std::process::Child)> {
        spawn_ephemeral_nats_with_auth_opt(Some(creds))
    }

    fn spawn_ephemeral_nats_with_auth_opt(
        creds: Option<&NatsCredentials>,
    ) -> Result<(String, std::process::Child)> {
        let binary = find_executable("nats-server").ok_or_else(|| {
            anyhow::anyhow!("nats-server not on PATH (install locally or rely on CI install step)")
        })?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .context("bind ephemeral port for nats-server")?;
        let port = listener
            .local_addr()
            .context("ephemeral listener local_addr")?
            .port();
        drop(listener);
        let mut args = vec![
            "-a".to_string(),
            "127.0.0.1".to_string(),
            "-p".to_string(),
            port.to_string(),
        ];
        // Keep conf dir alive for the child process lifetime.
        let _conf_dir_keepalive;
        if let Some(c) = creds {
            let dir = std::env::temp_dir().join(format!(
                "lounge-nats-ephemeral-conf-{}",
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&dir).context("create ephemeral conf dir")?;
            let path = dir.join("nats-server.conf");
            let body = super::super::lounge_auth::render_nats_auth_config(c)?;
            super::super::lounge_auth::write_secret_file(&path, body)?;
            args.push("-c".into());
            args.push(path.display().to_string());
            _conf_dir_keepalive = Some(dir);
        } else {
            _conf_dir_keepalive = None;
        }
        let mut command = GuardedCommand::new(binary)
            .args(&args)
            .internal_daemon()
            .into_std_command()
            .context("nats-server GuardedCommand")?;
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn nats-server")?;
        let url = format!("nats://127.0.0.1:{port}");
        // Bounded dials: raw nats::connect can hang forever on a non-NATS listener.
        // ~10s budget (200 × 50ms) absorbs slow Windows/macOS process start without flakes.
        for _ in 0..200 {
            let ok = match creds {
                Some(c) => super::super::lounge_auth::connect_nats_timeout(
                    &url,
                    Some((&c.user, &c.password)),
                    Duration::from_millis(500),
                )
                .is_ok(),
                None => super::super::lounge_auth::connect_nats_timeout(
                    &url,
                    None,
                    Duration::from_millis(500),
                )
                .is_ok(),
            };
            if ok {
                // Brief settle so subscribe/publish races on a freshly-bound port are rare.
                std::thread::sleep(Duration::from_millis(50));
                // Leak conf dir intentionally until process exits (OS cleans temp).
                std::mem::forget(_conf_dir_keepalive);
                return Ok((url, child));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
        Err(anyhow::anyhow!(
            "nats-server spawned but did not accept connections at {url} within ~10s"
        ))
    }

    /// Spawn legacy nats-server with `--user/--pass` on argv (migration fixture).
    fn spawn_legacy_nats_with_argv_auth(
        creds: &NatsCredentials,
    ) -> Result<(u16, std::process::Child)> {
        let binary = find_executable("nats-server").ok_or_else(|| {
            anyhow::anyhow!("nats-server not on PATH (install locally or rely on CI install step)")
        })?;
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .context("bind ephemeral port for legacy nats")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        let args = vec![
            "-a".to_string(),
            "127.0.0.1".to_string(),
            "-p".to_string(),
            port.to_string(),
            "--user".to_string(),
            creds.user.clone(),
            "--pass".to_string(),
            creds.password.clone(),
        ];
        let mut command = GuardedCommand::new(binary)
            .args(&args)
            .internal_daemon()
            .into_std_command()
            .context("legacy nats GuardedCommand")?;
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("spawn legacy nats-server")?;
        let url = format!("nats://127.0.0.1:{port}");
        for _ in 0..200 {
            if super::super::lounge_auth::connect_nats_timeout(
                &url,
                Some((&creds.user, &creds.password)),
                Duration::from_millis(500),
            )
            .is_ok()
            {
                std::thread::sleep(Duration::from_millis(50));
                return Ok((port, child));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut child = child;
        let _ = child.kill();
        let _ = child.wait();
        Err(anyhow::anyhow!(
            "legacy nats-server did not accept auth at {url}"
        ))
    }

    #[tokio::test]
    async fn ensure_migrates_legacy_pass_argv_server() {
        let _guard = super::super::lounge_auth::TestAuthGuard::new();
        let temp =
            std::env::temp_dir().join(format!("lounge-nats-migrate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let creds_path = temp.join("session.creds.json");
        let old_creds = NatsCredentials {
            user: format!("old_u_{}", uuid::Uuid::new_v4().simple()),
            password: format!("old_p_{}", uuid::Uuid::new_v4().simple()),
        };
        super::super::lounge_auth::write_credentials_file(
            &creds_path,
            &old_creds,
            "nats://127.0.0.1:0",
        )
        .unwrap();
        super::super::lounge_auth::activate_nats_auth(old_creds.clone());

        let (port, mut legacy) =
            spawn_legacy_nats_with_argv_auth(&old_creds).unwrap_or_else(|err| {
                panic!("nats-server required for ensure_migrates_legacy_pass_argv_server: {err}");
            });
        let legacy_pid = legacy.id();
        assert!(
            process_cmd_has_pass(&process_cmdline(legacy_pid)),
            "fixture must expose --pass on cmdline"
        );

        let prev_auth = std::env::var_os(super::super::lounge_auth::LOUNGE_AUTH_REQUIRED_ENV);
        let prev_creds_file =
            std::env::var_os(super::super::lounge_auth::LOUNGE_NATS_CREDS_FILE_ENV);
        unsafe {
            std::env::set_var(super::super::lounge_auth::LOUNGE_AUTH_REQUIRED_ENV, "true");
            std::env::set_var(
                super::super::lounge_auth::LOUNGE_NATS_CREDS_FILE_ENV,
                &creds_path,
            );
        }

        let mut service = NatsService::with_config(NatsConfig {
            host: "127.0.0.1".into(),
            port,
            http_port: 0,
            binary: "nats-server".into(),
            args: Vec::new(),
            credentials: None,
        });
        let health = service.ensure().await;
        assert!(
            health.running,
            "migration ensure must start clean server: {:?}",
            health.error
        );
        assert!(service.started_by_us);

        let new_pid = service.child.as_ref().and_then(|c| c.id());
        assert!(new_pid.is_some());
        assert_ne!(new_pid, Some(legacy_pid), "must not keep legacy PID");

        let new_cmd = process_cmdline(new_pid.unwrap());
        assert!(
            !process_cmd_has_pass(&new_cmd),
            "new argv must not contain --pass"
        );
        assert!(
            new_cmd.iter().any(|a| a == "-c"),
            "new argv must use -c conf"
        );

        let url = service.endpoint();
        assert!(
            super::super::lounge_auth::connect_nats_timeout(
                &url,
                Some((&old_creds.user, &old_creds.password)),
                Duration::from_secs(5),
            )
            .is_err(),
            "old leaked credentials must be rejected after rotation"
        );
        let new_creds = service.credentials().expect("rotated creds");
        assert_ne!(new_creds.password, old_creds.password);
        assert!(
            super::super::lounge_auth::connect_nats_timeout(
                &url,
                Some((&new_creds.user, &new_creds.password)),
                Duration::from_secs(5),
            )
            .is_ok(),
            "new credentials must work"
        );

        let pass_matches = count_cmdline_matches(&new_creds.password);
        assert_eq!(
            pass_matches, 0,
            "new password must not appear in any process cmdline (match count)"
        );

        service.kill_child().await;
        let _ = legacy.kill();
        let _ = legacy.wait();
        crate::services::lounge_auth::deactivate_nats_auth();
        unsafe {
            match prev_auth {
                Some(v) => {
                    std::env::set_var(super::super::lounge_auth::LOUNGE_AUTH_REQUIRED_ENV, v)
                }
                None => std::env::remove_var(super::super::lounge_auth::LOUNGE_AUTH_REQUIRED_ENV),
            }
            match prev_creds_file {
                Some(v) => {
                    std::env::set_var(super::super::lounge_auth::LOUNGE_NATS_CREDS_FILE_ENV, v)
                }
                None => std::env::remove_var(super::super::lounge_auth::LOUNGE_NATS_CREDS_FILE_ENV),
            }
        }
        let _ = std::fs::remove_dir_all(temp);
    }

    #[test]
    fn unused_port_has_no_nats_listeners() {
        assert!(listen_pids(1).is_empty());
        assert_eq!(kill_nats_on_port(1), 0);
    }

    /// macOS/Linux: `ps -axww -o args` must show 0 matches for the NATS password
    /// (counts only — values never printed).
    #[cfg(unix)]
    #[test]
    fn nats_password_absent_from_ps_axww_args() {
        use crate::kernel::ActionSource;
        let creds = NatsCredentials {
            user: format!("ps_u_{}", uuid::Uuid::new_v4().simple()),
            password: format!("ps_p_{}", uuid::Uuid::new_v4().simple()),
        };
        let (_url, mut child) = spawn_ephemeral_nats_with_auth(&creds).unwrap_or_else(|err| {
            panic!("nats-server required for nats_password_absent_from_ps_axww_args: {err}");
        });
        let output = GuardedCommand::new("ps")
            .args(["-axww", "-o", "args"])
            .source(ActionSource::User)
            .output()
            .expect("ps -axww -o args");
        assert!(output.status.success(), "ps failed");
        let dump = String::from_utf8_lossy(&output.stdout);
        let pass_matches = dump.matches(&creds.password).count();
        assert_eq!(
            pass_matches, 0,
            "ps -axww -o args password match count must be 0"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    /// Windows: `Get-CimInstance Win32_Process | Select-Object CommandLine` must show
    /// 0 matches for the NATS password (counts only — values never printed).
    #[cfg(windows)]
    #[test]
    fn nats_password_absent_from_win32_process_commandline() {
        use crate::kernel::ActionSource;
        let creds = NatsCredentials {
            user: format!("cim_u_{}", uuid::Uuid::new_v4().simple()),
            password: format!("cim_p_{}", uuid::Uuid::new_v4().simple()),
        };
        let (_url, mut child) = spawn_ephemeral_nats_with_auth(&creds).unwrap_or_else(|err| {
            panic!(
                "nats-server required for nats_password_absent_from_win32_process_commandline: {err}"
            );
        });
        let script = r#"Get-CimInstance Win32_Process | Select-Object -ExpandProperty CommandLine"#;
        let output = GuardedCommand::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .source(ActionSource::User)
            .output()
            .expect("Get-CimInstance Win32_Process");
        assert!(
            output.status.success(),
            "powershell Get-CimInstance failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let dump = String::from_utf8_lossy(&output.stdout);
        let pass_matches = dump.matches(&creds.password).count();
        assert_eq!(
            pass_matches, 0,
            "Get-CimInstance CommandLine password match count must be 0"
        );
        let _ = child.kill();
        let _ = child.wait();
    }
}
