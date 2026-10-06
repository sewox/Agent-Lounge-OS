//! codebase-memory-mcp 3D Graph UI — probe, spawn, singleton window.
//!
//! Port bandı 18749–18759; Antigravity cbm varsayılanı 9749 ile çakışmaz.
//! Yabancı `/api/ui-config` asla adopt edilmez; HTTP `/rpc` yalnız sahipli portta.

use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::Mutex as AsyncMutex;

use super::memory_bridge::{
    probe_ui_config, MemoryBridge, DEFAULT_GRAPH_UI_PORT, GRAPH_UI_PORT_BAND_END,
    GRAPH_UI_PORT_BAND_START, LEGACY_GRAPH_UI_PORT, UI_PROBE_TIMEOUT,
};
use super::probe::{port_owned_by_lounge, tcp_bind_available, tcp_ready, wait_until};
use crate::kernel::GuardedCommand;

pub const GRAPH_WINDOW_LABEL: &str = "graph-window";
const ENABLE_READY_DEADLINE: Duration = Duration::from_secs(10);
const SETTINGS_KEY_PORT: &str = "graph_ui_port";
const SETTINGS_KEY_MODE: &str = "graph_ui_port_mode";
/// Single-flight join penceresi — lock serbest kalınca bekleyenler taze sonucu alır.
const STATUS_JOIN_WINDOW: Duration = Duration::from_millis(500);

pub const GRAPH_UI_MIGRATION_LOG: &str =
    "graph UI port migration: saved 9749 → auto mode (band 18749–18759)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GraphUiPortMode {
    #[default]
    Auto,
    User,
}

impl GraphUiPortMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::User => "user",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().trim_matches('"').to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphUiPortPreference {
    pub mode: GraphUiPortMode,
    pub port: u16,
    /// Migrasyon mesajı (yalnız legacy 9749 → auto).
    pub migration_log: Option<&'static str>,
    pub persist_mode: bool,
    pub persist_port: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortSelectError {
    UserConflict { port: u16 },
    BandExhausted,
}

impl std::fmt::Display for PortSelectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserConflict { port } => write!(
                f,
                "Port {port} meşgul — user modunda otomatik değiştirilmez. Başka bir codebase-memory-mcp (ör. Antigravity) bu portu kullanıyor olabilir; Settings'ten portu değiştirin veya süreci kapatın."
            ),
            Self::BandExhausted => write!(
                f,
                "Graph UI port bandı {GRAPH_UI_PORT_BAND_START}–{GRAPH_UI_PORT_BAND_END} tamamen dolu — bir portu boşaltın veya Settings'te user modunda başka bir port seçin."
            ),
        }
    }
}

impl std::error::Error for PortSelectError {}

/// Lounge'un spawn ettiği cbm UI çocuğu + status single-flight + window port.
pub struct GraphUiState {
    child: Mutex<Option<Child>>,
    /// graph-window'un navigation guard'ında kilitli port.
    window_port: Mutex<Option<u16>>,
    port_mode: Mutex<GraphUiPortMode>,
    status_flight: AsyncMutex<StatusFlight>,
}

struct StatusFlight {
    last: Option<GraphUiStatus>,
    finished_at: Option<Instant>,
}

impl Default for GraphUiState {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphUiState {
    pub fn new() -> Self {
        Self {
            child: Mutex::new(None),
            window_port: Mutex::new(None),
            port_mode: Mutex::new(GraphUiPortMode::Auto),
            status_flight: AsyncMutex::new(StatusFlight {
                last: None,
                finished_at: None,
            }),
        }
    }

    pub fn port_mode(&self) -> GraphUiPortMode {
        match self.port_mode.lock() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        }
    }

    pub fn set_port_mode(&self, mode: GraphUiPortMode) {
        let mut guard = match self.port_mode.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = mode;
    }

    /// Yalnızca Lounge'un spawn ettiği çocuğu öldürür; önceden var olan cbm süreçlerine dokunmaz.
    pub fn kill_spawned_child(&self) {
        let mut guard = match self.child.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(mut child) = guard.take() {
            let pid = child.id();
            if let Err(err) = child.kill() {
                log::warn!("graph-ui child kill başarısız (pid={pid}): {err}");
            }
            let _ = child.wait();
            log::info!("graph-ui child sonlandırıldı (pid={pid})");
        }
    }

    fn store_child(&self, child: Child) {
        let mut guard = match self.child.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(mut prev) = guard.replace(child) {
            let _ = prev.kill();
            let _ = prev.wait();
        }
    }

    pub fn spawned_child_pid(&self) -> Option<u32> {
        let mut guard = match self.child.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        let child = guard.as_mut()?;
        match child.try_wait() {
            Ok(None) => Some(child.id()),
            Ok(Some(_)) => {
                let _ = guard.take();
                None
            }
            Err(_) => Some(child.id()),
        }
    }

    pub fn window_port(&self) -> Option<u16> {
        match self.window_port.lock() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        }
    }

    pub fn set_window_port(&self, port: Option<u16>) {
        let mut guard = match self.window_port.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = port;
    }

    /// Single-flight: mutex compute süresince tutulur; bekleyenler taze `last` sonucuna katılır.
    pub async fn status_single_flight<F, Fut>(&self, compute: F) -> GraphUiStatus
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = GraphUiStatus>,
    {
        let mut flight = self.status_flight.lock().await;
        if let (Some(status), Some(at)) = (flight.last.as_ref(), flight.finished_at) {
            if at.elapsed() < STATUS_JOIN_WINDOW {
                return status.clone();
            }
        }
        let status = compute().await;
        flight.last = Some(status.clone());
        flight.finished_at = Some(Instant::now());
        status
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct GraphUiStatus {
    pub binary_found: bool,
    pub ui_available: bool,
    pub project_indexed: bool,
    pub cbm_project_name: Option<String>,
    pub port: u16,
    pub port_conflict: bool,
    pub conflict_message: Option<String>,
    pub port_mode: GraphUiPortMode,
    pub owned_by_lounge: bool,
}

/// Saf tercih çözümleyici — store I/O yok (test edilebilir).
pub fn resolve_port_preference(
    mode_raw: Option<&str>,
    port_raw: Option<&str>,
) -> GraphUiPortPreference {
    let parsed_port = port_raw.and_then(parse_port_setting);
    let parsed_mode = mode_raw.and_then(GraphUiPortMode::parse);

    match (parsed_mode, parsed_port) {
        (None, Some(LEGACY_GRAPH_UI_PORT)) => GraphUiPortPreference {
            mode: GraphUiPortMode::Auto,
            port: DEFAULT_GRAPH_UI_PORT,
            migration_log: Some(GRAPH_UI_MIGRATION_LOG),
            persist_mode: true,
            persist_port: true,
        },
        (Some(mode), Some(port)) => GraphUiPortPreference {
            mode,
            port,
            migration_log: None,
            persist_mode: false,
            persist_port: false,
        },
        (Some(mode), None) => GraphUiPortPreference {
            mode,
            port: DEFAULT_GRAPH_UI_PORT,
            migration_log: None,
            persist_mode: false,
            persist_port: true,
        },
        // Legacy açık seçim (9749 değil) → user modunda koru.
        (None, Some(port)) => GraphUiPortPreference {
            mode: GraphUiPortMode::User,
            port,
            migration_log: None,
            persist_mode: true,
            persist_port: false,
        },
        (None, None) => GraphUiPortPreference {
            mode: GraphUiPortMode::Auto,
            port: DEFAULT_GRAPH_UI_PORT,
            migration_log: None,
            persist_mode: false,
            persist_port: false,
        },
    }
}

/// Auto: banddaki ilk boş port (preferred öncelikli). User: preferred korunur, çakışmada Err.
pub fn select_graph_ui_port(
    mode: GraphUiPortMode,
    preferred: u16,
    is_free: impl Fn(u16) -> bool,
) -> Result<u16, PortSelectError> {
    match mode {
        GraphUiPortMode::User => {
            if preferred == 0 {
                return Err(PortSelectError::UserConflict { port: preferred });
            }
            if is_free(preferred) {
                Ok(preferred)
            } else {
                Err(PortSelectError::UserConflict { port: preferred })
            }
        }
        GraphUiPortMode::Auto => {
            if (GRAPH_UI_PORT_BAND_START..=GRAPH_UI_PORT_BAND_END).contains(&preferred)
                && is_free(preferred)
            {
                return Ok(preferred);
            }
            for port in GRAPH_UI_PORT_BAND_START..=GRAPH_UI_PORT_BAND_END {
                if is_free(port) {
                    return Ok(port);
                }
            }
            Err(PortSelectError::BandExhausted)
        }
    }
}

pub async fn load_port_preference_from_store(
    store: &crate::db::ExperienceStore,
) -> GraphUiPortPreference {
    let mode_raw = store
        .get_setting(SETTINGS_KEY_MODE.into())
        .await
        .ok()
        .flatten();
    let port_raw = store
        .get_setting(SETTINGS_KEY_PORT.into())
        .await
        .ok()
        .flatten();
    let pref = resolve_port_preference(mode_raw.as_deref(), port_raw.as_deref());
    if let Some(msg) = pref.migration_log {
        log::info!("{msg}");
    }
    if pref.persist_mode {
        let _ = store
            .set_setting(SETTINGS_KEY_MODE.into(), pref.mode.as_str().into())
            .await;
    }
    if pref.persist_port {
        let _ = store
            .set_setting(SETTINGS_KEY_PORT.into(), pref.port.to_string())
            .await;
    }
    pref
}

pub async fn load_port_from_store(store: &crate::db::ExperienceStore) -> u16 {
    load_port_preference_from_store(store).await.port
}

pub async fn persist_port_preference(
    store: &crate::db::ExperienceStore,
    bridge: &MemoryBridge,
    app: &AppHandle,
    state: &GraphUiState,
    mode: GraphUiPortMode,
    port: u16,
) -> Result<()> {
    if port == 0 {
        bail!("port 0 geçersiz");
    }
    bridge.set_http_port(port);
    state.set_port_mode(mode);
    store
        .set_setting(SETTINGS_KEY_MODE.into(), mode.as_str().into())
        .await
        .context("graph_ui_port_mode kaydedilemedi")?;
    store
        .set_setting(SETTINGS_KEY_PORT.into(), port.to_string())
        .await
        .context("graph_ui_port kaydedilemedi")?;
    if state.window_port() != Some(port) {
        if let Some(window) = app.get_webview_window(GRAPH_WINDOW_LABEL) {
            let _ = window.destroy();
        }
        state.set_window_port(None);
    }
    Ok(())
}

pub async fn persist_port(
    store: &crate::db::ExperienceStore,
    bridge: &MemoryBridge,
    app: &AppHandle,
    state: &GraphUiState,
    port: u16,
) -> Result<()> {
    // Settings'ten açık port seçimi → user modu.
    persist_port_preference(store, bridge, app, state, GraphUiPortMode::User, port).await
}

pub fn parse_port_setting(raw: &str) -> Option<u16> {
    let trimmed = raw.trim().trim_matches('"');
    trimmed.parse::<u16>().ok().filter(|p| *p > 0)
}

pub async fn graph_ui_status(
    state: &GraphUiState,
    bridge: &MemoryBridge,
    project_root: Option<&str>,
) -> GraphUiStatus {
    let bridge = bridge.clone();
    let root = project_root.map(str::to_string);
    let mode = state.port_mode();
    let child_pid = state.spawned_child_pid();
    state
        .status_single_flight(|| async move {
            graph_ui_status_inner(&bridge, root.as_deref(), mode, child_pid).await
        })
        .await
}

async fn graph_ui_status_inner(
    bridge: &MemoryBridge,
    project_root: Option<&str>,
    port_mode: GraphUiPortMode,
    child_pid: Option<u32>,
) -> GraphUiStatus {
    let port = bridge.http_port();
    let binary_found = bridge.binary_path().is_file();
    let owned_by_lounge = port_owned_by_lounge(port, child_pid);
    if !binary_found {
        return GraphUiStatus {
            binary_found: false,
            ui_available: false,
            project_indexed: false,
            cbm_project_name: None,
            port,
            port_conflict: false,
            conflict_message: None,
            port_mode,
            owned_by_lounge,
        };
    }

    let (ui_available, port_conflict, conflict_message) =
        classify_port(port, child_pid, port_mode).await;

    let cbm_project_name = if ui_available {
        if let Some(root) = project_root.filter(|s| !s.trim().is_empty()) {
            match resolve_cbm_project_name(bridge, Path::new(root)).await {
                Ok(name) => name,
                Err(err) => {
                    log::debug!("cbm project eşlemesi: {err}");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };
    let project_indexed = cbm_project_name.is_some();

    GraphUiStatus {
        binary_found: true,
        ui_available,
        project_indexed,
        cbm_project_name,
        port,
        port_conflict,
        conflict_message,
        port_mode,
        owned_by_lounge,
    }
}

/// UI available yalnız sahipli + ui-config. Yabancı cbm → conflict, asla available.
pub async fn classify_port(
    port: u16,
    child_pid: Option<u32>,
    mode: GraphUiPortMode,
) -> (bool, bool, Option<String>) {
    let owned = port_owned_by_lounge(port, child_pid);
    if owned {
        if probe_ui_config(port).await {
            return (true, false, None);
        }
        return (false, false, None);
    }

    let foreign_ui = probe_ui_config(port).await;
    let tcp_open = tcp_ready("127.0.0.1", port, UI_PROBE_TIMEOUT).await;
    if foreign_ui || tcp_open {
        let msg = if foreign_ui {
            format!(
                "Port {port} üzerinde başka bir codebase-memory-mcp (ör. Antigravity) çalışıyor — Lounge bunu sahiplenmez.{}",
                match mode {
                    GraphUiPortMode::Auto => " Auto modda bir sonraki boş porta geçilecek.",
                    GraphUiPortMode::User => {
                        " User modunda port otomatik değiştirilmez; Settings'ten değiştirin."
                    }
                }
            )
        } else {
            format!(
                "Port {port} başka bir süreç tarafından kullanılıyor — Settings'te Graph UI Port'u değiştirin veya o süreci kontrol edin."
            )
        };
        return (false, true, Some(msg));
    }
    (false, false, None)
}

pub async fn resolve_cbm_project_name(
    bridge: &MemoryBridge,
    lounge_root: &Path,
) -> Result<Option<String>> {
    let want = canonicalize_lossy(lounge_root);
    let projects = bridge.list_projects().await?;
    for project in projects {
        let Some(root) = project.root_path.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        let got = canonicalize_lossy(Path::new(root));
        if paths_equal(&want, &got) {
            return Ok(Some(project.name));
        }
    }
    Ok(None)
}

fn canonicalize_lossy(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    let a = left.to_string_lossy().trim_end_matches('/').to_lowercase();
    let b = right.to_string_lossy().trim_end_matches('/').to_lowercase();
    a == b
}

pub fn graph_project_url(port: u16, cbm_project: &str) -> Result<Url> {
    let mut url = Url::parse(&format!("http://127.0.0.1:{port}/"))
        .with_context(|| format!("graph url parse :{port}"))?;
    url.query_pairs_mut()
        .append_pair("tab", "graph")
        .append_pair("project", cbm_project);
    Ok(url)
}

pub fn navigation_allowed(url: &Url, port: u16) -> bool {
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port_or_known_default() == Some(port)
}

pub fn open_or_focus_graph_window(
    app: &AppHandle,
    state: &GraphUiState,
    port: u16,
    cbm_project: &str,
) -> Result<()> {
    let url = graph_project_url(port, cbm_project)?;
    if let Some(window) = app.get_webview_window(GRAPH_WINDOW_LABEL) {
        if state.window_port() == Some(port) {
            window.navigate(url).context("graph-window navigate")?;
            window.set_focus().context("graph-window set_focus")?;
            return Ok(());
        }
        let _ = window.destroy();
        state.set_window_port(None);
    }

    let allowed_port = port;
    WebviewWindowBuilder::new(app, GRAPH_WINDOW_LABEL, WebviewUrl::External(url.clone()))
        .title("Codebase Memory · 3D Graph")
        .inner_size(1280.0, 800.0)
        .resizable(true)
        .on_navigation(move |nav| navigation_allowed(nav, allowed_port))
        .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Deny)
        .build()
        .context("graph-window create")?;
    state.set_window_port(Some(port));
    Ok(())
}

/// Main kapanınca graph-window + Lounge child temizliği.
pub fn on_main_window_closed(app: &AppHandle, state: &GraphUiState, bridge: Option<&MemoryBridge>) {
    if let Some(window) = app.get_webview_window(GRAPH_WINDOW_LABEL) {
        let _ = window.destroy();
    }
    state.set_window_port(None);
    state.kill_spawned_child();
    if let Some(bridge) = bridge {
        bridge.set_owned_ui_pid(None);
    }
}

fn sync_owned_pid(bridge: &MemoryBridge, state: &GraphUiState) {
    bridge.set_owned_ui_pid(state.spawned_child_pid());
}

async fn spawn_graph_ui_on_port(
    state: &GraphUiState,
    bridge: &MemoryBridge,
    port: u16,
) -> Result<()> {
    let binary = bridge.binary_path();
    if !binary.is_file() {
        bail!("codebase-memory-mcp bulunamadı");
    }

    let mut command = GuardedCommand::new(binary)
        .arg("--ui=true")
        .arg(format!("--port={port}"))
        .internal_daemon()
        .into_std_command()
        .with_context(|| format!("graph UI gate başarısız: {}", binary.display()))?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let child = command
        .spawn()
        .with_context(|| format!("graph UI spawn başarısız: {}", binary.display()))?;
    state.store_child(child);
    sync_owned_pid(bridge, state);

    let ready = wait_until(
        ENABLE_READY_DEADLINE,
        Duration::from_millis(250),
        || async { probe_ui_config(port).await },
    )
    .await;

    if !ready {
        state.kill_spawned_child();
        sync_owned_pid(bridge, state);
        bail!("Graph UI {ENABLE_READY_DEADLINE:?} içinde hazır olmadı — SemanticMap'e düşülüyor");
    }

    if !port_owned_by_lounge(port, state.spawned_child_pid()) {
        state.kill_spawned_child();
        sync_owned_pid(bridge, state);
        bail!("Graph UI port {port} sahiplik doğrulaması başarısız");
    }

    bridge.set_http_port(port);
    Ok(())
}

/// `--ui=true --port=N` spawn; stdin piped (kapanmaz). Hazır olmazsa çocuğu öldürür.
/// Auto: band seçimi + bind yarışında 1 retry. User: conflict'te asla port değiştirmez.
pub async fn enable_graph_ui(
    app: &AppHandle,
    state: &GraphUiState,
    bridge: &MemoryBridge,
    store: Option<&crate::db::ExperienceStore>,
    cbm_project: Option<&str>,
) -> Result<()> {
    let mode = state.port_mode();
    let preferred = bridge.http_port();
    let child_pid = state.spawned_child_pid();

    // Zaten bizim instance ayaktaysa pencereyi aç.
    if port_owned_by_lounge(preferred, child_pid) && probe_ui_config(preferred).await {
        if let Some(name) = cbm_project.filter(|s| !s.is_empty()) {
            open_or_focus_graph_window(app, state, preferred, name)?;
        }
        return Ok(());
    }

    let is_free = |port: u16| {
        if port_owned_by_lounge(port, state.spawned_child_pid()) {
            return true;
        }
        // Yabancı ui-config / dinleyici → boş değil.
        if !tcp_bind_available(port) {
            return false;
        }
        true
    };

    let selected = match select_graph_ui_port(mode, preferred, is_free) {
        Ok(p) => p,
        Err(err) => bail!("{err}"),
    };

    // Seçilen port zaten bizimse spawn etme.
    if port_owned_by_lounge(selected, state.spawned_child_pid()) && probe_ui_config(selected).await
    {
        bridge.set_http_port(selected);
        if let Some(name) = cbm_project.filter(|s| !s.is_empty()) {
            open_or_focus_graph_window(app, state, selected, name)?;
        }
        return Ok(());
    }

    match spawn_graph_ui_on_port(state, bridge, selected).await {
        Ok(()) => {}
        Err(first_err) => {
            // Bind yarışı: auto modda bir kez sonraki porta retry.
            if mode == GraphUiPortMode::Auto {
                let retry = select_graph_ui_port(
                    mode,
                    selected.saturating_add(1).max(GRAPH_UI_PORT_BAND_START),
                    |p| p != selected && is_free(p),
                );
                match retry {
                    Ok(next) if next != selected => {
                        log::warn!(
                            "graph UI bind race on {selected}, retrying on {next}: {first_err}"
                        );
                        spawn_graph_ui_on_port(state, bridge, next).await?;
                    }
                    Ok(_) | Err(PortSelectError::BandExhausted) => {
                        bail!("{first_err}; ayrıca port bandı tükendi veya retry yok")
                    }
                    Err(e) => bail!("{first_err}; retry: {e}"),
                }
            } else {
                return Err(first_err);
            }
        }
    }

    let live_port = bridge.http_port();
    if let Some(store) = store {
        let _ = store
            .set_setting(SETTINGS_KEY_PORT.into(), live_port.to_string())
            .await;
        let _ = store
            .set_setting(SETTINGS_KEY_MODE.into(), mode.as_str().into())
            .await;
    }

    if let Some(name) = cbm_project.filter(|s| !s.is_empty()) {
        open_or_focus_graph_window(app, state, live_port, name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::memory_bridge::{MemoryBridgeConfig, TransportMode};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn parses_port_setting_variants() {
        assert_eq!(parse_port_setting("9749"), Some(9749));
        assert_eq!(parse_port_setting("\"9749\""), Some(9749));
        assert_eq!(parse_port_setting("18749"), Some(18749));
        assert_eq!(parse_port_setting("0"), None);
        assert_eq!(parse_port_setting("nope"), None);
    }

    #[test]
    fn legacy_9749_migrates_to_auto_and_logs() {
        let pref = resolve_port_preference(None, Some("9749"));
        assert_eq!(pref.mode, GraphUiPortMode::Auto);
        assert_eq!(pref.port, DEFAULT_GRAPH_UI_PORT);
        assert_eq!(pref.migration_log, Some(GRAPH_UI_MIGRATION_LOG));
        assert!(pref.persist_mode);
        assert!(pref.persist_port);
        assert!(GRAPH_UI_MIGRATION_LOG.contains("9749"));
        assert!(GRAPH_UI_MIGRATION_LOG.contains("auto"));
    }

    #[tokio::test]
    async fn store_legacy_9749_migrates_persists_auto_mode() {
        let store = crate::db::ExperienceStore::memory().unwrap();
        store
            .set_setting("graph_ui_port".into(), "9749".into())
            .await
            .unwrap();
        let pref = load_port_preference_from_store(&store).await;
        assert_eq!(pref.mode, GraphUiPortMode::Auto);
        assert_eq!(pref.port, DEFAULT_GRAPH_UI_PORT);
        assert_eq!(pref.migration_log, Some(GRAPH_UI_MIGRATION_LOG));
        let mode = store
            .get_setting("graph_ui_port_mode".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mode, "auto");
        let port = store
            .get_setting("graph_ui_port".into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(port, DEFAULT_GRAPH_UI_PORT.to_string());
    }

    #[test]
    fn explicit_user_9749_is_respected_not_migrated() {
        let pref = resolve_port_preference(Some("user"), Some("9749"));
        assert_eq!(pref.mode, GraphUiPortMode::User);
        assert_eq!(pref.port, 9749);
        assert!(pref.migration_log.is_none());
    }

    #[test]
    fn legacy_non_default_port_becomes_user_mode() {
        let pref = resolve_port_preference(None, Some("9800"));
        assert_eq!(pref.mode, GraphUiPortMode::User);
        assert_eq!(pref.port, 9800);
    }

    #[test]
    fn auto_selects_preferred_when_free() {
        let port = select_graph_ui_port(GraphUiPortMode::Auto, 18751, |_| true).unwrap();
        assert_eq!(port, 18751);
    }

    #[test]
    fn auto_skips_busy_and_picks_next_in_band() {
        let busy: HashSet<u16> = [18749, 18750].into_iter().collect();
        let port =
            select_graph_ui_port(GraphUiPortMode::Auto, 18749, |p| !busy.contains(&p)).unwrap();
        assert_eq!(port, 18751);
    }

    #[test]
    fn auto_band_exhausted_returns_clear_error() {
        let err = select_graph_ui_port(GraphUiPortMode::Auto, 18749, |_| false).unwrap_err();
        assert_eq!(err, PortSelectError::BandExhausted);
        let msg = err.to_string();
        assert!(msg.contains("18749"));
        assert!(msg.contains("18759"));
    }

    #[test]
    fn user_mode_conflict_does_not_switch() {
        let err = select_graph_ui_port(GraphUiPortMode::User, 9749, |_| false).unwrap_err();
        match err {
            PortSelectError::UserConflict { port } => assert_eq!(port, 9749),
            other => panic!("expected UserConflict, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(msg.contains("9749"));
        assert!(msg.contains("user") || msg.contains("User") || msg.contains("otomatik"));
    }

    #[test]
    fn user_mode_keeps_free_port() {
        assert_eq!(
            select_graph_ui_port(GraphUiPortMode::User, 19001, |_| true).unwrap(),
            19001
        );
    }

    #[test]
    fn navigation_locked_to_loopback_port() {
        let ok = Url::parse("http://127.0.0.1:18749/?tab=graph&project=x").unwrap();
        assert!(navigation_allowed(&ok, 18749));
        let bad_host = Url::parse("http://example.com:18749/").unwrap();
        assert!(!navigation_allowed(&bad_host, 18749));
        let bad_port = Url::parse("http://127.0.0.1:80/").unwrap();
        assert!(!navigation_allowed(&bad_port, 18749));
        let https = Url::parse("https://127.0.0.1:18749/").unwrap();
        assert!(!navigation_allowed(&https, 18749));
    }

    #[test]
    fn graph_url_includes_tab_and_project() {
        let url = graph_project_url(18749, "Users-demo-Agent-Lounge-OS").unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert_eq!(url.port(), Some(18749));
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q.get("tab").map(String::as_str), Some("graph"));
        assert_eq!(
            q.get("project").map(String::as_str),
            Some("Users-demo-Agent-Lounge-OS")
        );
    }

    #[test]
    fn paths_equal_normalizes_trailing_slash() {
        assert!(paths_equal(Path::new("/tmp/foo"), Path::new("/tmp/foo/")));
    }

    #[tokio::test]
    async fn classify_rejects_foreign_and_auto_selects_next_band_port() {
        use axum::routing::get;
        use axum::{Json, Router};

        // Band start'ı işgal et (mümkünse); değilse rastgele foreign + selector birimi.
        let app = Router::new().route(
            "/api/ui-config",
            get(|| async { Json(serde_json::json!({"lang": "en"})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let foreign_port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(Duration::from_millis(40)).await;

        let (available, conflict, msg) =
            classify_port(foreign_port, None, GraphUiPortMode::Auto).await;
        assert!(!available);
        assert!(conflict);
        let msg = msg.expect("conflict message");
        assert!(
            msg.contains("Antigravity")
                || msg.contains("codebase-memory-mcp")
                || msg.contains("sahiplenmez"),
            "unexpected msg: {msg}"
        );

        // Auto seçici foreign portu atlar (bind dolu).
        let chosen = select_graph_ui_port(GraphUiPortMode::Auto, GRAPH_UI_PORT_BAND_START, |p| {
            p != foreign_port && tcp_bind_available(p)
        })
        .expect("band has a free port");
        assert_ne!(chosen, foreign_port);
        assert!((GRAPH_UI_PORT_BAND_START..=GRAPH_UI_PORT_BAND_END).contains(&chosen));
    }

    #[tokio::test]
    async fn status_single_flight_joins_via_arc() {
        let state = Arc::new(GraphUiState::new());
        let calls = Arc::new(AtomicU32::new(0));

        let run = |state: Arc<GraphUiState>, calls: Arc<AtomicU32>, port: u16| {
            tokio::spawn(async move {
                state
                    .status_single_flight(|| {
                        let calls = calls.clone();
                        async move {
                            calls.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            GraphUiStatus {
                                binary_found: true,
                                ui_available: false,
                                project_indexed: false,
                                cbm_project_name: None,
                                port,
                                port_conflict: false,
                                conflict_message: None,
                                port_mode: GraphUiPortMode::Auto,
                                owned_by_lounge: false,
                            }
                        }
                    })
                    .await
            })
        };

        let t1 = run(state.clone(), calls.clone(), 11);
        tokio::time::sleep(Duration::from_millis(15)).await;
        let t2 = run(state.clone(), calls.clone(), 22);
        let a = t1.await.unwrap();
        let b = t2.await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1, "compute should run once");
        assert_eq!(a.port, b.port);
        assert_eq!(a.port, 11);
    }

    #[tokio::test]
    async fn status_skips_list_projects_when_ui_down() {
        let bridge = MemoryBridge::with_config(
            "/tmp/missing-graph-status",
            MemoryBridgeConfig {
                http_port: 1,
                transport: TransportMode::ForceCli,
                ..MemoryBridgeConfig::default()
            },
        );
        let state = GraphUiState::new();
        let status = graph_ui_status(&state, &bridge, Some("/tmp/any")).await;
        assert!(!status.binary_found);
        assert!(status.cbm_project_name.is_none());
        assert!(!status.ui_available);
        assert_eq!(status.port_mode, GraphUiPortMode::Auto);
        assert!(!status.owned_by_lounge);
    }
}
