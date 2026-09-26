//! codebase-memory-mcp 3D Graph UI — probe, spawn, singleton window.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::Mutex as AsyncMutex;

use super::memory_bridge::{
    probe_ui_config, MemoryBridge, DEFAULT_GRAPH_UI_PORT, UI_PROBE_TIMEOUT,
};
use super::probe::{tcp_ready, wait_until};

pub const GRAPH_WINDOW_LABEL: &str = "graph-window";
const ENABLE_READY_DEADLINE: Duration = Duration::from_secs(10);
const SETTINGS_KEY: &str = "graph_ui_port";
/// Single-flight join penceresi — lock serbest kalınca bekleyenler taze sonucu alır.
const STATUS_JOIN_WINDOW: Duration = Duration::from_millis(500);

/// Lounge'un spawn ettiği cbm UI çocuğu + status single-flight + window port.
pub struct GraphUiState {
    child: Mutex<Option<Child>>,
    /// graph-window'un navigation guard'ında kilitli port.
    window_port: Mutex<Option<u16>>,
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
            status_flight: AsyncMutex::new(StatusFlight {
                last: None,
                finished_at: None,
            }),
        }
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
}

pub async fn load_port_from_store(store: &crate::db::ExperienceStore) -> u16 {
    match store.get_setting(SETTINGS_KEY.into()).await {
        Ok(Some(raw)) => parse_port_setting(&raw).unwrap_or(DEFAULT_GRAPH_UI_PORT),
        _ => DEFAULT_GRAPH_UI_PORT,
    }
}

pub async fn persist_port(
    store: &crate::db::ExperienceStore,
    bridge: &MemoryBridge,
    app: &AppHandle,
    state: &GraphUiState,
    port: u16,
) -> Result<()> {
    if port == 0 {
        bail!("port 0 geçersiz");
    }
    bridge.set_http_port(port);
    store
        .set_setting(SETTINGS_KEY.into(), port.to_string())
        .await
        .context("graph_ui_port kaydedilemedi")?;
    // Stale navigation lock: port değiştiyse destroy (sync) — aynı label ile recreate çakışmasın.
    if state.window_port() != Some(port) {
        if let Some(window) = app.get_webview_window(GRAPH_WINDOW_LABEL) {
            let _ = window.destroy();
        }
        state.set_window_port(None);
    }
    Ok(())
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
    state
        .status_single_flight(
            || async move { graph_ui_status_inner(&bridge, root.as_deref()).await },
        )
        .await
}

async fn graph_ui_status_inner(bridge: &MemoryBridge, project_root: Option<&str>) -> GraphUiStatus {
    let port = bridge.http_port();
    let binary_found = bridge.binary_path().is_file();
    if !binary_found {
        return GraphUiStatus {
            binary_found: false,
            ui_available: false,
            project_indexed: false,
            cbm_project_name: None,
            port,
            port_conflict: false,
            conflict_message: None,
        };
    }

    let (ui_available, port_conflict, conflict_message) = classify_port(port).await;

    // list_projects yalnızca UI ayaktayken — aksi halde CLI hang yığını.
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
    }
}

/// TCP açık ama `/api/ui-config` 200 JSON değilse port başka sürece ait.
pub async fn classify_port(port: u16) -> (bool, bool, Option<String>) {
    if probe_ui_config(port).await {
        return (true, false, None);
    }
    if tcp_ready("127.0.0.1", port, UI_PROBE_TIMEOUT).await {
        let msg = format!(
            "Port {port} başka bir süreç tarafından kullanılıyor — Settings'te Graph UI Port'u değiştirin veya o süreci kontrol edin."
        );
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
        // Port değişti — stale navigation lock; sync destroy sonra aynı label ile recreate.
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
pub fn on_main_window_closed(app: &AppHandle, state: &GraphUiState) {
    if let Some(window) = app.get_webview_window(GRAPH_WINDOW_LABEL) {
        let _ = window.destroy();
    }
    state.set_window_port(None);
    state.kill_spawned_child();
}

/// `--ui=true --port=N` spawn; stdin piped (kapanmaz). Hazır olmazsa çocuğu öldürür.
pub async fn enable_graph_ui(
    app: &AppHandle,
    state: &GraphUiState,
    bridge: &MemoryBridge,
    cbm_project: Option<&str>,
) -> Result<()> {
    let port = bridge.http_port();
    let (available, conflict, conflict_msg) = classify_port(port).await;
    if conflict {
        bail!(
            "{}",
            conflict_msg.unwrap_or_else(|| format!("Port {port} meşgul"))
        );
    }
    if available {
        if let Some(name) = cbm_project.filter(|s| !s.is_empty()) {
            open_or_focus_graph_window(app, state, port, name)?;
        }
        return Ok(());
    }

    let binary = bridge.binary_path();
    if !binary.is_file() {
        bail!("codebase-memory-mcp bulunamadı");
    }

    let mut command = Command::new(binary);
    command
        .arg("--ui=true")
        .arg(format!("--port={port}"))
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

    let ready = wait_until(
        ENABLE_READY_DEADLINE,
        Duration::from_millis(250),
        || async { probe_ui_config(port).await },
    )
    .await;

    if !ready {
        state.kill_spawned_child();
        bail!("Graph UI {ENABLE_READY_DEADLINE:?} içinde hazır olmadı — SemanticMap'e düşülüyor");
    }

    if let Some(name) = cbm_project.filter(|s| !s.is_empty()) {
        open_or_focus_graph_window(app, state, port, name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::memory_bridge::{MemoryBridgeConfig, TransportMode};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn parses_port_setting_variants() {
        assert_eq!(parse_port_setting("9749"), Some(9749));
        assert_eq!(parse_port_setting("\"9749\""), Some(9749));
        assert_eq!(parse_port_setting("0"), None);
        assert_eq!(parse_port_setting("nope"), None);
    }

    #[test]
    fn navigation_locked_to_loopback_port() {
        let ok = Url::parse("http://127.0.0.1:9749/?tab=graph&project=x").unwrap();
        assert!(navigation_allowed(&ok, 9749));
        let bad_host = Url::parse("http://example.com:9749/").unwrap();
        assert!(!navigation_allowed(&bad_host, 9749));
        let bad_port = Url::parse("http://127.0.0.1:80/").unwrap();
        assert!(!navigation_allowed(&bad_port, 9749));
        let https = Url::parse("https://127.0.0.1:9749/").unwrap();
        assert!(!navigation_allowed(&https, 9749));
    }

    #[test]
    fn graph_url_includes_tab_and_project() {
        let url = graph_project_url(9749, "Users-demo-Agent-Lounge-OS").unwrap();
        assert_eq!(url.host_str(), Some("127.0.0.1"));
        assert_eq!(url.port(), Some(9749));
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
    }
}
