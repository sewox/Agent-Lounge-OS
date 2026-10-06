//! Çalışma alanı taraması: kök altında keşfedilebilir proje dizinlerini bulur.
//!
//! "Çalışma Alanını Tara" tek bir klasörü `index_repository` ile senkron
//! indekslemek yerine önce proje köklerini keşfeder; kayıt/import ve arka plan
//! indeks kuyruğu bu liste üzerinden yürür.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::models::ProjectSummary;

/// Derin tarama üst sınırı (seçilen kök = 0).
pub const MAX_SCAN_DEPTH: usize = 4;
/// Tek taramada üst sınır — UI ve kuyruk patlamasın.
pub const MAX_DISCOVERED_PROJECTS: usize = 64;

const PROJECT_MARKERS: &[&str] = &[
    ".git",
    "Cargo.toml",
    "package.json",
    "pyproject.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "Gemfile",
    "composer.json",
];

const SKIP_DIR_NAMES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    "vendor",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    "coverage",
    ".cargo",
    ".idea",
    ".vscode",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceScanErrorKind {
    EmptyPath,
    NotFound,
    NotDirectory,
    PermissionDenied,
    EmptyWorkspace,
    SidecarMissing(String),
    Io(String),
}

impl WorkspaceScanErrorKind {
    pub fn code(&self) -> &'static str {
        match self {
            Self::EmptyPath => "empty_path",
            Self::NotFound => "not_found",
            Self::NotDirectory => "not_directory",
            Self::PermissionDenied => "permission_denied",
            Self::EmptyWorkspace => "empty_workspace",
            Self::SidecarMissing(_) => "sidecar_missing",
            Self::Io(_) => "io_error",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::EmptyPath => "workspace path is empty".into(),
            Self::NotFound => "workspace path not found".into(),
            Self::NotDirectory => "workspace path is not a directory".into(),
            Self::PermissionDenied => "permission denied reading workspace".into(),
            Self::EmptyWorkspace => "no projects found in workspace".into(),
            Self::SidecarMissing(path) => {
                format!("codebase-memory-mcp missing: {path}")
            }
            Self::Io(detail) => format!("workspace scan failed: {detail}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredProject {
    pub name: String,
    pub root_path: PathBuf,
}

impl DiscoveredProject {
    pub fn to_summary(&self) -> ProjectSummary {
        ProjectSummary {
            name: self.name.clone(),
            root_path: Some(self.root_path.display().to_string()),
            nodes: 0,
            edges: 0,
            files: Some(0),
        }
    }
}

/// Seçilen çalışma alanında proje köklerini keşfet.
///
/// - Seçilen dizin kendisi bir proje köküyse dahil edilir.
/// - Alt dizinlerde marker (`.git`, `Cargo.toml`, `package.json`, …) aranır.
/// - `node_modules` / `target` vb. atlanır; derinlik ve adet sınırlıdır.
pub fn discover_projects(
    workspace: impl AsRef<Path>,
) -> Result<Vec<DiscoveredProject>, WorkspaceScanErrorKind> {
    let raw = workspace.as_ref();
    if raw.as_os_str().is_empty() {
        return Err(WorkspaceScanErrorKind::EmptyPath);
    }
    let root = match fs::canonicalize(raw) {
        Ok(path) => path,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(WorkspaceScanErrorKind::NotFound);
        }
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(WorkspaceScanErrorKind::PermissionDenied);
        }
        Err(err) => return Err(WorkspaceScanErrorKind::Io(err.to_string())),
    };
    let meta = match fs::metadata(&root) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(WorkspaceScanErrorKind::PermissionDenied);
        }
        Err(err) => return Err(WorkspaceScanErrorKind::Io(err.to_string())),
    };
    if !meta.is_dir() {
        return Err(WorkspaceScanErrorKind::NotDirectory);
    }

    let mut found = Vec::new();
    let mut seen = std::collections::HashSet::new();
    walk_for_projects(&root, &root, 0, &mut found, &mut seen)?;

    if found.is_empty() {
        return Err(WorkspaceScanErrorKind::EmptyWorkspace);
    }
    found.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.root_path.cmp(&b.root_path))
    });
    Ok(found)
}

fn walk_for_projects(
    workspace_root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<DiscoveredProject>,
    seen: &mut std::collections::HashSet<PathBuf>,
) -> Result<(), WorkspaceScanErrorKind> {
    if out.len() >= MAX_DISCOVERED_PROJECTS {
        return Ok(());
    }

    let is_project = is_project_root(dir);
    if is_project {
        push_discovered(dir, out, seen);
        // Proje kökünün içinde de nested repo aranabilir (workspace monorepo).
    }

    if depth >= MAX_SCAN_DEPTH || out.len() >= MAX_DISCOVERED_PROJECTS {
        return Ok(());
    }

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            // Alt dizin reddedilirse üst seviyede bulunanları koru; kök okunamazsa hata.
            if dir == workspace_root {
                return Err(WorkspaceScanErrorKind::PermissionDenied);
            }
            return Ok(());
        }
        Err(err) => {
            if dir == workspace_root {
                return Err(WorkspaceScanErrorKind::Io(err.to_string()));
            }
            return Ok(());
        }
    };

    for entry in entries.flatten() {
        if out.len() >= MAX_DISCOVERED_PROJECTS {
            break;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if should_skip_dir(&name_str) {
            continue;
        }
        walk_for_projects(workspace_root, &path, depth + 1, out, seen)?;
    }
    Ok(())
}

fn push_discovered(
    dir: &Path,
    out: &mut Vec<DiscoveredProject>,
    seen: &mut std::collections::HashSet<PathBuf>,
) {
    if !seen.insert(dir.to_path_buf()) {
        return;
    }
    if out.len() >= MAX_DISCOVERED_PROJECTS {
        return;
    }
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("workspace")
        .to_string();
    out.push(DiscoveredProject {
        name,
        root_path: dir.to_path_buf(),
    });
}

pub fn is_project_root(dir: &Path) -> bool {
    PROJECT_MARKERS
        .iter()
        .any(|marker| dir.join(marker).exists())
}

fn should_skip_dir(name: &str) -> bool {
    if name.starts_with('.') && name != ".git" {
        // Gizli dizinler genelde proje kökü değil; `.git` marker olarak üstte kontrol edilir.
        return true;
    }
    SKIP_DIR_NAMES
        .iter()
        .any(|skip| name.eq_ignore_ascii_case(skip))
}

/// Blocking discover — Tauri komutundan `spawn_blocking` ile çağrılır.
pub fn discover_projects_blocking(workspace: PathBuf) -> Result<Vec<DiscoveredProject>> {
    discover_projects(&workspace)
        .map_err(|kind| anyhow::anyhow!("{}: {}", kind.code(), kind.message()))
}

/// Test / CLI yardımcısı: hata türünü koruyarak keşfet.
pub fn try_discover(
    workspace: impl AsRef<Path>,
) -> Result<Vec<DiscoveredProject>, WorkspaceScanErrorKind> {
    discover_projects(workspace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("lounge-scan-{label}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch_dir(path: &Path) {
        fs::create_dir_all(path).unwrap();
    }

    fn touch_file(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"x").unwrap();
    }

    #[test]
    fn discovers_nested_git_and_cargo_projects() {
        let root = temp_root("nested");
        touch_dir(&root.join("alpha/.git"));
        touch_file(&root.join("beta/Cargo.toml"));
        touch_file(&root.join("gamma/readme.txt")); // not a project
        touch_dir(&root.join("skip/node_modules/pkg/.git")); // skipped

        let found = discover_projects(&root).expect("discover");
        let names: Vec<_> = found.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"alpha"), "{names:?}");
        assert!(names.contains(&"beta"), "{names:?}");
        assert!(!names.contains(&"gamma"), "{names:?}");
        assert!(!names.contains(&"pkg"), "{names:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn selected_root_is_project_when_marked() {
        let root = temp_root("self");
        touch_file(&root.join("package.json"));
        touch_dir(&root.join("packages/ui"));
        touch_file(&root.join("packages/ui/package.json"));

        let found = discover_projects(&root).expect("discover");
        assert!(found.len() >= 2, "{found:?}");
        assert!(found
            .iter()
            .any(|p| p.root_path == fs::canonicalize(&root).unwrap()));
        assert!(found.iter().any(|p| p.name == "ui"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_workspace_is_explicit_error() {
        let root = temp_root("empty");
        touch_file(&root.join("notes.txt"));
        let err = discover_projects(&root).expect_err("empty");
        assert_eq!(err, WorkspaceScanErrorKind::EmptyWorkspace);
        assert_eq!(err.code(), "empty_workspace");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_path_is_not_found() {
        let missing = std::env::temp_dir().join("lounge-scan-missing-does-not-exist");
        let _ = fs::remove_dir_all(&missing);
        let err = discover_projects(&missing).expect_err("missing");
        assert_eq!(err, WorkspaceScanErrorKind::NotFound);
    }

    #[test]
    fn empty_path_rejected() {
        let err = discover_projects(Path::new("")).expect_err("empty");
        assert_eq!(err, WorkspaceScanErrorKind::EmptyPath);
    }

    #[test]
    fn discover_projects_blocking_maps_error() {
        let root = temp_root("block");
        touch_file(&root.join("only.txt"));
        let err = discover_projects_blocking(root.clone()).unwrap_err();
        assert!(err.to_string().contains("empty_workspace"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    /// POSIX-only: `chmod 000` / `PermissionsExt` unreadable-dir probe.
    /// Windows ACL denial (icacls) is a different API surface; not a hollow
    /// cross-platform twin — keep this out of the Windows unit count.
    #[cfg(unix)]
    #[test]
    fn permission_denied_on_unreadable_root() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_root("perm");
        touch_file(&root.join("Cargo.toml"));
        let mut perms = fs::metadata(&root).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&root, perms).unwrap();
        let result = discover_projects(&root);
        // Restore before cleanup so remove_dir_all works.
        let mut restore = fs::metadata(&root).unwrap().permissions();
        restore.set_mode(0o755);
        let _ = fs::set_permissions(&root, restore);
        match result {
            Err(WorkspaceScanErrorKind::PermissionDenied) => {}
            Err(WorkspaceScanErrorKind::EmptyWorkspace) => {
                // Bazı ortamlar (root/CI) chmod'u yok sayabilir — izin ver.
            }
            other => panic!("expected permission_denied or empty, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&root);
    }
}
