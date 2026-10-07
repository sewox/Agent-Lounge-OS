//! Monitor-aware window sizing and per-monitor geometry persistence.
//!
//! Pure geometry helpers are display-free and unit-tested. Tauri wiring lives
//! here so main + graph windows share one code path.
//!
//! # Work-area behaviour (platform notes)
//!
//! - **macOS / Windows:** `Monitor::work_area()` excludes the menu bar, dock,
//!   and taskbar when the platform reports a visible frame. If the runtime
//!   cannot obtain a visible frame it falls back to the full monitor size.
//! - **Wayland:** many compositors expose only the full geometry as the work
//!   area (no exclusive-zone subtraction); we still centre at 90% of whatever
//!   is reported so behaviour stays consistent cross-platform.
//! - Positions are stored and restored in **global physical pixels** so
//!   mixed-DPI layouts (Windows / X11) do not round-trip through a single
//!   logical scale. Inner size stays logical (CSS pixels).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Monitor, PhysicalPosition, Runtime, WebviewWindow, WindowEvent};

/// Matches `tauri.conf.json` main window label.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Persistence file under the app data / data-root directory.
pub const GEOMETRY_FILE_NAME: &str = "window_geometry.json";

/// First-open fill of the monitor work area (not maximized).
pub const FILL_RATIO: f64 = 0.90;

/// Preferred minimum logical width — fits portrait 1080-wide work areas.
pub const MIN_WIDTH_LOGICAL: f64 = 960.0;

/// Preferred minimum logical height.
pub const MIN_HEIGHT_LOGICAL: f64 = 640.0;

/// Fallback when no monitor can be detected (matches historical defaults).
pub const FALLBACK_WIDTH: f64 = 1280.0;
pub const FALLBACK_HEIGHT: f64 = 800.0;

/// Debounce interval for disk writes after Moved/Resized.
pub const PERSIST_DEBOUNCE: Duration = Duration::from_millis(500);

/// Minimum overlap of the window with the work area to accept a restore (25%).
pub const MIN_OVERLAP_RATIO: f64 = 0.25;

/// Top strip height (physical px) treated as "title bar visible".
const TOP_STRIP_PHYS: i32 = 32;

/// Logical-pixel rectangle (CSS units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Physical rectangle in global desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Display-free monitor snapshot used by pure geometry functions.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorSnapshot {
    pub name: String,
    pub scale_factor: f64,
    /// Full monitor resolution in physical pixels (persistence key).
    pub physical_width: u32,
    pub physical_height: u32,
    /// Usable area in **global physical** pixels.
    pub work_area_phys: PhysRect,
}

/// Saved / resolved placement: physical outer origin + logical inner size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SavedGeometry {
    pub x_phys: i32,
    pub y_phys: i32,
    pub width: f64,
    pub height: f64,
}

/// Per-window record: last monitor + per-monitor geometries.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_monitor: Option<String>,
    #[serde(default)]
    pub monitors: BTreeMap<String, SavedGeometry>,
}

/// On-disk store.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeometryStore {
    #[serde(default)]
    pub windows: BTreeMap<String, WindowRecord>,
}

/// Resolved open plan for a window.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenPlacement {
    pub geometry: SavedGeometry,
    /// When true, use toolkit centring (no monitor available).
    pub center: bool,
    pub min_width: f64,
    pub min_height: f64,
}

/// In-memory cache + debounced disk writer (shared via `app.manage`).
pub struct GeometrySession {
    data_root: PathBuf,
    cache: Mutex<BTreeMap<String, CacheEntry>>,
    generation: AtomicU64,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    monitor_key: String,
    geometry: SavedGeometry,
}

/// Build the persistence key: monitor name + physical resolution.
pub fn monitor_key(name: &str, physical_width: u32, physical_height: u32) -> String {
    let name = if name.trim().is_empty() {
        "unknown"
    } else {
        name.trim()
    };
    format!("{name}|{physical_width}x{physical_height}")
}

impl MonitorSnapshot {
    pub fn key(&self) -> String {
        monitor_key(&self.name, self.physical_width, self.physical_height)
    }

    pub fn work_area_logical(&self) -> LogicalRect {
        physical_to_logical_rect(
            self.work_area_phys.x,
            self.work_area_phys.y,
            self.work_area_phys.width,
            self.work_area_phys.height,
            self.scale_factor,
        )
    }
}

/// Convert physical bounds to logical using `scale` (HiDPI-safe).
pub fn physical_to_logical_rect(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
) -> LogicalRect {
    let scale = sanitize_scale(scale);
    LogicalRect {
        x: x as f64 / scale,
        y: y as f64 / scale,
        width: width as f64 / scale,
        height: height as f64 / scale,
    }
}

/// Logical size → physical size at `scale`.
pub fn logical_to_physical_size(width: f64, height: f64, scale: f64) -> (u32, u32) {
    let scale = sanitize_scale(scale);
    (
        (width * scale).round().max(1.0) as u32,
        (height * scale).round().max(1.0) as u32,
    )
}

fn sanitize_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// Preferred mins capped by the work area (logical).
pub fn effective_min_size(work_area_logical: &LogicalRect) -> (f64, f64) {
    (
        MIN_WIDTH_LOGICAL.min(work_area_logical.width.max(1.0)),
        MIN_HEIGHT_LOGICAL.min(work_area_logical.height.max(1.0)),
    )
}

/// Convert a Tauri monitor into a physical work-area snapshot.
pub fn snapshot_from_monitor(monitor: &Monitor) -> MonitorSnapshot {
    let scale = sanitize_scale(monitor.scale_factor());
    let work = monitor.work_area();
    let size = monitor.size();
    MonitorSnapshot {
        name: monitor
            .name()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "unknown".into()),
        scale_factor: scale,
        physical_width: size.width,
        physical_height: size.height,
        work_area_phys: PhysRect {
            x: work.position.x,
            y: work.position.y,
            width: work.size.width,
            height: work.size.height,
        },
    }
}

fn clamp_size(desired: f64, min: f64, max: f64) -> f64 {
    if !max.is_finite() || max <= 0.0 {
        return desired.max(1.0);
    }
    let floor = if min.is_finite() && min > 0.0 {
        min.min(max)
    } else {
        1.0_f64.min(max)
    };
    desired.max(floor).min(max)
}

/// Intersection area of two physical rects (i64 to avoid overflow).
fn intersection_area(a: &PhysRect, b: &PhysRect) -> i64 {
    let ax2 = a.x as i64 + a.width as i64;
    let ay2 = a.y as i64 + a.height as i64;
    let bx2 = b.x as i64 + b.width as i64;
    let by2 = b.y as i64 + b.height as i64;
    let ix1 = (a.x as i64).max(b.x as i64);
    let iy1 = (a.y as i64).max(b.y as i64);
    let ix2 = ax2.min(bx2);
    let iy2 = ay2.min(by2);
    let w = (ix2 - ix1).max(0);
    let h = (iy2 - iy1).max(0);
    w * h
}

fn window_phys_rect(geom: &SavedGeometry, scale: f64) -> PhysRect {
    let (w, h) = logical_to_physical_size(geom.width, geom.height, scale);
    PhysRect {
        x: geom.x_phys,
        y: geom.y_phys,
        width: w,
        height: h,
    }
}

/// True when the window overlaps the work area enough to restore (or its top strip is visible).
pub fn geometry_restorable(geom: &SavedGeometry, monitor: &MonitorSnapshot) -> bool {
    if !geom.width.is_finite() || !geom.height.is_finite() || geom.width < 1.0 || geom.height < 1.0
    {
        return false;
    }
    let win = window_phys_rect(geom, monitor.scale_factor);
    let work = &monitor.work_area_phys;
    let win_area = (win.width as i64) * (win.height as i64);
    if win_area <= 0 {
        return false;
    }
    let overlap = intersection_area(&win, work) as f64 / win_area as f64;
    if overlap >= MIN_OVERLAP_RATIO {
        return true;
    }
    // Top strip (title bar) intersects the work area.
    let strip = PhysRect {
        x: win.x,
        y: win.y,
        width: win.width,
        height: TOP_STRIP_PHYS.max(1) as u32,
    };
    intersection_area(&strip, work) > 0
}

/// Clamp the outer origin into the work area (keeps size; softens Aero Snap x≈-7).
pub fn clamp_geometry_to_work_area(
    geom: SavedGeometry,
    monitor: &MonitorSnapshot,
) -> SavedGeometry {
    let work = &monitor.work_area_phys;
    let (w_phys, h_phys) = logical_to_physical_size(geom.width, geom.height, monitor.scale_factor);
    let work_w = work.width as i32;
    let work_h = work.height as i32;
    let max_x = work.x + work_w - w_phys as i32;
    let max_y = work.y + work_h - h_phys as i32;
    // If the window is wider/taller than the work area, pin to the origin.
    let x = if max_x >= work.x {
        geom.x_phys.clamp(work.x, max_x)
    } else {
        work.x
    };
    let y = if max_y >= work.y {
        geom.y_phys.clamp(work.y, max_y)
    } else {
        work.y
    };
    let work_log = monitor.work_area_logical();
    let (min_w, min_h) = effective_min_size(&work_log);
    let width = clamp_size(geom.width, min_w, work_log.width)
        .round()
        .max(1.0);
    let height = clamp_size(geom.height, min_h, work_log.height)
        .round()
        .max(1.0);
    SavedGeometry {
        x_phys: x,
        y_phys: y,
        width,
        height,
    }
}

/// 90% of the work area, centred in physical space.
pub fn default_centered_geometry(monitor: &MonitorSnapshot, ratio: f64) -> SavedGeometry {
    let ratio = if ratio.is_finite() && ratio > 0.0 && ratio <= 1.0 {
        ratio
    } else {
        FILL_RATIO
    };
    let work_log = monitor.work_area_logical();
    let (min_w, min_h) = effective_min_size(&work_log);
    let width = clamp_size(work_log.width * ratio, min_w, work_log.width)
        .round()
        .max(1.0);
    let height = clamp_size(work_log.height * ratio, min_h, work_log.height)
        .round()
        .max(1.0);
    let (w_phys, h_phys) = logical_to_physical_size(width, height, monitor.scale_factor);
    let work = &monitor.work_area_phys;
    let x = work.x + ((work.width as i32 - w_phys as i32) / 2);
    let y = work.y + ((work.height as i32 - h_phys as i32) / 2);
    SavedGeometry {
        x_phys: x,
        y_phys: y,
        width,
        height,
    }
}

/// Restore from last-used monitor when present and restorable; else 90% on primary.
pub fn resolve_open_placement(
    window_label: &str,
    monitors: &[MonitorSnapshot],
    primary: &MonitorSnapshot,
    store: &GeometryStore,
) -> OpenPlacement {
    let work_log = primary.work_area_logical();
    let (min_w, min_h) = effective_min_size(&work_log);

    if let Some(record) = store.windows.get(window_label) {
        if let Some(key) = record.last_monitor.as_deref() {
            if let Some(mon) = monitors.iter().find(|m| m.key() == key) {
                if let Some(saved) = record.monitors.get(key) {
                    if geometry_restorable(saved, mon) {
                        let geometry = clamp_geometry_to_work_area(*saved, mon);
                        let (mw, mh) = effective_min_size(&mon.work_area_logical());
                        return OpenPlacement {
                            geometry,
                            center: false,
                            min_width: mw,
                            min_height: mh,
                        };
                    }
                }
            }
        }
    }

    OpenPlacement {
        geometry: default_centered_geometry(primary, FILL_RATIO),
        center: false,
        min_width: min_w,
        min_height: min_h,
    }
}

/// Pure gate: skip persist when the window is minimized / maximized / fullscreen.
pub fn persist_allowed(minimized: bool, maximized: bool, fullscreen: bool) -> bool {
    !(minimized || maximized || fullscreen)
}

/// Path to the geometry JSON under `data_root`.
pub fn geometry_store_path(data_root: &Path) -> PathBuf {
    data_root.join(GEOMETRY_FILE_NAME)
}

fn remove_file_logged(path: &Path, context: &str) {
    if let Err(err) = fs::remove_file(path) {
        if path.exists() {
            log::warn!("{context}: {err} ({})", path.display());
        }
    }
}

/// Quarantine a corrupt store so the next write starts clean.
pub fn quarantine_corrupt_store(path: &Path) {
    if !path.exists() {
        return;
    }
    let corrupt = path.with_extension("json.corrupt");
    if let Err(err) = fs::rename(path, &corrupt) {
        log::warn!(
            "window geometry bozuk dosya taşınamadı ({} → {}): {err}",
            path.display(),
            corrupt.display()
        );
        remove_file_logged(path, "window geometry bozuk dosya silinemedi");
    } else {
        log::warn!(
            "window geometry JSON bozuk; {} olarak ayrıldı",
            corrupt.display()
        );
    }
}

/// Load store from disk. Missing file → empty. Corrupt → quarantine + empty + warn via Err.
pub fn load_geometry_store(path: &Path) -> Result<GeometryStore, String> {
    if !path.exists() {
        return Ok(GeometryStore::default());
    }
    let raw = fs::read_to_string(path)
        .map_err(|err| format!("window geometry okunamadı ({}): {err}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(GeometryStore::default());
    }
    serde_json::from_str(&raw)
        .map_err(|err| format!("window geometry JSON bozuk ({}): {err}", path.display()))
}

/// Load, quarantining corrupt files and returning an empty store after `log::warn!`.
pub fn load_geometry_store_or_default(path: &Path) -> GeometryStore {
    match load_geometry_store(path) {
        Ok(s) => s,
        Err(err) => {
            log::warn!("{err}");
            if err.contains("JSON bozuk") {
                quarantine_corrupt_store(path);
            }
            GeometryStore::default()
        }
    }
}

/// Atomically write the store (`*.json.part` → rename only).
pub fn save_geometry_store(path: &Path, store: &GeometryStore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "window geometry dizini oluşturulamadı ({}): {err}",
                parent.display()
            )
        })?;
    }
    let json = serde_json::to_string_pretty(store)
        .map_err(|err| format!("window geometry serileştirilemedi: {err}"))?;
    write_atomic(path, json.as_bytes())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(GEOMETRY_FILE_NAME);
    let tmp = path.with_file_name(format!("{file_name}.part"));
    if tmp.exists() {
        remove_file_logged(&tmp, "window geometry eski .part silinemedi");
    }
    fs::write(&tmp, bytes).map_err(|err| {
        format!(
            "window geometry geçici yazılamadı ({}): {err}",
            tmp.display()
        )
    })?;
    if let Err(rename_err) = fs::rename(&tmp, path) {
        remove_file_logged(&tmp, "window geometry .part temizlenemedi");
        return Err(format!(
            "window geometry atomik yazılamadı ({}): {rename_err}",
            path.display()
        ));
    }
    Ok(())
}

impl GeometryStore {
    pub fn set(&mut self, window_label: &str, monitor_key: String, geom: SavedGeometry) {
        let record = self.windows.entry(window_label.to_string()).or_default();
        record.last_monitor = Some(monitor_key.clone());
        record.monitors.insert(monitor_key, geom);
    }

    fn merge_cache(&mut self, cache: &BTreeMap<String, CacheEntry>) {
        for (label, entry) in cache {
            self.set(label, entry.monitor_key.clone(), entry.geometry);
        }
    }
}

impl GeometrySession {
    pub fn new(data_root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            data_root,
            cache: Mutex::new(BTreeMap::new()),
            generation: AtomicU64::new(0),
        })
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    fn store_path(&self) -> PathBuf {
        geometry_store_path(&self.data_root)
    }

    /// Update in-memory cache for a label (no disk I/O).
    pub fn cache_entry(&self, window_label: &str, monitor_key: String, geometry: SavedGeometry) {
        match self.cache.lock() {
            Ok(mut guard) => {
                guard.insert(
                    window_label.to_string(),
                    CacheEntry {
                        monitor_key,
                        geometry,
                    },
                );
            }
            Err(err) => log::warn!("window geometry cache kilitlenemedi: {err}"),
        }
    }

    /// Capture from a live window into the cache when persist is allowed.
    pub fn capture_window<R: Runtime>(&self, app: &AppHandle<R>, window_label: &str) {
        let Some(window) = app.get_webview_window(window_label) else {
            return;
        };
        if !window_allows_persist(&window) {
            return;
        }
        let (key, geom) = match read_window_saved_geometry(app, &window) {
            Ok(v) => v,
            Err(err) => {
                log::warn!("window geometry okunamadı ({window_label}): {err}");
                return;
            }
        };
        self.cache_entry(window_label, key, geom);
    }

    /// Schedule a debounced flush of the in-memory cache.
    pub fn schedule_debounce(self: &Arc<Self>) {
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let session = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(PERSIST_DEBOUNCE).await;
            if session.generation.load(Ordering::SeqCst) == gen {
                session.flush();
            }
        });
    }

    /// Capture + debounce (Moved / Resized).
    pub fn on_moved_or_resized(self: &Arc<Self>, app: &AppHandle, window_label: &str) {
        self.capture_window(app, window_label);
        self.schedule_debounce();
    }

    /// Write cache → disk immediately (CloseRequested / Exit).
    pub fn flush(&self) {
        let cache = match self.cache.lock() {
            Ok(guard) => guard.clone(),
            Err(err) => {
                log::warn!("window geometry cache kilitlenemedi (flush): {err}");
                return;
            }
        };
        if cache.is_empty() {
            return;
        }
        let path = self.store_path();
        let mut store = load_geometry_store_or_default(&path);
        store.merge_cache(&cache);
        if let Err(err) = save_geometry_store(&path, &store) {
            log::warn!("{err}");
        }
    }

    /// Capture current windows then flush (best effort before destroy).
    pub fn flush_labels<R: Runtime>(&self, app: &AppHandle<R>, labels: &[&str]) {
        for label in labels {
            self.capture_window(app, label);
        }
        self.flush();
    }
}

fn window_allows_persist<R: Runtime>(window: &WebviewWindow<R>) -> bool {
    let minimized = window.is_minimized().unwrap_or(false);
    let maximized = window.is_maximized().unwrap_or(false);
    let fullscreen = window.is_fullscreen().unwrap_or(false);
    persist_allowed(minimized, maximized, fullscreen)
}

/// Read outer physical position + logical inner size + monitor key.
pub fn read_window_saved_geometry<R: Runtime>(
    app: &AppHandle<R>,
    window: &WebviewWindow<R>,
) -> Result<(String, SavedGeometry), String> {
    let scale = sanitize_scale(
        window
            .scale_factor()
            .map_err(|err| format!("scale_factor: {err}"))?,
    );
    let pos = window
        .outer_position()
        .map_err(|err| format!("outer_position: {err}"))?;
    let size = window
        .inner_size()
        .map_err(|err| format!("inner_size: {err}"))?;
    let logical = physical_to_logical_rect(0, 0, size.width, size.height, scale);
    let geom = SavedGeometry {
        x_phys: pos.x,
        y_phys: pos.y,
        width: logical.width,
        height: logical.height,
    };
    let monitor = match window.current_monitor() {
        Ok(Some(m)) => snapshot_from_monitor(&m),
        Ok(None) => primary_monitor_snapshot(app).ok_or_else(|| "monitör yok".to_string())?,
        Err(err) => return Err(format!("current_monitor: {err}")),
    };
    Ok((monitor.key(), geom))
}

/// Best-effort primary (or first available) monitor snapshot.
pub fn primary_monitor_snapshot<R: Runtime>(app: &AppHandle<R>) -> Option<MonitorSnapshot> {
    match app.primary_monitor() {
        Ok(Some(m)) => return Some(snapshot_from_monitor(&m)),
        Ok(None) => {}
        Err(err) => log::warn!("primary_monitor okunamadı: {err}"),
    }
    match app.available_monitors() {
        Ok(list) => list.first().map(snapshot_from_monitor),
        Err(err) => {
            log::warn!("available_monitors okunamadı: {err}");
            None
        }
    }
}

pub fn available_monitor_snapshots<R: Runtime>(app: &AppHandle<R>) -> Vec<MonitorSnapshot> {
    match app.available_monitors() {
        Ok(list) => list.iter().map(snapshot_from_monitor).collect(),
        Err(err) => {
            log::warn!("available_monitors okunamadı: {err}");
            Vec::new()
        }
    }
}

/// Load store + resolve placement (last monitor → primary 90% → centred fallback).
pub fn placement_for_window<R: Runtime>(
    app: &AppHandle<R>,
    window_label: &str,
    data_root: &Path,
) -> OpenPlacement {
    let store = load_geometry_store_or_default(&geometry_store_path(data_root));
    let monitors = available_monitor_snapshots(app);
    let primary = primary_monitor_snapshot(app).or_else(|| monitors.first().cloned());
    match primary {
        Some(primary) => {
            let list = if monitors.is_empty() {
                vec![primary.clone()]
            } else {
                monitors
            };
            resolve_open_placement(window_label, &list, &primary, &store)
        }
        None => {
            log::warn!(
                "monitör algılanamadı; varsayılan {FALLBACK_WIDTH}x{FALLBACK_HEIGHT} (center)"
            );
            OpenPlacement {
                geometry: SavedGeometry {
                    x_phys: 0,
                    y_phys: 0,
                    width: FALLBACK_WIDTH,
                    height: FALLBACK_HEIGHT,
                },
                center: true,
                min_width: MIN_WIDTH_LOGICAL,
                min_height: MIN_HEIGHT_LOGICAL,
            }
        }
    }
}

/// Apply physical position on a hidden window, then show (mixed-DPI safe).
pub fn apply_placement_show<R: Runtime>(
    window: &WebviewWindow<R>,
    placement: &OpenPlacement,
) -> Result<(), String> {
    if !placement.center {
        window
            .set_position(PhysicalPosition::new(
                placement.geometry.x_phys,
                placement.geometry.y_phys,
            ))
            .map_err(|err| format!("set_position: {err}"))?;
    }
    window.show().map_err(|err| format!("window show: {err}"))?;
    Ok(())
}

/// Handle Moved/Resized for a known geometry window label.
pub fn handle_window_event(app: &AppHandle, label: &str, event: &WindowEvent) {
    let Some(session) = app.try_state::<Arc<GeometrySession>>() else {
        return;
    };
    match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
            session.on_moved_or_resized(app, label);
        }
        WindowEvent::CloseRequested { .. } => {
            session.flush_labels(app, &[label]);
        }
        _ => {}
    }
}

/// Flush all cached geometry (Cmd+Q / Exit / ExitRequested).
pub fn flush_session(app: &AppHandle) {
    if let Some(session) = app.try_state::<Arc<GeometrySession>>() {
        for label in [MAIN_WINDOW_LABEL, super::graph_ui::GRAPH_WINDOW_LABEL] {
            session.capture_window(app, label);
        }
        session.flush();
    }
}

/// Persist one window into the session cache + immediate flush (graph recreate).
pub fn persist_window_geometry<R: Runtime>(
    app: &AppHandle<R>,
    window: &WebviewWindow<R>,
    window_label: &str,
    session: Option<&GeometrySession>,
) {
    if !window_allows_persist(window) {
        return;
    }
    let (key, geom) = match read_window_saved_geometry(app, window) {
        Ok(v) => v,
        Err(err) => {
            log::warn!("window geometry okunamadı ({window_label}): {err}");
            return;
        }
    };
    if let Some(session) = session {
        session.cache_entry(window_label, key, geom);
        session.flush();
        return;
    }
    // Fallback when session is not managed (should be rare).
    let root = crate::services::data_root();
    let path = geometry_store_path(&root);
    let mut store = load_geometry_store_or_default(&path);
    store.set(window_label, key, geom);
    if let Err(err) = save_geometry_store(&path, &store) {
        log::warn!("{err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Owner display 1: 14" MacBook Retina — physical work area @2×.
    fn macbook_retina() -> MonitorSnapshot {
        MonitorSnapshot {
            name: "Built-in Retina Display".into(),
            scale_factor: 2.0,
            physical_width: 3024,
            physical_height: 1964,
            work_area_phys: PhysRect {
                x: 0,
                y: 66,
                width: 3024,
                height: 1898,
            },
        }
    }

    /// Owner display 2: 32" 1920×1080 landscape @1× (alone).
    fn desktop_1080p() -> MonitorSnapshot {
        MonitorSnapshot {
            name: "Dell S3221QS".into(),
            scale_factor: 1.0,
            physical_width: 1920,
            physical_height: 1080,
            work_area_phys: PhysRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1040,
            },
        }
    }

    /// Same 1080p placed to the right of the MacBook (mixed-DPI desktop).
    fn desktop_1080p_right_of_macbook() -> MonitorSnapshot {
        let mut m = desktop_1080p();
        m.work_area_phys.x = 3024;
        m
    }

    /// Owner display 3: 27" portrait 1080×1920.
    fn portrait_1080() -> MonitorSnapshot {
        MonitorSnapshot {
            name: "Portrait-27".into(),
            scale_factor: 1.0,
            physical_width: 1080,
            physical_height: 1920,
            work_area_phys: PhysRect {
                x: 0,
                y: 0,
                width: 1080,
                height: 1880,
            },
        }
    }

    fn temp_path(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("lounge-win-geom-{tag}-{nanos}.json"))
    }

    #[test]
    fn monitor_key_includes_name_and_physical_resolution() {
        assert_eq!(
            monitor_key("Built-in Retina Display", 3024, 1964),
            "Built-in Retina Display|3024x1964"
        );
        assert_eq!(monitor_key("  ", 1920, 1080), "unknown|1920x1080");
        assert_eq!(monitor_key("", 1080, 1920), "unknown|1080x1920");
    }

    #[test]
    fn physical_to_logical_rect_retina_2x() {
        let logical = physical_to_logical_rect(0, 66, 3024, 1898, 2.0);
        assert!((logical.x - 0.0).abs() < f64::EPSILON);
        assert!((logical.y - 33.0).abs() < f64::EPSILON);
        assert!((logical.width - 1512.0).abs() < f64::EPSILON);
        assert!((logical.height - 949.0).abs() < f64::EPSILON);
    }

    #[test]
    fn physical_to_logical_rect_scale_1_25() {
        // 2560×1392 @ 1.25 → 2048×1113.6
        let logical = physical_to_logical_rect(0, 0, 2560, 1392, 1.25);
        assert!((logical.width - 2048.0).abs() < f64::EPSILON);
        assert!((logical.height - 1113.6).abs() < 1e-9);
        let (w, h) = logical_to_physical_size(logical.width, logical.height, 1.25);
        assert_eq!(w, 2560);
        assert_eq!(h, 1392);
    }

    #[test]
    fn physical_to_logical_rect_scale_1_5() {
        let logical = physical_to_logical_rect(10, 20, 1920, 1080, 1.5);
        assert!((logical.x - (10.0 / 1.5)).abs() < 1e-9);
        assert!((logical.y - (20.0 / 1.5)).abs() < 1e-9);
        assert!((logical.width - 1280.0).abs() < f64::EPSILON);
        assert!((logical.height - 720.0).abs() < f64::EPSILON);
    }

    #[test]
    fn physical_to_logical_rect_fractional_scale() {
        let scale = 1.333_333_333_f64;
        let logical = physical_to_logical_rect(0, 0, 2560, 1440, scale);
        assert!((logical.width - (2560.0 / scale)).abs() < 1e-6);
        assert!((logical.height - (1440.0 / scale)).abs() < 1e-6);
    }

    #[test]
    fn work_area_90_percent_centred_macbook_retina() {
        let m = macbook_retina();
        let g = default_centered_geometry(&m, FILL_RATIO);
        let work_log = m.work_area_logical();
        assert!((g.width - work_log.width * 0.9).abs() < 1.0);
        assert!((g.height - work_log.height * 0.9).abs() < 1.0);
        assert!(g.width >= MIN_WIDTH_LOGICAL.min(work_log.width));
        assert!(geometry_restorable(&g, &m));
        let (w_phys, h_phys) = logical_to_physical_size(g.width, g.height, m.scale_factor);
        let cx = g.x_phys + w_phys as i32 / 2;
        let cy = g.y_phys + h_phys as i32 / 2;
        let work = m.work_area_phys;
        assert!((cx - (work.x + work.width as i32 / 2)).abs() <= 1);
        assert!((cy - (work.y + work.height as i32 / 2)).abs() <= 1);
    }

    #[test]
    fn work_area_90_percent_centred_1080p() {
        let m = desktop_1080p();
        let g = default_centered_geometry(&m, FILL_RATIO);
        assert!((g.width - 1728.0).abs() < 1.0);
        assert!((g.height - 936.0).abs() < 1.0);
        assert!(geometry_restorable(&g, &m));
        assert!(g.width > 1280.0);
    }

    #[test]
    fn portrait_1080_respects_min_width_and_fills() {
        let m = portrait_1080();
        let g = default_centered_geometry(&m, FILL_RATIO);
        assert!((g.width - 972.0).abs() < 1.0);
        assert!(g.width >= MIN_WIDTH_LOGICAL);
        assert!(geometry_restorable(&g, &m));
    }

    #[test]
    fn effective_min_softens_when_work_area_smaller() {
        let m = MonitorSnapshot {
            name: "tiny".into(),
            scale_factor: 1.0,
            physical_width: 800,
            physical_height: 500,
            work_area_phys: PhysRect {
                x: 0,
                y: 0,
                width: 800,
                height: 500,
            },
        };
        let (mw, mh) = effective_min_size(&m.work_area_logical());
        assert!((mw - 800.0).abs() < f64::EPSILON);
        assert!((mh - 500.0).abs() < f64::EPSILON);
        let g = default_centered_geometry(&m, FILL_RATIO);
        assert!((g.width - 800.0).abs() < 0.5);
        assert!((g.height - 500.0).abs() < 0.5);
    }

    #[test]
    fn aero_snap_negative_x_restores_clamped() {
        let m = desktop_1080p();
        let saved = SavedGeometry {
            x_phys: -7,
            y_phys: 0,
            width: 960.0,
            height: 1040.0,
        };
        assert!(
            geometry_restorable(&saved, &m),
            "x=-7 must still overlap the work area"
        );
        let clamped = clamp_geometry_to_work_area(saved, &m);
        assert_eq!(clamped.x_phys, 0);
        assert_eq!(clamped.y_phys, 0);
    }

    #[test]
    fn off_screen_saved_geometry_falls_back_to_90_percent() {
        let primary = desktop_1080p();
        let mut store = GeometryStore::default();
        store.set(
            MAIN_WINDOW_LABEL,
            primary.key(),
            SavedGeometry {
                x_phys: 5000,
                y_phys: 5000,
                width: 1280.0,
                height: 800.0,
            },
        );
        let placement = resolve_open_placement(
            MAIN_WINDOW_LABEL,
            std::slice::from_ref(&primary),
            &primary,
            &store,
        );
        let expected = default_centered_geometry(&primary, FILL_RATIO);
        assert_eq!(placement.geometry, expected);
    }

    #[test]
    fn two_monitor_restores_on_secondary_falls_back_when_absent() {
        let primary = macbook_retina();
        let secondary = desktop_1080p_right_of_macbook();
        assert_ne!(primary.key(), secondary.key());

        let saved = SavedGeometry {
            x_phys: secondary.work_area_phys.x + 40,
            y_phys: 80,
            width: 1600.0,
            height: 900.0,
        };
        assert!(geometry_restorable(&saved, &secondary));

        let mut store = GeometryStore::default();
        store.set(MAIN_WINDOW_LABEL, secondary.key(), saved);

        let both = [primary.clone(), secondary.clone()];
        let on_secondary = resolve_open_placement(MAIN_WINDOW_LABEL, &both, &primary, &store);
        assert_eq!(on_secondary.geometry.x_phys, saved.x_phys);
        assert_eq!(on_secondary.geometry.y_phys, saved.y_phys);
        assert!((on_secondary.geometry.width - 1600.0).abs() < f64::EPSILON);

        // Secondary unplugged → 90% on primary.
        let only_primary = resolve_open_placement(
            MAIN_WINDOW_LABEL,
            std::slice::from_ref(&primary),
            &primary,
            &store,
        );
        let expected = default_centered_geometry(&primary, FILL_RATIO);
        assert_eq!(only_primary.geometry, expected);
    }

    #[test]
    fn persist_allowed_rejects_min_max_fullscreen() {
        assert!(persist_allowed(false, false, false));
        assert!(!persist_allowed(true, false, false));
        assert!(!persist_allowed(false, true, false));
        assert!(!persist_allowed(false, false, true));
        assert!(!persist_allowed(true, true, true));
    }

    #[test]
    fn graph_and_main_geometry_are_independent() {
        let m = desktop_1080p();
        let mut store = GeometryStore::default();
        store.set(
            MAIN_WINDOW_LABEL,
            m.key(),
            SavedGeometry {
                x_phys: 10,
                y_phys: 10,
                width: 1400.0,
                height: 900.0,
            },
        );
        store.set(
            "graph-window",
            m.key(),
            SavedGeometry {
                x_phys: 50,
                y_phys: 60,
                width: 1500.0,
                height: 950.0,
            },
        );
        let main = resolve_open_placement(MAIN_WINDOW_LABEL, std::slice::from_ref(&m), &m, &store);
        let graph = resolve_open_placement("graph-window", std::slice::from_ref(&m), &m, &store);
        assert!((main.geometry.width - 1400.0).abs() < f64::EPSILON);
        assert!((graph.geometry.width - 1500.0).abs() < f64::EPSILON);
        assert_eq!(graph.geometry.x_phys, 50);
    }

    #[test]
    fn atomic_save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "lounge-geom-dir-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = geometry_store_path(&dir);
        let mut store = GeometryStore::default();
        store.set(
            MAIN_WINDOW_LABEL,
            "A|1920x1080".into(),
            SavedGeometry {
                x_phys: 1,
                y_phys: 2,
                width: 3.0,
                height: 4.0,
            },
        );
        save_geometry_store(&path, &store).unwrap();
        assert!(path.is_file());
        let part = path.with_file_name(format!("{GEOMETRY_FILE_NAME}.part"));
        assert!(!part.exists(), "atomic write must remove the .part file");
        let loaded = load_geometry_store(&path).unwrap();
        assert_eq!(loaded, store);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_yields_empty_store() {
        let path = temp_path("missing");
        let _ = fs::remove_file(&path);
        let store = load_geometry_store(&path).unwrap();
        assert!(store.windows.is_empty());
    }

    #[test]
    fn load_corrupt_json_quarantines_to_corrupt_extension() {
        let path = temp_path("corrupt");
        fs::write(&path, "{not-json").unwrap();
        let err = load_geometry_store(&path).unwrap_err();
        assert!(err.contains("JSON bozuk"));
        // load_geometry_store_or_default path:
        let _ = load_geometry_store_or_default(&path);
        let corrupt = path.with_extension("json.corrupt");
        assert!(
            corrupt.is_file() || !path.exists(),
            "corrupt file should be renamed or removed"
        );
        let _ = fs::remove_file(&corrupt);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn sanitize_scale_rejects_non_finite() {
        assert_eq!(sanitize_scale(2.0), 2.0);
        assert_eq!(sanitize_scale(0.0), 1.0);
        assert_eq!(sanitize_scale(-1.0), 1.0);
        assert_eq!(sanitize_scale(f64::NAN), 1.0);
    }

    #[test]
    fn fixtures_cover_owner_three_displays() {
        let keys = [
            macbook_retina().key(),
            desktop_1080p().key(),
            portrait_1080().key(),
        ];
        assert_eq!(keys[0], "Built-in Retina Display|3024x1964");
        assert_eq!(keys[1], "Dell S3221QS|1920x1080");
        assert_eq!(keys[2], "Portrait-27|1080x1920");
        for snap in [macbook_retina(), desktop_1080p(), portrait_1080()] {
            let g = default_centered_geometry(&snap, FILL_RATIO);
            assert!(geometry_restorable(&g, &snap), "fail on {}", snap.key());
        }
    }

    #[test]
    fn session_cache_flush_writes_last_monitor() {
        let dir = std::env::temp_dir().join(format!(
            "lounge-geom-session-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let session = GeometrySession::new(dir.clone());
        session.cache_entry(
            MAIN_WINDOW_LABEL,
            "Dell|1920x1080".into(),
            SavedGeometry {
                x_phys: 10,
                y_phys: 20,
                width: 1000.0,
                height: 700.0,
            },
        );
        session.flush();
        let loaded = load_geometry_store(&geometry_store_path(&dir)).unwrap();
        let rec = loaded.windows.get(MAIN_WINDOW_LABEL).unwrap();
        assert_eq!(rec.last_monitor.as_deref(), Some("Dell|1920x1080"));
        let g = rec.monitors.get("Dell|1920x1080").unwrap();
        assert_eq!(g.x_phys, 10);
        let _ = fs::remove_dir_all(&dir);
    }
}
