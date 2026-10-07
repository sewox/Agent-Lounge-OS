//! Monitor-aware window sizing and per-monitor geometry persistence.
//!
//! Pure geometry helpers are display-free and unit-tested. Tauri wiring lives
//! in the same module so main + graph windows share one code path.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Monitor, Runtime, WebviewWindow};

/// Matches `tauri.conf.json` main window label.
pub const MAIN_WINDOW_LABEL: &str = "main";

/// Persistence file under the app data / data-root directory.
pub const GEOMETRY_FILE_NAME: &str = "window_geometry.json";

/// First-open fill of the monitor work area (not maximized).
pub const FILL_RATIO: f64 = 0.90;

/// Minimum logical width — fits portrait 1080-wide work areas.
pub const MIN_WIDTH_LOGICAL: f64 = 960.0;

/// Minimum logical height.
pub const MIN_HEIGHT_LOGICAL: f64 = 640.0;

/// Fallback when no monitor can be detected (matches historical defaults).
pub const FALLBACK_WIDTH: f64 = 1280.0;
pub const FALLBACK_HEIGHT: f64 = 800.0;

/// Logical-pixel rectangle (CSS / Tauri window units).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Minimum inner size in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinSize {
    pub width: f64,
    pub height: f64,
}

impl Default for MinSize {
    fn default() -> Self {
        Self {
            width: MIN_WIDTH_LOGICAL,
            height: MIN_HEIGHT_LOGICAL,
        }
    }
}

/// Display-free monitor snapshot used by pure geometry functions.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorSnapshot {
    pub name: String,
    pub scale_factor: f64,
    /// Full monitor resolution in **physical** pixels (persistence key).
    pub physical_width: u32,
    pub physical_height: u32,
    /// Usable area (excludes menu bar / dock / taskbar) in **logical** pixels.
    pub work_area: LogicalRect,
}

/// Resolved placement to apply when opening a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowPlacement {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<LogicalRect> for WindowPlacement {
    fn from(r: LogicalRect) -> Self {
        Self {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }
    }
}

/// Per-window, per-monitor saved geometries.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct GeometryStore {
    /// `window_label` → (`monitor_key` → rect)
    #[serde(flatten)]
    pub by_label: BTreeMap<String, BTreeMap<String, LogicalRect>>,
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
}

/// Convert a Tauri monitor into a logical work-area snapshot (HiDPI-safe).
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
        work_area: LogicalRect {
            x: work.position.x as f64 / scale,
            y: work.position.y as f64 / scale,
            width: work.size.width as f64 / scale,
            height: work.size.height as f64 / scale,
        },
    }
}

fn sanitize_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// 90% of the work area, centred; clamped to min size without exceeding the area.
pub fn default_centered_geometry(
    work_area: &LogicalRect,
    ratio: f64,
    min: &MinSize,
) -> LogicalRect {
    let ratio = if ratio.is_finite() && ratio > 0.0 && ratio <= 1.0 {
        ratio
    } else {
        FILL_RATIO
    };
    let width = clamp_size(work_area.width * ratio, min.width, work_area.width);
    let height = clamp_size(work_area.height * ratio, min.height, work_area.height);
    let x = work_area.x + ((work_area.width - width) / 2.0).round();
    let y = work_area.y + ((work_area.height - height) / 2.0).round();
    LogicalRect {
        x,
        y,
        width: width.round().max(1.0),
        height: height.round().max(1.0),
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

/// True when `geom` lies entirely inside `work_area` (1px float slack).
pub fn geometry_fits_work_area(geom: &LogicalRect, work_area: &LogicalRect) -> bool {
    const EPS: f64 = 1.0;
    if !geom.width.is_finite()
        || !geom.height.is_finite()
        || geom.width < 1.0
        || geom.height < 1.0
        || !geom.x.is_finite()
        || !geom.y.is_finite()
    {
        return false;
    }
    geom.x + EPS >= work_area.x
        && geom.y + EPS >= work_area.y
        && geom.x + geom.width <= work_area.x + work_area.width + EPS
        && geom.y + geom.height <= work_area.y + work_area.height + EPS
}

/// Restore saved geometry when it still fits; otherwise 90% centred fallback.
pub fn resolve_window_geometry(
    saved: Option<&LogicalRect>,
    work_area: &LogicalRect,
    min: &MinSize,
    ratio: f64,
) -> LogicalRect {
    if let Some(g) = saved {
        if geometry_fits_work_area(g, work_area) {
            let width = clamp_size(g.width, min.width, work_area.width);
            let height = clamp_size(g.height, min.height, work_area.height);
            // Keep origin; if clamp shrunk the size, keep top-left (still on-screen).
            return LogicalRect {
                x: g.x,
                y: g.y,
                width: width.round().max(1.0),
                height: height.round().max(1.0),
            };
        }
    }
    default_centered_geometry(work_area, ratio, min)
}

/// Path to the geometry JSON under `data_root`.
pub fn geometry_store_path(data_root: &Path) -> PathBuf {
    data_root.join(GEOMETRY_FILE_NAME)
}

/// Load store from disk. Missing file → empty store. Parse/IO errors are returned
/// so callers can `log::warn!` (never swallow).
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

/// Atomically write the store (`*.json.part` → rename). Errors are returned for logging.
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
        let _ = fs::remove_file(&tmp);
    }
    fs::write(&tmp, bytes).map_err(|err| {
        format!(
            "window geometry geçici yazılamadı ({}): {err}",
            tmp.display()
        )
    })?;
    if let Err(rename_err) = fs::rename(&tmp, path) {
        // Windows may refuse rename-over-existing; copy then remove.
        if let Err(copy_err) = fs::copy(&tmp, path) {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "window geometry atomik yazılamadı ({}): rename={rename_err}; copy={copy_err}",
                path.display()
            ));
        }
        let _ = fs::remove_file(&tmp);
    }
    Ok(())
}

impl GeometryStore {
    pub fn get(&self, window_label: &str, key: &str) -> Option<&LogicalRect> {
        self.by_label.get(window_label)?.get(key)
    }

    pub fn set(&mut self, window_label: &str, key: String, rect: LogicalRect) {
        self.by_label
            .entry(window_label.to_string())
            .or_default()
            .insert(key, rect);
    }
}

/// Resolve placement for `window_label` on `monitor`, using optional disk store.
pub fn resolve_placement(
    window_label: &str,
    monitor: &MonitorSnapshot,
    store: &GeometryStore,
    min: &MinSize,
) -> WindowPlacement {
    let key = monitor.key();
    let saved = store.get(window_label, &key);
    resolve_window_geometry(saved, &monitor.work_area, min, FILL_RATIO).into()
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

/// Load store + resolve placement; falls back to 1280×800 when no monitor.
pub fn placement_for_window<R: Runtime>(
    app: &AppHandle<R>,
    window_label: &str,
    data_root: &Path,
) -> WindowPlacement {
    let store = match load_geometry_store(&geometry_store_path(data_root)) {
        Ok(s) => s,
        Err(err) => {
            log::warn!("{err}");
            GeometryStore::default()
        }
    };
    let min = MinSize::default();
    match primary_monitor_snapshot(app) {
        Some(monitor) => resolve_placement(window_label, &monitor, &store, &min),
        None => {
            log::warn!("monitör algılanamadı; varsayılan {FALLBACK_WIDTH}x{FALLBACK_HEIGHT}");
            WindowPlacement {
                x: 0.0,
                y: 0.0,
                width: FALLBACK_WIDTH,
                height: FALLBACK_HEIGHT,
            }
        }
    }
}

/// Read current window geometry in logical pixels (outer position + inner size).
pub fn read_window_logical_rect<R: Runtime>(
    window: &WebviewWindow<R>,
) -> Result<LogicalRect, String> {
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
    Ok(LogicalRect {
        x: pos.x as f64 / scale,
        y: pos.y as f64 / scale,
        width: size.width as f64 / scale,
        height: size.height as f64 / scale,
    })
}

/// Persist geometry for the window's current monitor. Errors → `log::warn!`.
pub fn persist_window_geometry<R: Runtime>(
    app: &AppHandle<R>,
    window: &WebviewWindow<R>,
    window_label: &str,
    data_root: &Path,
) {
    let rect = match read_window_logical_rect(window) {
        Ok(r) => r,
        Err(err) => {
            log::warn!("window geometry okunamadı ({window_label}): {err}");
            return;
        }
    };
    let monitor = match window.current_monitor() {
        Ok(Some(m)) => snapshot_from_monitor(&m),
        Ok(None) => match primary_monitor_snapshot(app) {
            Some(m) => m,
            None => {
                log::warn!("window geometry kaydı atlandı ({window_label}): monitör yok");
                return;
            }
        },
        Err(err) => {
            log::warn!("current_monitor okunamadı ({window_label}): {err}");
            return;
        }
    };
    let path = geometry_store_path(data_root);
    let mut store = match load_geometry_store(&path) {
        Ok(s) => s,
        Err(err) => {
            log::warn!("{err}");
            GeometryStore::default()
        }
    };
    store.set(window_label, monitor.key(), rect);
    if let Err(err) = save_geometry_store(&path, &store) {
        log::warn!("{err}");
    }
}

/// Persist by label if the window still exists.
pub fn persist_window_label<R: Runtime>(app: &AppHandle<R>, window_label: &str, data_root: &Path) {
    if let Some(window) = app.get_webview_window(window_label) {
        persist_window_geometry(app, &window, window_label, data_root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn min() -> MinSize {
        MinSize::default()
    }

    /// Owner display 1: 14" MacBook Retina ~1512×982 logical work area.
    fn macbook_retina_logical() -> MonitorSnapshot {
        MonitorSnapshot {
            name: "Built-in Retina Display".into(),
            scale_factor: 2.0,
            physical_width: 3024,
            physical_height: 1964,
            work_area: LogicalRect {
                x: 0.0,
                y: 33.0,
                width: 1512.0,
                height: 949.0,
            },
        }
    }

    /// Owner display 2: 32" 1920×1080 landscape.
    fn desktop_1080p() -> MonitorSnapshot {
        MonitorSnapshot {
            name: "Dell S3221QS".into(),
            scale_factor: 1.0,
            physical_width: 1920,
            physical_height: 1080,
            work_area: LogicalRect {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1040.0,
            },
        }
    }

    /// Owner display 3: 27" portrait 1080×1920.
    fn portrait_1080() -> MonitorSnapshot {
        MonitorSnapshot {
            name: "Portrait-27".into(),
            scale_factor: 1.0,
            physical_width: 1080,
            physical_height: 1920,
            work_area: LogicalRect {
                x: 0.0,
                y: 0.0,
                width: 1080.0,
                height: 1880.0,
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
    fn work_area_90_percent_centred_macbook_retina() {
        let m = macbook_retina_logical();
        let g = default_centered_geometry(&m.work_area, FILL_RATIO, &min());
        assert!((g.width - (1512.0 * 0.9)).abs() < 1.0);
        assert!((g.height - (949.0 * 0.9)).abs() < 1.0);
        assert!(g.width >= MIN_WIDTH_LOGICAL);
        // Centred within work area.
        let cx = g.x + g.width / 2.0;
        let cy = g.y + g.height / 2.0;
        assert!((cx - (m.work_area.x + m.work_area.width / 2.0)).abs() < 1.5);
        assert!((cy - (m.work_area.y + m.work_area.height / 2.0)).abs() < 1.5);
        assert!(geometry_fits_work_area(&g, &m.work_area));
    }

    #[test]
    fn work_area_90_percent_centred_1080p() {
        let m = desktop_1080p();
        let g = default_centered_geometry(&m.work_area, FILL_RATIO, &min());
        assert!((g.width - 1728.0).abs() < 1.0);
        assert!((g.height - 936.0).abs() < 1.0);
        assert!(geometry_fits_work_area(&g, &m.work_area));
        assert!(
            g.width > 1280.0,
            "must fill more than historical 1280 default"
        );
    }

    #[test]
    fn portrait_1080_respects_min_width_and_fills() {
        let m = portrait_1080();
        let g = default_centered_geometry(&m.work_area, FILL_RATIO, &min());
        // 90% of 1080 = 972 ≥ min 960
        assert!((g.width - 972.0).abs() < 1.0);
        assert!(g.width >= MIN_WIDTH_LOGICAL);
        assert!(g.width <= m.work_area.width);
        assert!(g.height > 800.0);
        assert!(geometry_fits_work_area(&g, &m.work_area));
    }

    #[test]
    fn min_clamp_raises_undersized_90_percent_without_overflow() {
        // Narrow work area: 90% would be 900; clamp up to min 960.
        let work = LogicalRect {
            x: 10.0,
            y: 20.0,
            width: 1000.0,
            height: 800.0,
        };
        let g = default_centered_geometry(&work, FILL_RATIO, &min());
        assert!((g.width - 960.0).abs() < 0.5);
        assert!(g.width <= work.width);
        assert!(geometry_fits_work_area(&g, &work));
    }

    #[test]
    fn min_clamp_softens_when_work_area_smaller_than_min() {
        let work = LogicalRect {
            x: 0.0,
            y: 0.0,
            width: 800.0,
            height: 500.0,
        };
        let g = default_centered_geometry(&work, FILL_RATIO, &min());
        assert!((g.width - 800.0).abs() < 0.5);
        assert!((g.height - 500.0).abs() < 0.5);
        assert!(geometry_fits_work_area(&g, &work));
    }

    #[test]
    fn hidpi_physical_to_logical_work_area_math() {
        // Physical work area 3024×1898 @ 2× → logical 1512×949.
        let scale = 2.0_f64;
        let phys_w = 3024_u32;
        let phys_h = 1898_u32;
        let phys_x = 0_i32;
        let phys_y = 66_i32;
        let logical = LogicalRect {
            x: phys_x as f64 / scale,
            y: phys_y as f64 / scale,
            width: phys_w as f64 / scale,
            height: phys_h as f64 / scale,
        };
        assert!((logical.x - 0.0).abs() < f64::EPSILON);
        assert!((logical.y - 33.0).abs() < f64::EPSILON);
        assert!((logical.width - 1512.0).abs() < f64::EPSILON);
        assert!((logical.height - 949.0).abs() < f64::EPSILON);
        let g = default_centered_geometry(&logical, FILL_RATIO, &min());
        assert!(geometry_fits_work_area(&g, &logical));
    }

    #[test]
    fn off_screen_saved_geometry_falls_back_to_90_percent() {
        let m = desktop_1080p();
        let off = LogicalRect {
            x: 5000.0,
            y: 5000.0,
            width: 1280.0,
            height: 800.0,
        };
        assert!(!geometry_fits_work_area(&off, &m.work_area));
        let resolved = resolve_window_geometry(Some(&off), &m.work_area, &min(), FILL_RATIO);
        let expected = default_centered_geometry(&m.work_area, FILL_RATIO, &min());
        assert_eq!(resolved, expected);
    }

    #[test]
    fn partially_off_screen_falls_back() {
        let m = portrait_1080();
        let partial = LogicalRect {
            x: 200.0,
            y: -100.0,
            width: 900.0,
            height: 700.0,
        };
        assert!(!geometry_fits_work_area(&partial, &m.work_area));
        let resolved = resolve_window_geometry(Some(&partial), &m.work_area, &min(), FILL_RATIO);
        assert!(geometry_fits_work_area(&resolved, &m.work_area));
    }

    #[test]
    fn restore_saved_geometry_when_on_same_monitor() {
        let m = macbook_retina_logical();
        let saved = LogicalRect {
            x: 40.0,
            y: 50.0,
            width: 1200.0,
            height: 800.0,
        };
        assert!(geometry_fits_work_area(&saved, &m.work_area));
        let mut store = GeometryStore::default();
        store.set(MAIN_WINDOW_LABEL, m.key(), saved);
        let placement = resolve_placement(MAIN_WINDOW_LABEL, &m, &store, &min());
        assert!((placement.x - 40.0).abs() < f64::EPSILON);
        assert!((placement.y - 50.0).abs() < f64::EPSILON);
        assert!((placement.width - 1200.0).abs() < f64::EPSILON);
        assert!((placement.height - 800.0).abs() < f64::EPSILON);
    }

    #[test]
    fn per_monitor_keys_do_not_cross_restore() {
        let a = desktop_1080p();
        let b = portrait_1080();
        assert_ne!(a.key(), b.key());
        let mut store = GeometryStore::default();
        store.set(
            MAIN_WINDOW_LABEL,
            a.key(),
            LogicalRect {
                x: 100.0,
                y: 80.0,
                width: 1600.0,
                height: 900.0,
            },
        );
        // Opening on portrait must ignore landscape save → 90% fallback.
        let placement = resolve_placement(MAIN_WINDOW_LABEL, &b, &store, &min());
        let expected = default_centered_geometry(&b.work_area, FILL_RATIO, &min());
        assert_eq!(LogicalRect::from(placement), expected);
    }

    #[test]
    fn graph_and_main_geometry_are_independent_keys() {
        let m = desktop_1080p();
        let mut store = GeometryStore::default();
        store.set(
            MAIN_WINDOW_LABEL,
            m.key(),
            LogicalRect {
                x: 10.0,
                y: 10.0,
                width: 1400.0,
                height: 900.0,
            },
        );
        store.set(
            "graph-window",
            m.key(),
            LogicalRect {
                x: 50.0,
                y: 60.0,
                width: 1500.0,
                height: 950.0,
            },
        );
        let main = resolve_placement(MAIN_WINDOW_LABEL, &m, &store, &min());
        let graph = resolve_placement("graph-window", &m, &store, &min());
        assert!((main.width - 1400.0).abs() < f64::EPSILON);
        assert!((graph.width - 1500.0).abs() < f64::EPSILON);
        assert!((graph.x - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gone_monitor_key_miss_falls_back() {
        let current = desktop_1080p();
        let mut store = GeometryStore::default();
        store.set(
            MAIN_WINDOW_LABEL,
            "Old-Monitor|2560x1440".into(),
            LogicalRect {
                x: 0.0,
                y: 0.0,
                width: 2000.0,
                height: 1200.0,
            },
        );
        let placement = resolve_placement(MAIN_WINDOW_LABEL, &current, &store, &min());
        let expected = default_centered_geometry(&current.work_area, FILL_RATIO, &min());
        assert_eq!(LogicalRect::from(placement), expected);
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
            LogicalRect {
                x: 1.0,
                y: 2.0,
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
        assert!(store.by_label.is_empty());
    }

    #[test]
    fn load_corrupt_json_returns_error_not_empty() {
        let path = temp_path("corrupt");
        fs::write(&path, "{not-json").unwrap();
        let err = load_geometry_store(&path).unwrap_err();
        assert!(err.contains("JSON bozuk") || err.contains("window geometry"));
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
        // Ensures the three owner displays stay in the suite (macOS / Win / Linux CI).
        let keys = [
            macbook_retina_logical().key(),
            desktop_1080p().key(),
            portrait_1080().key(),
        ];
        assert_eq!(keys[0], "Built-in Retina Display|3024x1964");
        assert_eq!(keys[1], "Dell S3221QS|1920x1080");
        assert_eq!(keys[2], "Portrait-27|1080x1920");
        for snap in [macbook_retina_logical(), desktop_1080p(), portrait_1080()] {
            let g = default_centered_geometry(&snap.work_area, FILL_RATIO, &min());
            assert!(
                geometry_fits_work_area(&g, &snap.work_area),
                "fail on {}",
                snap.key()
            );
            assert!(
                g.width >= MIN_WIDTH_LOGICAL.min(snap.work_area.width),
                "min width on {}",
                snap.key()
            );
        }
    }

    impl From<WindowPlacement> for LogicalRect {
        fn from(p: WindowPlacement) -> Self {
            Self {
                x: p.x,
                y: p.y,
                width: p.width,
                height: p.height,
            }
        }
    }
}
