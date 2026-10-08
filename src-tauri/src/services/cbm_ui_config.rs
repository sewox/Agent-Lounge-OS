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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationOutcome {
    pub changed: bool,
    pub backup_path: Option<PathBuf>,
    pub detail: String,
}

/// Resolve CBM UI config directory — mirrors upstream `cbm_resolve_cache_dir`
/// (v0.11.0): `CBM_CACHE_DIR` if set, else `$HOME/.cache/codebase-memory-mcp`
/// (`USERPROFILE` on Windows when `HOME` is unset). Not `%LOCALAPPDATA%`.
pub fn cbm_cache_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("CBM_CACHE_DIR") {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    Some(home.join(".cache").join("codebase-memory-mcp"))
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

pub fn read_cbm_ui_config(path: &Path) -> Result<Option<(CbmUiConfigFile, String)>> {
    if !path.is_file() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let cfg: CbmUiConfigFile =
        serde_json::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    Ok(Some((cfg, raw)))
}

pub fn snapshot_cbm_ui_config_at(path: &Path) -> Result<CbmUiConfigSnapshot> {
    match read_cbm_ui_config(path)? {
        None => Ok(CbmUiConfigSnapshot {
            existed: false,
            ui_enabled: None,
            ui_port: None,
            raw: None,
        }),
        Some((cfg, raw)) => Ok(CbmUiConfigSnapshot {
            existed: true,
            ui_enabled: cfg.ui_enabled,
            ui_port: cfg.ui_port,
            raw: Some(raw),
        }),
    }
}

pub fn snapshot_cbm_ui_config() -> Result<CbmUiConfigSnapshot> {
    let path = cbm_ui_config_path().context("CBM cache dir unresolved")?;
    snapshot_cbm_ui_config_at(&path)
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

fn backup_config(path: &Path) -> Result<PathBuf> {
    let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let backup = path.with_file_name(format!("config.json.lounge-backup-{stamp}"));
    fs::copy(path, &backup)
        .with_context(|| format!("backup {} → {}", path.display(), backup.display()))?;
    Ok(backup)
}

/// Restore pre-spawn values. Idempotent when already matching the snapshot.
pub fn restore_cbm_ui_config_at(path: &Path, snapshot: &CbmUiConfigSnapshot) -> Result<()> {
    if !snapshot.existed {
        if path.is_file() {
            // File was created by Lounge's child — remove it to restore absence.
            log::warn!(
                "cbm ui config restore: removing Lounge-created {} (did not exist before)",
                path.display()
            );
            fs::remove_file(path).with_context(|| format!("remove {}", path.display()))?;
        }
        return Ok(());
    }

    if let Some(raw) = snapshot.raw.as_deref() {
        let current = fs::read_to_string(path).ok();
        if current.as_deref() == Some(raw) {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.lounge-restore-tmp");
        {
            let mut file = fs::File::create(&tmp)?;
            file.write_all(raw.as_bytes())?;
            file.sync_all().ok();
        }
        fs::rename(&tmp, path).with_context(|| {
            let _ = fs::remove_file(&tmp);
            format!("restore rename → {}", path.display())
        })?;
        log::warn!(
            "cbm ui config restore: {} restored from snapshot (raw)",
            path.display()
        );
        return Ok(());
    }

    // Field-level restore when raw is missing.
    let mut cfg = read_cbm_ui_config(path)?
        .map(|(c, _)| c)
        .unwrap_or(CbmUiConfigFile {
            ui_enabled: None,
            ui_port: None,
            extra: serde_json::Map::new(),
        });
    let old_enabled = cfg.ui_enabled;
    let old_port = cfg.ui_port;
    cfg.ui_enabled = snapshot.ui_enabled;
    cfg.ui_port = snapshot.ui_port;
    if old_enabled == cfg.ui_enabled && old_port == cfg.ui_port {
        return Ok(());
    }
    write_cbm_ui_config_atomic(path, &cfg)?;
    log::warn!(
        "cbm ui config restore: {} ui_enabled {:?}→{:?} ui_port {:?}→{:?}",
        path.display(),
        old_enabled,
        cfg.ui_enabled,
        old_port,
        cfg.ui_port
    );
    Ok(())
}

pub fn restore_cbm_ui_config(snapshot: &CbmUiConfigSnapshot) -> Result<()> {
    let path = cbm_ui_config_path().context("CBM cache dir unresolved")?;
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

    // Prefer explicit Lounge snapshot when present and the live file looks polluted.
    if let Some(snap) = snapshot {
        if snap.existed && looks_like_lounge_pollution(&cfg) {
            let snap_clean = !matches!(
                (snap.ui_enabled, snap.ui_port),
                (Some(true), Some(p)) if port_in_lounge_band(p)
            );
            if snap_clean {
                let backup = backup_config(path)?;
                let old = format!("ui_enabled={:?} ui_port={:?}", cfg.ui_enabled, cfg.ui_port);
                restore_cbm_ui_config_at(path, snap)?;
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
    // Revert only Lounge footprint to upstream defaults.
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
    let path = match cbm_ui_config_path() {
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

pub async fn run_startup_cbm_config_migration(store: &crate::db::ExperienceStore) -> Result<()> {
    let snapshot = load_snapshot_from_store(store).await;
    // Always re-check the live file: second run is a no-op when clean.
    let outcome = migrate_lounge_cbm_config_pollution(snapshot.as_ref())?;
    store
        .set_setting(SETTINGS_KEY_CBM_CONFIG_MIGRATED.into(), "1".into())
        .await
        .context("mark cbm config migrated")?;
    if outcome.changed {
        log::warn!("cbm ui config startup migration: {}", outcome.detail);
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
    fn cache_dir_prefers_cbm_cache_dir_env() {
        let dir = temp_dir();
        let prev = std::env::var_os("CBM_CACHE_DIR");
        std::env::set_var("CBM_CACHE_DIR", &dir);
        let resolved = cbm_cache_dir().unwrap();
        assert_eq!(resolved, dir);
        match prev {
            Some(v) => std::env::set_var("CBM_CACHE_DIR", v),
            None => std::env::remove_var("CBM_CACHE_DIR"),
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn snapshot_absent_file() {
        let dir = temp_dir();
        let path = dir.join("config.json");
        let snap = snapshot_cbm_ui_config_at(&path).unwrap();
        assert!(!snap.existed);
        assert!(snap.ui_enabled.is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn snapshot_and_restore_user_values() {
        let dir = temp_dir();
        let path = dir.join("config.json");
        let original = CbmUiConfigFile {
            ui_enabled: Some(false),
            ui_port: Some(9749),
            extra: serde_json::Map::new(),
        };
        write_cbm_ui_config_atomic(&path, &original).unwrap();
        let snap = snapshot_cbm_ui_config_at(&path).unwrap();
        assert!(snap.existed);
        assert_eq!(snap.ui_port, Some(9749));

        // Lounge pollution.
        let polluted = CbmUiConfigFile {
            ui_enabled: Some(true),
            ui_port: Some(18749),
            extra: serde_json::Map::new(),
        };
        write_cbm_ui_config_atomic(&path, &polluted).unwrap();
        restore_cbm_ui_config_at(&path, &snap).unwrap();
        let (restored, _) = read_cbm_ui_config(&path).unwrap().unwrap();
        assert_eq!(restored.ui_enabled, Some(false));
        assert_eq!(restored.ui_port, Some(9749));
        // Idempotent.
        restore_cbm_ui_config_at(&path, &snap).unwrap();
        let (again, _) = read_cbm_ui_config(&path).unwrap().unwrap();
        assert_eq!(again.ui_port, Some(9749));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn restore_absent_removes_lounge_created_file() {
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
        };
        restore_cbm_ui_config_at(&path, &snap).unwrap();
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
