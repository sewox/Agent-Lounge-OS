//! codebase-memory-mcp 3D Graph UI — probe, spawn, singleton window.
//!
//! Port bandı 18749–18759; Antigravity cbm varsayılanı 9749 ile çakışmaz.
//! Yabancı `/api/ui-config` asla adopt edilmez; HTTP `/rpc` yalnız sahipli portta.

#[cfg(feature = "test-helpers")]
use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::Mutex as AsyncMutex;

use super::cbm_ui_config::{
    clear_snapshot_in_store, persist_snapshot_to_store, restore_cbm_ui_config,
    snapshot_cbm_ui_config_at, CbmUiConfigSnapshot, RestoreOutcome,
};
use super::memory_bridge::{
    probe_ui_config, MemoryBridge, DEFAULT_GRAPH_UI_PORT, GRAPH_UI_PORT_BAND_END,
    GRAPH_UI_PORT_BAND_START, LEGACY_GRAPH_UI_PORT, UI_PROBE_TIMEOUT,
};
use super::probe::{listen_pids, port_owned_by_lounge, tcp_ready, wait_until};
use super::window_geometry::{
    apply_placement_show, persist_window_geometry, placement_for_window, GeometrySession,
};
use crate::kernel::GuardedCommand;
use std::sync::Arc;

pub const GRAPH_WINDOW_LABEL: &str = "graph-window";
const ENABLE_READY_DEADLINE: Duration = Duration::from_secs(10);
const SETTINGS_KEY_PORT: &str = "graph_ui_port";
const SETTINGS_KEY_MODE: &str = "graph_ui_port_mode";
/// Single-flight join penceresi — lock serbest kalınca bekleyenler taze sonucu alır.
const STATUS_JOIN_WINDOW: Duration = Duration::from_millis(500);

pub const GRAPH_UI_MIGRATION_LOG: &str =
    "graph UI port migration: saved 9749 → auto mode (band 18749–18759)";

/// Test-only: ports kept reserved (LISTEN held) but treated as free by
/// [`classify_port_status`] Auto selection — avoids drop→steal TOCTOU (N11).
#[cfg(test)]
static CLASSIFY_HELD_FREE_PORTS: Mutex<Vec<u16>> = Mutex::new(Vec::new());

#[cfg(test)]
fn classify_held_free_contains(port: u16) -> bool {
    CLASSIFY_HELD_FREE_PORTS
        .lock()
        .map(|g| g.contains(&port))
        .unwrap_or(false)
}

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
    BandExhausted { start: u16, end: u16 },
}

impl std::fmt::Display for PortSelectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserConflict { port } => write!(
                f,
                "Port {port} meşgul — user modunda otomatik değiştirilmez. Başka bir codebase-memory-mcp (ör. Antigravity) bu portu kullanıyor olabilir; Settings'ten portu değiştirin veya süreci kapatın."
            ),
            Self::BandExhausted { start, end } => write!(
                f,
                "Graph UI port bandı {start}–{end} tamamen dolu — bir portu boşaltın veya Settings'te user modunda başka bir port seçin."
            ),
        }
    }
}

impl std::error::Error for PortSelectError {}

/// Varsayılan Lounge Graph UI port bandı (18749–18759).
pub fn default_graph_ui_port_band() -> RangeInclusive<u16> {
    GRAPH_UI_PORT_BAND_START..=GRAPH_UI_PORT_BAND_END
}

/// Lounge'un spawn ettiği cbm UI çocuğu + status single-flight + window port.
pub struct GraphUiState {
    child: Mutex<Option<Child>>,
    /// graph-window'un navigation guard'ında kilitli port.
    window_port: Mutex<Option<u16>>,
    port_mode: Mutex<GraphUiPortMode>,
    status_flight: AsyncMutex<StatusFlight>,
    /// Pre-spawn shared CBM `config.json` snapshot (cleared after guarded restore).
    cbm_config_snapshot: Mutex<Option<CbmUiConfigSnapshot>>,
    /// Test/prod override for shared CBM `config.json` path (avoids real HOME).
    cbm_config_path_override: Mutex<Option<PathBuf>>,
    /// Test-only: pre-bound LISTEN sockets handed to the next spawn on that port
    /// (avoids reserve→free→rebind races). Gated behind `test-helpers`.
    #[cfg(feature = "test-helpers")]
    listen_handoffs: Mutex<HashMap<u16, std::net::TcpListener>>,
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
            cbm_config_snapshot: Mutex::new(None),
            cbm_config_path_override: Mutex::new(None),
            #[cfg(feature = "test-helpers")]
            listen_handoffs: Mutex::new(HashMap::new()),
        }
    }

    /// Stage a reserved listener for the next Graph UI child on this port (tests).
    #[cfg(feature = "test-helpers")]
    pub fn stage_listen_handoff(&self, listener: std::net::TcpListener) {
        let port = match listener.local_addr() {
            Ok(addr) => addr.port(),
            Err(err) => {
                log::warn!("stage_listen_handoff: local_addr failed: {err}");
                return;
            }
        };
        let mut guard = match self.listen_handoffs.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.insert(port, listener);
    }

    #[cfg(feature = "test-helpers")]
    fn take_listen_handoff(&self, port: u16) -> Option<std::net::TcpListener> {
        let mut guard = match self.listen_handoffs.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.remove(&port)
    }

    /// Ports staged for listen handoff are treated as free for Auto selection.
    #[cfg(feature = "test-helpers")]
    pub fn has_listen_handoff(&self, port: u16) -> bool {
        let guard = match self.listen_handoffs.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.contains_key(&port)
    }

    pub fn set_cbm_config_path_override(&self, path: Option<PathBuf>) {
        let mut guard = match self.cbm_config_path_override.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = path;
    }

    pub fn cbm_config_path_override(&self) -> Option<PathBuf> {
        match self.cbm_config_path_override.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    fn resolve_cbm_config_path(&self) -> Option<PathBuf> {
        self.cbm_config_path_override()
            .or_else(super::cbm_ui_config::cbm_ui_config_path)
    }

    pub fn set_cbm_config_snapshot(&self, snapshot: Option<CbmUiConfigSnapshot>) {
        let mut guard = match self.cbm_config_snapshot.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = snapshot;
    }

    pub fn cbm_config_snapshot(&self) -> Option<CbmUiConfigSnapshot> {
        match self.cbm_config_snapshot.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    pub fn take_cbm_config_snapshot(&self) -> Option<CbmUiConfigSnapshot> {
        let mut guard = match self.cbm_config_snapshot.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.take()
    }

    /// Guarded restore + clear. No-op when snapshot already cleared after ready.
    pub fn restore_cbm_config_best_effort(&self) {
        let Some(snapshot) = self.take_cbm_config_snapshot() else {
            return;
        };
        match restore_cbm_ui_config(&snapshot) {
            Ok(RestoreOutcome::Restored) => {
                log::warn!("cbm ui config: guarded restore applied; snapshot cleared");
            }
            Ok(RestoreOutcome::Skipped) => {
                log::warn!("cbm ui config: guarded restore skipped; snapshot cleared");
            }
            Err(err) => {
                log::warn!("cbm ui config restore failed (snapshot cleared): {err}");
            }
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
        // Fallback restore if child re-persisted shared config on exit.
        self.restore_cbm_config_best_effort();
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
    /// Auto-mode informational note (preferred port busy → next free). Not an error.
    #[serde(default)]
    pub info_message: Option<String>,
    /// When Auto remaps away from a busy preferred port, the port that was skipped.
    #[serde(default)]
    pub remap_from_port: Option<u16>,
    /// i18n key for conflict/info (frontend translates; backend must not be TR-only).
    #[serde(default)]
    pub message_key: Option<String>,
    /// Params for `message_key` (`port`, `busy`, `next`, `start`, `end`).
    #[serde(default)]
    pub message_params: Option<serde_json::Map<String, serde_json::Value>>,
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
    band: RangeInclusive<u16>,
    is_free: impl Fn(u16) -> bool,
) -> Result<u16, PortSelectError> {
    let start = *band.start();
    let end = *band.end();
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
            if band.contains(&preferred) && is_free(preferred) {
                return Ok(preferred);
            }
            for port in band {
                if is_free(port) {
                    return Ok(port);
                }
            }
            Err(PortSelectError::BandExhausted { start, end })
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
    let preferred = bridge.http_port();
    let binary_found = bridge.binary_path().is_file();
    if !binary_found {
        return GraphUiStatus {
            binary_found: false,
            ui_available: false,
            project_indexed: false,
            cbm_project_name: None,
            port: preferred,
            port_conflict: false,
            conflict_message: None,
            info_message: None,
            remap_from_port: None,
            message_key: None,
            message_params: None,
            port_mode,
            owned_by_lounge: port_owned_by_lounge(preferred, child_pid),
        };
    }

    let classified = classify_port_status(
        preferred,
        child_pid,
        port_mode,
        default_graph_ui_port_band(),
    )
    .await;
    let port = classified.port;
    let owned_by_lounge = port_owned_by_lounge(port, child_pid);
    let remap_from_port = classified.remap_from_port;

    let cbm_project_name = if classified.ui_available {
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
        ui_available: classified.ui_available,
        project_indexed,
        cbm_project_name,
        port,
        port_conflict: classified.port_conflict,
        conflict_message: classified.conflict_message,
        info_message: classified.info_message,
        remap_from_port,
        message_key: classified.message_key,
        message_params: classified.message_params,
        port_mode,
        owned_by_lounge,
    }
}

fn msg_params(pairs: &[(&str, serde_json::Value)]) -> serde_json::Map<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortClassification {
    pub ui_available: bool,
    pub port: u16,
    pub port_conflict: bool,
    pub conflict_message: Option<String>,
    pub info_message: Option<String>,
    pub remap_from_port: Option<u16>,
    pub message_key: Option<String>,
    pub message_params: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Status classification: Auto never surfaces `port_conflict` for a foreign preferred
/// port when a free band successor exists — it reports that port + an info note.
/// When the selected next port is already owned by Lounge, probe and report availability.
pub async fn classify_port_status(
    preferred: u16,
    child_pid: Option<u32>,
    mode: GraphUiPortMode,
    band: RangeInclusive<u16>,
) -> PortClassification {
    let owned = port_owned_by_lounge(preferred, child_pid);
    if owned {
        if probe_ui_config(preferred).await {
            return PortClassification {
                ui_available: true,
                port: preferred,
                port_conflict: false,
                conflict_message: None,
                info_message: None,
                remap_from_port: None,
                message_key: None,
                message_params: None,
            };
        }
        return PortClassification {
            ui_available: false,
            port: preferred,
            port_conflict: false,
            conflict_message: None,
            info_message: None,
            remap_from_port: None,
            message_key: None,
            message_params: None,
        };
    }

    let foreign_ui = probe_ui_config(preferred).await;
    let tcp_open = tcp_ready("127.0.0.1", preferred, UI_PROBE_TIMEOUT).await;
    if !(foreign_ui || tcp_open) {
        return PortClassification {
            ui_available: false,
            port: preferred,
            port_conflict: false,
            conflict_message: None,
            info_message: None,
            remap_from_port: None,
            message_key: None,
            message_params: None,
        };
    }

    match mode {
        GraphUiPortMode::User => {
            let key = if foreign_ui {
                "graphMsgForeignUiUser"
            } else {
                "graphMsgPortBusyUser"
            };
            PortClassification {
                ui_available: false,
                port: preferred,
                port_conflict: true,
                conflict_message: None,
                info_message: None,
                remap_from_port: None,
                message_key: Some(key.into()),
                message_params: Some(msg_params(&[("port", preferred.into())])),
            }
        }
        GraphUiPortMode::Auto => {
            let is_free = |port: u16| {
                if port_owned_by_lounge(port, child_pid) {
                    return true;
                }
                // cfg(test): reserved listeners still held by the test process.
                #[cfg(test)]
                if classify_held_free_contains(port) {
                    return true;
                }
                listen_pids(port).is_empty()
            };
            match select_graph_ui_port(mode, preferred, band, is_free) {
                Ok(next) => {
                    let remapped = next != preferred;
                    let owned_next = port_owned_by_lounge(next, child_pid);
                    let ui_available = if owned_next {
                        probe_ui_config(next).await
                    } else {
                        false
                    };
                    PortClassification {
                        ui_available,
                        port: next,
                        port_conflict: false,
                        conflict_message: None,
                        info_message: None,
                        remap_from_port: remapped.then_some(preferred),
                        message_key: remapped.then(|| "graphAutoPortInfo".into()),
                        message_params: remapped.then(|| {
                            msg_params(&[("busy", preferred.into()), ("next", next.into())])
                        }),
                    }
                }
                Err(PortSelectError::BandExhausted { start, end }) => PortClassification {
                    ui_available: false,
                    port: preferred,
                    port_conflict: true,
                    conflict_message: None,
                    info_message: None,
                    remap_from_port: None,
                    message_key: Some("graphMsgBandExhausted".into()),
                    message_params: Some(msg_params(&[
                        ("start", start.into()),
                        ("end", end.into()),
                        ("port", preferred.into()),
                    ])),
                },
                Err(PortSelectError::UserConflict { port }) => PortClassification {
                    ui_available: false,
                    port,
                    port_conflict: true,
                    conflict_message: None,
                    info_message: None,
                    remap_from_port: None,
                    message_key: Some(
                        if foreign_ui {
                            "graphMsgForeignUiUser"
                        } else {
                            "graphMsgPortBusyUser"
                        }
                        .into(),
                    ),
                    message_params: Some(msg_params(&[("port", port.into())])),
                },
            }
        }
    }
}

/// UI available yalnız sahipli + ui-config. Legacy tuple helper for tests.
/// Auto + foreign preferred → `port_conflict=false` when a free band port exists.
pub async fn classify_port(
    port: u16,
    child_pid: Option<u32>,
    mode: GraphUiPortMode,
) -> (bool, bool, Option<String>) {
    let c = classify_port_status(port, child_pid, mode, default_graph_ui_port_band()).await;
    let msg = c
        .message_key
        .clone()
        .or(c.conflict_message)
        .or(c.info_message);
    (c.ui_available, c.port_conflict, msg)
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
    let session = app.try_state::<Arc<GeometrySession>>();
    let data_root = session
        .as_ref()
        .map(|s| s.data_root().to_path_buf())
        .unwrap_or_else(super::data_root);
    if let Some(window) = app.get_webview_window(GRAPH_WINDOW_LABEL) {
        if state.window_port() == Some(port) {
            window.navigate(url).context("graph-window navigate")?;
            window.set_focus().context("graph-window set_focus")?;
            return Ok(());
        }
        persist_window_geometry(
            app,
            &window,
            GRAPH_WINDOW_LABEL,
            session.as_ref().map(|s| s.inner().as_ref()),
        );
        let _ = window.destroy();
        state.set_window_port(None);
    }

    let placement = placement_for_window(app, GRAPH_WINDOW_LABEL, &data_root);
    let allowed_port = port;
    let window =
        WebviewWindowBuilder::new(app, GRAPH_WINDOW_LABEL, WebviewUrl::External(url.clone()))
            .title("Codebase Memory · 3D Graph")
            .inner_size(placement.geometry.width, placement.geometry.height)
            .min_inner_size(placement.min_width, placement.min_height)
            .resizable(true)
            .visible(false)
            .on_navigation(move |nav| navigation_allowed(nav, allowed_port))
            .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Deny)
            .build()
            .context("graph-window create")?;
    apply_placement_show(&window, &placement).map_err(|err| anyhow::anyhow!("{err}"))?;
    state.set_window_port(Some(port));
    Ok(())
}

/// Main kapanınca graph-window + Lounge child temizliği.
pub fn on_main_window_closed(app: &AppHandle, state: &GraphUiState, bridge: Option<&MemoryBridge>) {
    if let Some(window) = app.get_webview_window(GRAPH_WINDOW_LABEL) {
        let _ = window.destroy();
    }
    state.set_window_port(None);
    // kill_spawned_child performs guarded restore + clear when a snapshot remains.
    state.kill_spawned_child();
    if let Some(bridge) = bridge {
        bridge.set_owned_ui_pid(None);
    }
}

fn sync_owned_pid(bridge: &MemoryBridge, state: &GraphUiState) {
    bridge.set_owned_ui_pid(state.spawned_child_pid());
}

/// Spawn `--ui=true --port=N`; sahiplik doğrulanana kadar bekler.
pub async fn spawn_graph_ui_on_port(
    state: &GraphUiState,
    bridge: &MemoryBridge,
    port: u16,
) -> Result<()> {
    spawn_graph_ui_on_port_with_store(state, bridge, port, None).await
}

pub async fn spawn_graph_ui_on_port_with_store(
    state: &GraphUiState,
    bridge: &MemoryBridge,
    port: u16,
    store: Option<&crate::db::ExperienceStore>,
) -> Result<()> {
    let binary = bridge.binary_path();
    if !binary.is_file() {
        bail!("codebase-memory-mcp bulunamadı");
    }

    // Keep a pending older snapshot (pre-pollution) when re-spawning; only update lounge_port.
    let snapshot = if let Some(mut existing) = state.cbm_config_snapshot() {
        existing.lounge_port = Some(port);
        state.set_cbm_config_snapshot(Some(existing.clone()));
        Some(existing)
    } else {
        match state.resolve_cbm_config_path() {
            Some(path) => match snapshot_cbm_ui_config_at(&path, Some(port)) {
                Ok(s) => {
                    state.set_cbm_config_snapshot(Some(s.clone()));
                    if let Some(store) = store {
                        if let Err(err) = persist_snapshot_to_store(store, &s).await {
                            log::warn!("persist cbm config snapshot: {err}");
                        }
                    }
                    Some(s)
                }
                Err(err) => {
                    log::warn!("cbm ui config snapshot failed (continuing spawn): {err}");
                    None
                }
            },
            None => {
                log::warn!("cbm ui config path unresolved — skipping snapshot");
                None
            }
        }
    };

    let mut command = GuardedCommand::new(binary)
        .arg("--ui=true")
        .arg(format!("--port={port}"))
        .internal_daemon()
        .into_std_command()
        .with_context(|| format!("graph UI gate başarısız: {}", binary.display()))?;
    command.stdout(Stdio::null());

    #[cfg(feature = "test-helpers")]
    let pending_handoff = {
        if let Some(listener) = state.take_listen_handoff(port) {
            // Unix: stdin null + --listen-fd (CLOEXEC cleared in child pre_exec).
            // Windows: attach converts exclusive→SO_REUSEADDR then --reuse-bind;
            // stderr piped for listen-adopted. Skip CREATE_NO_WINDOW.

            #[cfg(unix)]
            {
                command.stdin(Stdio::null());
                command.stderr(Stdio::null());
            }
            #[cfg(windows)]
            {
                command.stderr(Stdio::piped());
            }
            Some(
                super::listen_handoff::attach_inherited_listener_owned(&mut command, listener)
                    .context("attach Graph UI listen handoff")?,
            )
        } else {
            command.stdin(Stdio::piped());
            command.stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000);
            }
            None
        }
    };
    #[cfg(not(feature = "test-helpers"))]
    {
        command.stdin(Stdio::piped());
        command.stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            #[cfg(feature = "test-helpers")]
            drop(pending_handoff);
            state.restore_cbm_config_best_effort();
            if let Some(store) = store {
                let _ = clear_snapshot_in_store(store).await;
            }
            return Err(err)
                .with_context(|| format!("graph UI spawn başarısız: {}", binary.display()));
        }
    };
    #[cfg(feature = "test-helpers")]
    if let Some(pending) = pending_handoff {
        // Drops parent LISTEN after child's listen-adopted (Windows).
        if let Err(err) = super::listen_handoff::complete_listen_handoff(pending, &mut child) {
            let _ = child.kill();
            let _ = child.wait();
            state.restore_cbm_config_best_effort();
            if let Some(store) = store {
                let _ = clear_snapshot_in_store(store).await;
            }
            return Err(err).context("complete Graph UI listen handoff");
        }
    }
    // Drain leftover stderr (do not close the pipe — Windows helpers die on
    // ERROR_BROKEN_PIPE if the parent drops the read end while they still write).
    if let Some(mut stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = [0u8; 512];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
    }
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
        if let Some(store) = store {
            let _ = clear_snapshot_in_store(store).await;
        }
        sync_owned_pid(bridge, state);
        bail!("Graph UI {ENABLE_READY_DEADLINE:?} içinde hazır olmadı — SemanticMap'e düşülüyor");
    }

    // HTTP probe already proved the socket accepts — still poll ownership briefly
    // so Windows GetExtendedTcpTable / netstat can catch up after bind (same race
    // as the tcp-hold integration test). Bounded; not a retry-until-pass mask.
    let owned = wait_until(
        Duration::from_secs(3),
        Duration::from_millis(50),
        || async { port_owned_by_lounge(port, state.spawned_child_pid()) },
    )
    .await;
    if !owned {
        let pid = state.spawned_child_pid();
        let pids = listen_pids(port);
        state.kill_spawned_child();
        if let Some(store) = store {
            let _ = clear_snapshot_in_store(store).await;
        }
        sync_owned_pid(bridge, state);
        bail!("Graph UI port {port} sahiplik doğrulaması başarısız (child={pid:?}, listen_pids={pids:?})");
    }

    // Ownership verified — guarded restore once, then clear so stop/exit cannot clobber.
    if snapshot.is_some() {
        state.restore_cbm_config_best_effort();
        if let Some(store) = store {
            if let Err(err) = clear_snapshot_in_store(store).await {
                log::warn!("clear cbm config snapshot: {err}");
            }
        }
    }

    bridge.set_http_port(port);
    Ok(())
}

/// Graph UI'yi pencere açmadan başlatır (test + enable_graph_ui çekirdeği).
/// `band` enjekte edilebilir — CI'da 18749 meşgul olsa da ardışık boş port çifti kullanılır.
pub async fn enable_graph_ui_headless(
    state: &GraphUiState,
    bridge: &MemoryBridge,
    store: Option<&crate::db::ExperienceStore>,
    band: RangeInclusive<u16>,
) -> Result<u16> {
    let mode = state.port_mode();
    let preferred = bridge.http_port();
    let child_pid = state.spawned_child_pid();
    let band_start = *band.start();

    if port_owned_by_lounge(preferred, child_pid) && probe_ui_config(preferred).await {
        return Ok(preferred);
    }

    // Prefer LISTEN-PID emptiness over bind-and-release probes: on macOS a
    // successful `tcp_bind_available` can leave the port briefly unusable for the
    // child (TIME_WAIT), causing a false "port free → spawn failed" race.
    let is_free = |port: u16| {
        if port_owned_by_lounge(port, state.spawned_child_pid()) {
            return true;
        }
        // Staged handoff listeners still LISTEN in the parent; treat as free so
        // Auto can select them without a free→rebind race.
        #[cfg(feature = "test-helpers")]
        if state.has_listen_handoff(port) {
            return true;
        }
        listen_pids(port).is_empty()
    };

    let selected = match select_graph_ui_port(mode, preferred, band.clone(), is_free) {
        Ok(p) => p,
        Err(err) => bail!("{err}"),
    };

    if port_owned_by_lounge(selected, state.spawned_child_pid()) && probe_ui_config(selected).await
    {
        bridge.set_http_port(selected);
        return Ok(selected);
    }

    match spawn_graph_ui_on_port_with_store(state, bridge, selected, store).await {
        Ok(()) => {}
        Err(first_err) => {
            if mode == GraphUiPortMode::Auto {
                let retry_preferred = selected.saturating_add(1).max(band_start);
                let retry = select_graph_ui_port(mode, retry_preferred, band, |p| {
                    p != selected && is_free(p)
                });
                match retry {
                    Ok(next) if next != selected => {
                        log::warn!(
                            "graph UI bind race on {selected}, retrying on {next}: {first_err}"
                        );
                        spawn_graph_ui_on_port_with_store(state, bridge, next, store).await?;
                    }
                    Ok(_) | Err(PortSelectError::BandExhausted { .. }) => {
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
        // Snapshot is cleared after post-ready restore; ensure store is clean too.
        if state.cbm_config_snapshot().is_none() {
            let _ = clear_snapshot_in_store(store).await;
        }
    }
    Ok(live_port)
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
    let live_port =
        enable_graph_ui_headless(state, bridge, store, default_graph_ui_port_band()).await?;
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
    fn restore_best_effort_clears_snapshot() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "lounge-snap-clear-{}-{}",
            std::process::id(),
            nanos
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        let snap =
            crate::services::cbm_ui_config::snapshot_cbm_ui_config_at(&path, Some(18749)).unwrap();
        let state = GraphUiState::new();
        state.set_cbm_config_path_override(Some(path.clone()));
        state.set_cbm_config_snapshot(Some(snap));
        // Write Lounge footprint then restore+clear.
        crate::services::cbm_ui_config::write_cbm_ui_config_atomic(
            &path,
            &crate::services::cbm_ui_config::CbmUiConfigFile {
                ui_enabled: Some(true),
                ui_port: Some(18749),
                extra: Default::default(),
            },
        )
        .unwrap();
        state.restore_cbm_config_best_effort();
        assert!(state.cbm_config_snapshot().is_none());
        // Second call is a no-op (snapshot already cleared).
        state.restore_cbm_config_best_effort();
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

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
        let port = select_graph_ui_port(
            GraphUiPortMode::Auto,
            18751,
            default_graph_ui_port_band(),
            |_| true,
        )
        .unwrap();
        assert_eq!(port, 18751);
    }

    #[test]
    fn auto_skips_busy_and_picks_next_in_band() {
        let busy: HashSet<u16> = [18749, 18750].into_iter().collect();
        let port = select_graph_ui_port(
            GraphUiPortMode::Auto,
            18749,
            default_graph_ui_port_band(),
            |p| !busy.contains(&p),
        )
        .unwrap();
        assert_eq!(port, 18751);
    }

    #[test]
    fn auto_band_exhausted_returns_clear_error() {
        let err = select_graph_ui_port(
            GraphUiPortMode::Auto,
            18749,
            default_graph_ui_port_band(),
            |_| false,
        )
        .unwrap_err();
        assert_eq!(
            err,
            PortSelectError::BandExhausted {
                start: GRAPH_UI_PORT_BAND_START,
                end: GRAPH_UI_PORT_BAND_END
            }
        );
        let msg = err.to_string();
        assert!(msg.contains("18749"));
        assert!(msg.contains("18759"));
    }

    #[test]
    fn user_mode_conflict_does_not_switch() {
        let err = select_graph_ui_port(
            GraphUiPortMode::User,
            9749,
            default_graph_ui_port_band(),
            |_| false,
        )
        .unwrap_err();
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
            select_graph_ui_port(
                GraphUiPortMode::User,
                19001,
                default_graph_ui_port_band(),
                |_| true
            )
            .unwrap(),
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

    /// Reserve preferred + two successors; serve foreign UI by handing over the
    /// reserved preferred listener (no free→rebind). Successors stay bound for
    /// the whole test — registered as held-free so Auto can select them without
    /// a drop→steal TOCTOU (review nit N11).
    async fn reserve_classify_band() -> (u16, u16, Vec<tokio::net::TcpListener>) {
        for _ in 0..300 {
            let hold_preferred = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
                Ok(l) => l,
                Err(_) => continue,
            };
            let preferred = hold_preferred.local_addr().unwrap().port();
            if preferred >= u16::MAX - 2 {
                continue;
            }
            let mut holds = Vec::new();
            let mut ok = true;
            for offset in 1u16..=2 {
                match tokio::net::TcpListener::bind(("127.0.0.1", preferred + offset)).await {
                    Ok(l) => holds.push(l),
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                continue;
            }
            let app = axum::Router::new().route(
                "/api/ui-config",
                axum::routing::get(|| async {
                    axum::Json(serde_json::json!({"lang": "en", "foreign": true}))
                }),
            );
            tokio::spawn(async move {
                let _ = axum::serve(hold_preferred, app).await;
            });
            for _ in 0..40 {
                if probe_ui_config(preferred).await {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(probe_ui_config(preferred).await, "foreign ui up");
            return (preferred, preferred + 2, holds);
        }
        panic!("could not reserve classify band");
    }

    fn held_free_ports_from(holds: &[tokio::net::TcpListener]) -> Vec<u16> {
        holds
            .iter()
            .map(|l| l.local_addr().expect("held addr").port())
            .collect()
    }

    async fn with_held_free_ports<F, Fut, R>(ports: Vec<u16>, f: F) -> R
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = R>,
    {
        // Per-test add/remove — never overwrite the global list (parallel tests).
        {
            let mut guard = CLASSIFY_HELD_FREE_PORTS.lock().expect("held-free lock");
            for p in &ports {
                if !guard.contains(p) {
                    guard.push(*p);
                }
            }
        }
        let result = f().await;
        {
            let mut guard = CLASSIFY_HELD_FREE_PORTS.lock().expect("held-free lock");
            guard.retain(|p| !ports.contains(p));
        }
        result
    }

    #[tokio::test]
    async fn classify_auto_foreign_reports_next_port_without_conflict() {
        let (preferred, band_end, free_holds) = reserve_classify_band().await;
        // Keep successors LISTEN-held so nothing can steal them; mark held-free.
        let held_ports = held_free_ports_from(&free_holds);

        let band = preferred..=band_end;
        let classified = with_held_free_ports(held_ports.clone(), || async {
            classify_port_status(preferred, None, GraphUiPortMode::Auto, band.clone()).await
        })
        .await;
        assert!(!classified.ui_available);
        assert!(!classified.port_conflict);
        assert!(classified.port > preferred && classified.port <= band_end);
        assert_eq!(classified.remap_from_port, Some(preferred));
        assert_eq!(classified.message_key.as_deref(), Some("graphAutoPortInfo"));

        let user = classify_port_status(preferred, None, GraphUiPortMode::User, band.clone()).await;
        assert!(user.port_conflict);
        assert_eq!(user.port, preferred);
        assert_eq!(user.message_key.as_deref(), Some("graphMsgForeignUiUser"));

        let exhausted = classify_port_status(
            preferred,
            None,
            GraphUiPortMode::Auto,
            preferred..=preferred,
        )
        .await;
        assert!(exhausted.port_conflict);
        assert_eq!(
            exhausted.message_key.as_deref(),
            Some("graphMsgBandExhausted")
        );
        drop(free_holds);
    }

    #[tokio::test]
    async fn classify_user_conflict_keeps_preferred_port() {
        let (preferred, _end, free_holds) = reserve_classify_band().await;
        // Keep holds — User mode does not scan successors; no drop→steal window.
        let (available, conflict, msg) =
            classify_port(preferred, None, GraphUiPortMode::User).await;
        assert!(!available);
        assert!(conflict);
        let msg = msg.expect("message key");
        assert_eq!(msg, "graphMsgForeignUiUser");
        drop(free_holds);
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
                                info_message: None,
                                remap_from_port: None,
                                message_key: None,
                                message_params: None,
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
