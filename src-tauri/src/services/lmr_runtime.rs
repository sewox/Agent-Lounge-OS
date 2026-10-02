//! LMR runtime — EchoMind `models/` kalıbı.
//!
//! Uygulama kendi dizinindeki binary'yi kullanır (`data/lmr/`).
//! Host Ollama (`:11434`, Ollama.app, PATH) ne spawn edilir ne kopyalanır.
//! Host yalnızca HTTP keşfi: EchoMind `test_ollama_connection` → GET /api/tags.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::probe::{find_executable, lounge_lmr_binary_path, lounge_lmr_dir};

pub fn is_default_ollama_name(name: &str) -> bool {
    let stem = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(name);
    stem == "ollama" || stem.eq_ignore_ascii_case("ollama.exe")
}

pub fn is_lmr_owned_path(path: &Path) -> bool {
    let lmr = lounge_lmr_dir();
    if path.starts_with(&lmr) {
        return true;
    }
    match (path.canonicalize(), lmr.canonicalize()) {
        (Ok(resolved), Ok(root)) => resolved.starts_with(root),
        _ => false,
    }
}

/// Kullanıcının kendi Ollama kurulumu. `data/lmr` host sayılmaz.
pub fn find_host_ollama_binary() -> Option<PathBuf> {
    host_ollama_candidates()
        .into_iter()
        .find(|path| path.is_file() && !is_lmr_owned_path(path))
}

pub fn host_ollama_installed() -> bool {
    find_host_ollama_binary().is_some() || host_ollama_app_present()
}

pub fn host_ollama_down_detail(installed: bool) -> &'static str {
    if installed {
        "yüklü, çalışmıyor"
    } else {
        "bulunamadı"
    }
}

fn host_ollama_app_present() -> bool {
    host_ollama_app_dirs().into_iter().any(|path| path.is_dir())
}

fn host_ollama_app_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if cfg!(target_os = "macos") {
        dirs.push(PathBuf::from("/Applications/Ollama.app"));
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Applications/Ollama.app"));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Programs/Ollama"));
    }
    if let Some(pf) = std::env::var_os("PROGRAMFILES") {
        dirs.push(PathBuf::from(pf).join("Ollama"));
    }
    dirs.push(PathBuf::from("/opt/Ollama"));
    dirs.push(PathBuf::from("/usr/share/ollama"));
    dirs
}

fn host_ollama_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = find_executable("ollama") {
        candidates.push(path);
    }
    for app in host_ollama_app_dirs() {
        candidates.push(app.join("Contents/Resources/ollama"));
        candidates.push(app.join("ollama.exe"));
        candidates.push(app.join("ollama"));
    }
    candidates
}

/// Sync probe: is an LMR-owned binary present? Never falls back to host Ollama.
pub fn lmr_runtime_present(requested: &str) -> bool {
    resolve_lmr_runtime_path(requested).is_some()
}

fn resolve_lmr_runtime_path(requested: &str) -> Option<PathBuf> {
    let requested = requested.trim();
    let requested_path = PathBuf::from(requested);
    if requested_path.is_file() {
        return Some(requested_path);
    }

    if !is_default_ollama_name(requested) {
        return find_executable(requested);
    }

    if let Some(path) = std::env::var_os("LOUNGE_LMR_BINARY") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }

    let managed = lounge_lmr_binary_path();
    if managed.is_file() {
        return Some(managed);
    }
    None
}

/// LMR binary: yalnızca uygulama dizini. Host PATH'e düşmez.
pub async fn ensure_lmr_runtime(requested: &str) -> Result<PathBuf> {
    resolve_lmr_runtime_path(requested).ok_or_else(|| {
        anyhow::anyhow!(
            "LMR runtime yok ({}). Host Ollama kullanılmaz.",
            lounge_lmr_dir().display()
        )
    })
}

/// Optional install guidance when the managed binary is absent.
pub fn lmr_not_installed_detail() -> String {
    format!(
        "optional — place an Ollama-compatible binary at {} (or set LOUNGE_LMR_BINARY)",
        lounge_lmr_binary_path().display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_name_is_ollama_only() {
        assert!(is_default_ollama_name("ollama"));
        assert!(is_default_ollama_name("ollama.exe"));
        assert!(!is_default_ollama_name("__missing_ollama__"));
        assert!(!is_default_ollama_name("nats-server"));
    }

    #[test]
    fn missing_host_detail_is_bulunamadi() {
        assert_eq!(host_ollama_down_detail(false), "bulunamadı");
        assert_eq!(host_ollama_down_detail(true), "yüklü, çalışmıyor");
    }

    #[test]
    fn lmr_dir_is_not_host_home_ollama() {
        let dir = lounge_lmr_dir();
        assert!(dir.ends_with("lmr"));
        assert!(!dir.ends_with(".ollama"));
    }

    #[tokio::test]
    async fn missing_custom_binary_does_not_use_host() {
        let err = ensure_lmr_runtime("__missing_ollama__")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("LMR runtime yok"));
    }

    #[test]
    fn missing_default_runtime_is_not_present() {
        // Default name resolves only under data/lmr — never host PATH.
        let prev = std::env::var_os("LOUNGE_LMR_BINARY");
        std::env::remove_var("LOUNGE_LMR_BINARY");
        assert!(!lmr_runtime_present("ollama"));
        assert!(lmr_not_installed_detail().contains("optional"));
        match prev {
            Some(v) => std::env::set_var("LOUNGE_LMR_BINARY", v),
            None => std::env::remove_var("LOUNGE_LMR_BINARY"),
        }
    }
}
