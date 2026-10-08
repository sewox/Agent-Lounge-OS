//! Shared codebase-memory-mcp UI `config.json` hygiene.
//!
//! Lounge spawns CBM with `--ui=true --port=N`, which persists into the shared
//! `${CBM_CACHE_DIR:-~/.cache/codebase-memory-mcp}/config.json`. That pollutes
//! every other CBM session on the machine. Isolation via a separate
//! `CBM_CACHE_DIR` is **not** viable: upstream docs say it also moves indexes
//! and `_config.db`, and "one account can use only one canonical cache root at
//! a time; a process configured with a different root fails while any CBM
//! session is active."
//!
//! Therefore Lounge uses snapshot/restore around its child, plus a one-time
//! migration that reverts Lounge band pollution (`ui_enabled=true` +
//! `ui_port` in 18749–18759).
//!
//! Restores are **compare-and-restore**: only when the live file still shows
//! exactly Lounge's footprint (`ui_enabled=true` + `ui_port=N`). After restore
//! (or skip), the snapshot is cleared so stop/exit cannot clobber later edits.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::memory_bridge::{GRAPH_UI_PORT_BAND_END, GRAPH_UI_PORT_BAND_START};

/// Lounge app-data key holding the pre-spawn snapshot JSON.
pub const SETTINGS_KEY_CBM_CONFIG_SNAPSHOT: &str = "graph_ui_cbm_config_snapshot";
/// Marker that the one-time pollution migration has been considered.
pub const SETTINGS_KEY_CBM_CONFIG_MIGRATED: &str = "graph_ui_cbm_config_migrated";

const CBM_DEFAULT_UI_PORT: u16 = 9749;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CbmUiConfigFile {
    #[serde(default)]
    pub ui_enabled: Option<bool>,
    #[serde(default)]
    pub ui_port: Option<u16>,
    /// Preserve unknown keys when rewriting.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CbmUiConfigSnapshot {
    /// Whether `config.json` existed before Lounge touched it.
    pub existed: bool,
    pub ui_enabled: Option<bool>,
    pub ui_port: Option<u16>,
    /// Full original JSON text when the file existed (best-effort restore).
    #[serde(default)]
    pub raw: Option<String>,
    /// Absolute path of `config.json` at snapshot time.
    #[serde(default)]
    pub path: Option<PathBuf>,
    /// Port Lounge passed via `--port=N` (footprint for guarded restore).
    #[serde(default)]
    pub lounge_port: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreOutcome {
    Restored,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationOutcome {
    pub changed: bool,
    pub backup_path: Option<PathBuf>,
    pub detail: String,
}

/// Pure cache-dir resolver (testable without mutating process env).
pub fn cbm_cache_dir_from(mut getenv: impl FnMut(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(dir) = getenv("CBM_CACHE_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    let home = getenv("HOME")
        .or_else(|| getenv("USERPROFILE"))
        .map(PathBuf::from)?;
    Some(home.join(".cache").join("codebase-memory-mcp"))
}

/// Resolve CBM UI config directory — mirrors upstream `cbm_resolve_cache_dir`
/// (v0.11.0): `CBM_CACHE_DIR` if set, else `$HOME/.cache/codebase-memory-mcp`
/// (`USERPROFILE` on Windows when `HOME` is unset). Not `%LOCALAPPDATA%`.
pub fn cbm_cache_dir() -> Option<PathBuf> {
    cbm_cache_dir_from(|k| std::env::var(k).ok())
}

pub fn cbm_ui_config_path() -> Option<PathBuf> {
    Some(cbm_cache_dir()?.join("config.json"))
}

pub fn port_in_lounge_band(port: u16) -> bool {
    (GRAPH_UI_PORT_BAND_START..=GRAPH_UI_PORT_BAND_END).contains(&port)
}

pub fn looks_like_lounge_pollution(cfg: &CbmUiConfigFile) -> bool {
    matches!(cfg.ui_enabled, Some(true)) && cfg.ui_port.is_some_and(port_in_lounge_band)
}

/// Live file still shows exactly what Lounge wrote (`--ui=true --port=N`).
pub fn matches_lounge_footprint(cfg: &CbmUiConfigFile, lounge_port: u16) -> bool {
    cfg.ui_enabled == Some(true) && cfg.ui_port == Some(lounge_port)
}

pub fn read_cbm_ui_config(path: &Path) -> Result<Option<(CbmUiConfigFile, String)>> {
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let cfg: CbmUiConfigFile =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    Ok(Some((cfg, raw)))
}

pub fn snapshot_cbm_ui_config_at(
    path: &Path,
    lounge_port: Option<u16>,
) -> Result<CbmUiConfigSnapshot> {
    match read_cbm_ui_config(path)? {
        None => Ok(CbmUiConfigSnapshot {
            existed: false,
            ui_enabled: None,
            ui_port: None,
            raw: None,
            path: Some(path.to_path_buf()),
            lounge_port,
        }),
        Some((cfg, raw)) => Ok(CbmUiConfigSnapshot {
            existed: true,
            ui_enabled: cfg.ui_enabled,
            ui_port: cfg.ui_port,
            raw: Some(raw),
            path: Some(path.to_path_buf()),
            lounge_port,
        }),
    }
}

pub fn snapshot_cbm_ui_config(lounge_port: Option<u16>) -> Result<CbmUiConfigSnapshot> {
    let path = cbm_ui_config_path().context("CBM cache dir unresolved")?;
    snapshot_cbm_ui_config_at(&path, lounge_port)
}

/// Atomic write: temp in same directory → rename.
pub fn write_cbm_ui_config_atomic(path: &Path, cfg: &CbmUiConfigFile) -> Result<()> {
    let parent = path
        .parent()
        .context("config.json has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    let json = serde_json::to_string_pretty(cfg).context("serialize CBM UI config")?;
    let tmp_name = format!(
        "{}.lounge-tmp-{}-{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("config.json"),
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let tmp = parent.join(tmp_name);
    {
        let mut file =
            fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        file.write_all(json.as_bytes())
            .with_context(|| format!("write {}", tmp.display()))?;
        file.sync_all().ok();
    }
    fs::rename(&tmp, path).with_context(|| {
        let _ = fs::remove_file(&tmp);
        format!("rename {} → {}", tmp.display(), path.display())
    })?;
    Ok(())
}

fn write_raw_atomic(path: &Path, raw: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("config.json has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    let tmp_name = format!(
        "{}.lounge-restore-tmp-{}-{}",
        path.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("config.json"),
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or(0)
    );
    let tmp = parent.join(tmp_name);
    {
        let mut file =
            fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        file.write_all(raw.as_bytes())
            .with_context(|| format!("write {}", tmp.display()))?;
        file.sync_all().ok();
    }
    fs::rename(&tmp, path).with_context(|| {
        let _ = fs::remove_file(&tmp);
        format!("restore rename {} → {}", tmp.display(), path.display())
    })?;
    Ok(())
}

fn backup_config(path: &Path) -> Result<PathBuf> {
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let backup = path.with_file_name(format!("config.json.lounge-backup-{stamp}"));
    fs::copy(path, &backup)
        .with_context(|| format!("backup {} → {}", path.display(), backup.display()))?;
    Ok(backup)
}

fn resolve_snapshot_path(snapshot: &CbmUiConfigSnapshot) -> Result<PathBuf> {
    if let Some(path) = snapshot.path.as_ref() {
        return Ok(path.clone());
    }
    cbm_ui_config_path().context("CBM cache dir unresolved")
}

/// Guarded restore: only when the live file still matches Lounge's footprint.
pub fn restore_cbm_ui_config_at(
    path: &Path,
    snapshot: &CbmUiConfigSnapshot,
) -> Result<RestoreOutcome> {
    let Some(lounge_port) = snapshot.lounge_port else {
        log::warn!(
            "cbm ui config restore skipped: {} has no lounge_port footprint",
            path.display()
        );
        return Ok(RestoreOutcome::Skipped);
    };

    let Some((cfg, _)) = read_cbm_ui_config(path)? else {
        log::warn!(
            "cbm ui config restore skipped: {} absent (nothing to undo)",
            path.display()
        );
        return Ok(RestoreOutcome::Skipped);
    };

    if !matches_lounge_footprint(&cfg, lounge_port) {
        log::warn!(
            "cbm ui config restore skipped: {} no longer matches Lounge footprint ui_enabled=true ui_port={} (live ui_enabled={:?} ui_port={:?})",
            path.display(),
            lounge_port,
            cfg.ui_enabled,
            cfg.ui_port
        );
        return Ok(RestoreOutcome::Skipped);
    }

    if !snapshot.existed {
        // Delete only when content is still Lounge's footprint (checked above).
        log::warn!(
            "cbm ui config restore: removing Lounge footprint from {} (file did not exist before; ui_port={lounge_port})",
            path.display()
        );
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
        return Ok(RestoreOutcome::Restored);
    }

    if let Some(raw) = snapshot.raw.as_deref() {
        let current = fs::read_to_string(path).ok();
        if current.as_deref() == Some(raw) {
            log::warn!(
                "cbm ui config restore: {} already matches snapshot (no-op)",
                path.display()
            );
            return Ok(RestoreOutcome::Restored);
        }
        write_raw_atomic(path, raw)?;
        log::warn!(
            "cbm ui config restore: {} restored from snapshot raw (lounge_port={lounge_port})",
            path.display()
        );
        return Ok(RestoreOutcome::Restored);
    }

    let mut next = cfg;
    let old_enabled = next.ui_enabled;
    let old_port = next.ui_port;
    next.ui_enabled = snapshot.ui_enabled;
    next.ui_port = snapshot.ui_port;
    write_cbm_ui_config_atomic(path, &next)?;
    log::warn!(
        "cbm ui config restore: {} ui_enabled {:?}→{:?} ui_port {:?}→{:?}",
        path.display(),
        old_enabled,
        next.ui_enabled,
        old_port,
        next.ui_port
    );
    Ok(RestoreOutcome::Restored)
}

/// Unconditional restore used by one-time migration (backup already taken).
pub fn force_restore_cbm_ui_config_at(path: &Path, snapshot: &CbmUiConfigSnapshot) -> Result<()> {
    if !snapshot.existed {
        if path.is_file() {
            fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
            log::warn!(
                "cbm ui config migration: removed {} (snapshot said absent)",
                path.display()
            );
        }
        return Ok(());
    }
    if let Some(raw) = snapshot.raw.as_deref() {
        write_raw_atomic(path, raw)?;
        log::warn!(
            "cbm ui config migration: {} force-restored from snapshot raw",
            path.display()
        );
        return Ok(());
    }
    let mut cfg = read_cbm_ui_config(path)?
        .map(|(c, _)| c)
        .unwrap_or(CbmUiConfigFile {
            ui_enabled: None,
            ui_port: None,
            extra: serde_json::Map::new(),
        });
    cfg.ui_enabled = snapshot.ui_enabled;
    cfg.ui_port = snapshot.ui_port;
    write_cbm_ui_config_atomic(path, &cfg)?;
    log::warn!(
        "cbm ui config migration: {} force-restored fields ui_enabled={:?} ui_port={:?}",
        path.display(),
        cfg.ui_enabled,
        cfg.ui_port
    );
    Ok(())
}

pub fn restore_cbm_ui_config(snapshot: &CbmUiConfigSnapshot) -> Result<RestoreOutcome> {
    let path = resolve_snapshot_path(snapshot)?;
    restore_cbm_ui_config_at(&path, snapshot)
}

/// One-time migration: revert Lounge band pollution without touching out-of-band values.
pub fn migrate_lounge_cbm_config_pollution_at(
    path: &Path,
    snapshot: Option<&CbmUiConfigSnapshot>,
) -> Result<MigrationOutcome> {
    let Some((mut cfg, _)) = read_cbm_ui_config(path)? else {
        return Ok(MigrationOutcome {
            changed: false,
            backup_path: None,
            detail: "config.json absent — no migration".into(),
        });
    };

    if let Some(snap) = snapshot {
        if snap.existed && looks_like_lounge_pollution(&cfg) {
            let snap_clean = !matches!(
                (snap.ui_enabled, snap.ui_port),
                (Some(true), Some(p)) if port_in_lounge_band(p)
            );
            if snap_clean {
                let backup = backup_config(path)?;
                let old = format!("ui_enabled={:?} ui_port={:?}", cfg.ui_enabled, cfg.ui_port);
                force_restore_cbm_ui_config_at(path, snap)?;
                let detail = format!(
                    "migrated via snapshot: {old} → snapshot (backup {})",
                    backup.display()
                );
                log::warn!("cbm ui config migration: {detail}");
                return Ok(MigrationOutcome {
                    changed: true,
                    backup_path: Some(backup),
                    detail,
                });
            }
        }
    }

    if !looks_like_lounge_pollution(&cfg) {
        return Ok(MigrationOutcome {
            changed: false,
            backup_path: None,
            detail: "no Lounge-band pollution — no-op".into(),
        });
    }

    let backup = backup_config(path)?;
    let old_enabled = cfg.ui_enabled;
    let old_port = cfg.ui_port;
    cfg.ui_enabled = Some(false);
    cfg.ui_port = Some(CBM_DEFAULT_UI_PORT);
    write_cbm_ui_config_atomic(path, &cfg)?;
    let detail = format!(
        "band-rule migration: {} ui_enabled {:?}→{:?} ui_port {:?}→{:?} (backup {})",
        path.display(),
        old_enabled,
        cfg.ui_enabled,
        old_port,
        cfg.ui_port,
        backup.display()
    );
    log::warn!("cbm ui config migration: {detail}");
    Ok(MigrationOutcome {
        changed: true,
        backup_path: Some(backup),
        detail,
    })
}

pub fn migrate_lounge_cbm_config_pollution(
    snapshot: Option<&CbmUiConfigSnapshot>,
) -> Result<MigrationOutcome> {
    let path = match snapshot
        .and_then(|s| s.path.clone())
        .or_else(cbm_ui_config_path)
    {
        Some(p) => p,
        None => {
            return Ok(MigrationOutcome {
                changed: false,
                backup_path: None,
                detail: "CBM cache dir unresolved — skip migration".into(),
            });
        }
    };
    migrate_lounge_cbm_config_pollution_at(&path, snapshot)
}

pub async fn load_snapshot_from_store(
    store: &crate::db::ExperienceStore,
) -> Option<CbmUiConfigSnapshot> {
    let raw = store
        .get_setting(SETTINGS_KEY_CBM_CONFIG_SNAPSHOT.into())
        .await
        .ok()
        .flatten()?;
    if raw.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&raw).ok()
}

pub async fn persist_snapshot_to_store(
    store: &crate::db::ExperienceStore,
    snapshot: &CbmUiConfigSnapshot,
) -> Result<()> {
    let json = serde_json::to_string(snapshot).context("serialize snapshot")?;
    store
        .set_setting(SETTINGS_KEY_CBM_CONFIG_SNAPSHOT.into(), json)
        .await
        .context("persist cbm config snapshot")?;
    Ok(())
}

pub async fn clear_snapshot_in_store(store: &crate::db::ExperienceStore) -> Result<()> {
    store
        .set_setting(SETTINGS_KEY_CBM_CONFIG_SNAPSHOT.into(), String::new())
        .await
        .context("clear cbm config snapshot")?;
    Ok(())
}

pub async fn migration_marker_set(store: &crate::db::ExperienceStore) -> bool {
    matches!(
        store
            .get_setting(SETTINGS_KEY_CBM_CONFIG_MIGRATED.into())
            .await
            .ok()
            .flatten()
            .as_deref(),
        Some("1")
    )
}

/// Startup hygiene:
/// - If migration marker is set: crash-recovery only (guarded restore of stored snapshot).
/// - Otherwise: one-time band-rule / snapshot migration, then set the marker.
pub async fn run_startup_cbm_config_migration(store: &crate::db::ExperienceStore) -> Result<()> {
    let snapshot = load_snapshot_from_store(store).await;

    if migration_marker_set(store).await {
        if let Some(snap) = snapshot.as_ref() {
            if snap.lounge_port.is_some() {
                match restore_cbm_ui_config(snap) {
                    Ok(RestoreOutcome::Restored) => {
                        log::warn!(
                            "cbm ui config startup: crash-recovery restore applied; clearing snapshot"
                        );
                    }
                    Ok(RestoreOutcome::Skipped) => {
                        log::warn!(
                            "cbm ui config startup: crash-recovery restore skipped; clearing snapshot"
                        );
                    }
                    Err(err) => {
                        log::warn!("cbm ui config startup crash-recovery failed: {err}");
                    }
                }
                let _ = clear_snapshot_in_store(store).await;
            }
        }
        return Ok(());
    }

    let outcome = migrate_lounge_cbm_config_pollution(snapshot.as_ref())?;
    store
        .set_setting(SETTINGS_KEY_CBM_CONFIG_MIGRATED.into(), "1".into())
        .await
        .context("mark cbm config migrated")?;
    if outcome.changed {
        log::warn!("cbm ui config startup migration: {}", outcome.detail);
    }
    if snapshot.is_some() {
        let _ = clear_snapshot_in_store(store).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("lounge-cbm-cfg-{nanos}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cache_dir_prefers_cbm_cache_dir_env_pure() {
        let resolved = cbm_cache_dir_from(|k| match k {
            "CBM_CACHE_DIR" => Some("/tmp/cbm-test-cache".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(resolved, PathBuf::from("/tmp/cbm-test-cache"));
    }

    #[test]
    fn cache_dir_falls_back_to_home_cache() {
        let resolved = cbm_cache_dir_from(|k| match k {
            "HOME" => Some("/Users/demo".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            resolved,
            PathBuf::from("/Users/demo/.cache/codebase-memory-mcp")
        );
    }

    #[test]
    fn snapshot_absent_file() {
        let dir = temp_dir();
        let path = dir.join("config.json");
        let snap = snapshot_cbm_ui_config_at(&path, Some(18750)).unwrap();
        assert!(!snap.existed);
        assert_eq!(snap.lounge_port, Some(18750));
        assert_eq!(snap.path.as_deref(), Some(path.as_path()));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn snapshot_and_restore_user_values_when_footprint_matches() {
        let dir = temp_dir();
        let path = dir.join("config.json");
        let original = CbmUiConfigFile {
            ui_enabled: Some(false),
            ui_port: Some(9749),
            extra: serde_json::Map::new(),
        };
        write_cbm_ui_config_atomic(&path, &original).unwrap();
        let snap = snapshot_cbm_ui_config_at(&path, Some(18749)).unwrap();

        let polluted = CbmUiConfigFile {
            ui_enabled: Some(true),
            ui_port: Some(18749),
            extra: serde_json::Map::new(),
        };
        write_cbm_ui_config_atomic(&path, &polluted).unwrap();
        assert_eq!(
            restore_cbm_ui_config_at(&path, &snap).unwrap(),
            RestoreOutcome::Restored
        );
        let (restored, _) = read_cbm_ui_config(&path).unwrap().unwrap();
        assert_eq!(restored.ui_enabled, Some(false));
        assert_eq!(restored.ui_port, Some(9749));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn restore_skips_when_third_party_changed_after_lounge() {
        let dir = temp_dir();
        let path = dir.join("config.json");
        let snap = snapshot_cbm_ui_config_at(&path, Some(18749)).unwrap();
        assert!(!snap.existed);

        // Lounge wrote footprint, then a third party rewrote to Antigravity defaults.
        write_cbm_ui_config_atomic(
            &path,
            &CbmUiConfigFile {
                ui_enabled: Some(true),
                ui_port: Some(18749),
                extra: serde_json::Map::new(),
            },
        )
        .unwrap();
        // Simulate post-ready restore first.
        assert_eq!(
            restore_cbm_ui_config_at(&path, &snap).unwrap(),
            RestoreOutcome::Restored
        );
        assert!(!path.exists());

        // Third party creates a new config.
        write_cbm_ui_config_atomic(
            &path,
            &CbmUiConfigFile {
                ui_enabled: Some(true),
                ui_port: Some(9749),
                extra: serde_json::Map::new(),
            },
        )
        .unwrap();
        // Stale kill/exit restore must leave it untouched.
        assert_eq!(
            restore_cbm_ui_config_at(&path, &snap).unwrap(),
            RestoreOutcome::Skipped
        );
        let (cfg, _) = read_cbm_ui_config(&path).unwrap().unwrap();
        assert_eq!(cfg.ui_port, Some(9749));
        assert_eq!(cfg.ui_enabled, Some(true));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn restore_absent_does_not_delete_third_party_file() {
        let dir = temp_dir();
        let path = dir.join("config.json");
        let snap = CbmUiConfigSnapshot {
            existed: false,
            ui_enabled: None,
            ui_port: None,
            raw: None,
            path: Some(path.clone()),
            lounge_port: Some(18750),
        };
        // Third party created a file that is NOT Lounge's footprint.
        write_cbm_ui_config_atomic(
            &path,
            &CbmUiConfigFile {
                ui_enabled: Some(true),
                ui_port: Some(9749),
                extra: serde_json::Map::new(),
            },
        )
        .unwrap();
        assert_eq!(
            restore_cbm_ui_config_at(&path, &snap).unwrap(),
            RestoreOutcome::Skipped
        );
        assert!(path.is_file());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn restore_absent_removes_only_lounge_footprint() {
        let dir = temp_dir();
        let path = dir.join("config.json");
        write_cbm_ui_config_atomic(
            &path,
            &CbmUiConfigFile {
                ui_enabled: Some(true),
                ui_port: Some(18750),
                extra: serde_json::Map::new(),
            },
        )
        .unwrap();
        let snap = CbmUiConfigSnapshot {
            existed: false,
            ui_enabled: None,
            ui_port: None,
            raw: None,
            path: Some(path.clone()),
            lounge_port: Some(18750),
        };
        assert_eq!(
            restore_cbm_ui_config_at(&path, &snap).unwrap(),
            RestoreOutcome::Restored
        );
        assert!(!path.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn migration_band_rule_and_idempotent() {
        let dir = temp_dir();
        let path = dir.join("config.json");
        write_cbm_ui_config_atomic(
            &path,
            &CbmUiConfigFile {
                ui_enabled: Some(true),
                ui_port: Some(18749),
                extra: serde_json::Map::new(),
            },
        )
        .unwrap();
        let first = migrate_lounge_cbm_config_pollution_at(&path, None).unwrap();
        assert!(first.changed);
        assert!(first.backup_path.as_ref().unwrap().is_file());
        let (cfg, _) = read_cbm_ui_config(&path).unwrap().unwrap();
        assert_eq!(cfg.ui_enabled, Some(false));
        assert_eq!(cfg.ui_port, Some(9749));

        let second = migrate_lounge_cbm_config_pollution_at(&path, None).unwrap();
        assert!(!second.changed);
        let _ = fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn startup_migration_marker_skips_second_band_rewrite() {
        let dir = temp_dir();
        let path = dir.join("config.json");
        write_cbm_ui_config_atomic(
            &path,
            &CbmUiConfigFile {
                ui_enabled: Some(true),
                ui_port: Some(18749),
                extra: serde_json::Map::new(),
            },
        )
        .unwrap();

        let store = crate::db::ExperienceStore::memory().unwrap();
        // First run: migrate via path injection through snapshot.path.
        let snap = CbmUiConfigSnapshot {
            existed: false,
            ui_enabled: None,
            ui_port: None,
            raw: None,
            path: Some(path.clone()),
            lounge_port: None,
        };
        // Use migrate directly then mark — mirrors first startup without HOME pollution.
        let first = migrate_lounge_cbm_config_pollution_at(&path, None).unwrap();
        assert!(first.changed);
        store
            .set_setting(SETTINGS_KEY_CBM_CONFIG_MIGRATED.into(), "1".into())
            .await
            .unwrap();

        // Third party deliberately picks an in-band port after migration.
        write_cbm_ui_config_atomic(
            &path,
            &CbmUiConfigFile {
                ui_enabled: Some(true),
                ui_port: Some(18755),
                extra: serde_json::Map::new(),
            },
        )
        .unwrap();

        // Second startup: marker set → must not rewrite.
        assert!(migration_marker_set(&store).await);
        // Simulate run_startup with marker: only crash-recovery (no snap lounge_port).
        store
            .set_setting(
                SETTINGS_KEY_CBM_CONFIG_SNAPSHOT.into(),
                serde_json::to_string(&snap).unwrap(),
            )
            .await
            .unwrap();
        run_startup_cbm_config_migration(&store).await.unwrap();
        let (cfg, _) = read_cbm_ui_config(&path).unwrap().unwrap();
        assert_eq!(cfg.ui_port, Some(18755));
        assert_eq!(cfg.ui_enabled, Some(true));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn migration_leaves_out_of_band_untouched() {
        let dir = temp_dir();
        let path = dir.join("config.json");
        write_cbm_ui_config_atomic(
            &path,
            &CbmUiConfigFile {
                ui_enabled: Some(true),
                ui_port: Some(9749),
                extra: serde_json::Map::new(),
            },
        )
        .unwrap();
        let outcome = migrate_lounge_cbm_config_pollution_at(&path, None).unwrap();
        assert!(!outcome.changed);
        let (cfg, _) = read_cbm_ui_config(&path).unwrap().unwrap();
        assert_eq!(cfg.ui_port, Some(9749));
        assert_eq!(cfg.ui_enabled, Some(true));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn migration_via_snapshot_restores_user_port() {
        let dir = temp_dir();
        let path = dir.join("config.json");
        let snap = CbmUiConfigSnapshot {
            existed: true,
            ui_enabled: Some(false),
            ui_port: Some(9800),
            raw: Some(r#"{"ui_enabled":false,"ui_port":9800}"#.into()),
            path: Some(path.clone()),
            lounge_port: Some(18751),
        };
        write_cbm_ui_config_atomic(
            &path,
            &CbmUiConfigFile {
                ui_enabled: Some(true),
                ui_port: Some(18751),
                extra: serde_json::Map::new(),
            },
        )
        .unwrap();
        let outcome = migrate_lounge_cbm_config_pollution_at(&path, Some(&snap)).unwrap();
        assert!(outcome.changed);
        let (cfg, _) = read_cbm_ui_config(&path).unwrap().unwrap();
        assert_eq!(cfg.ui_port, Some(9800));
        assert_eq!(cfg.ui_enabled, Some(false));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn looks_like_lounge_requires_enabled_and_band() {
        assert!(looks_like_lounge_pollution(&CbmUiConfigFile {
            ui_enabled: Some(true),
            ui_port: Some(18749),
            extra: Default::default(),
        }));
        assert!(!looks_like_lounge_pollution(&CbmUiConfigFile {
            ui_enabled: Some(false),
            ui_port: Some(18749),
            extra: Default::default(),
        }));
        assert!(!looks_like_lounge_pollution(&CbmUiConfigFile {
            ui_enabled: Some(true),
            ui_port: Some(9749),
            extra: Default::default(),
        }));
    }
}
