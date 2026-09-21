use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::{sleep, timeout, Instant};

pub const DEFAULT_OLLAMA_HOST: &str = "127.0.0.1";
/// Host Ollama (EchoMind / Cursor) varsayılanı. LMR buna dokunmaz.
pub const SYSTEM_OLLAMA_PORT: u16 = 11434;
/// LMR — yalnızca Agent Lounge'a hizmet eden kapalı devre örnek.
pub const LOUNGE_OLLAMA_PORT: u16 = 18790;
pub const DEFAULT_NATS_HOST: &str = "127.0.0.1";
pub const DEFAULT_NATS_PORT: u16 = 4222;
pub const DEFAULT_NATS_HTTP_PORT: u16 = 8222;

const EXTRA_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/opt/homebrew/opt/nats-server/bin",
    "/opt/Claude",
    "/opt/claude-desktop",
    "/opt/Cursor",
    "/opt/cursor",
    "/opt/Antigravity",
    "/opt/Ollama",
    "/snap/bin",
];

pub async fn tcp_ready(host: &str, port: u16, wait: Duration) -> bool {
    timeout(wait, TcpStream::connect((host, port)))
        .await
        .ok()
        .and_then(Result::ok)
        .is_some()
}

pub async fn wait_until<F, Fut>(deadline: Duration, interval: Duration, mut check: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = Instant::now();
    loop {
        if check().await {
            return true;
        }
        if start.elapsed() >= deadline {
            return false;
        }
        sleep(interval).await;
    }
}

pub fn endpoint(host: &str, port: u16) -> String {
    format!("{host}:{port}")
}

pub fn http_endpoint(host: &str, port: u16) -> String {
    format!("http://{host}:{port}")
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn env_u16(key: &str, default: u16) -> u16 {
    env_nonempty(key)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

/// `127.0.0.1:11434` veya `http://127.0.0.1:11434` → `http://127.0.0.1:11434`
pub fn normalize_ollama_base(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    }
}

pub fn lounge_ollama_host() -> String {
    env_nonempty("LOUNGE_OLLAMA_HOST").unwrap_or_else(|| DEFAULT_OLLAMA_HOST.to_string())
}

pub fn lounge_ollama_port() -> u16 {
    env_u16("LOUNGE_OLLAMA_PORT", LOUNGE_OLLAMA_PORT)
}

pub fn lounge_ollama_endpoint() -> String {
    http_endpoint(&lounge_ollama_host(), lounge_ollama_port())
}

/// Host Ollama Sunucusu. `OLLAMA_HOST` varsa onu kullanır.
pub fn system_ollama_endpoint() -> String {
    env_nonempty("OLLAMA_HOST")
        .map(|raw| normalize_ollama_base(&raw))
        .unwrap_or_else(|| http_endpoint(DEFAULT_OLLAMA_HOST, SYSTEM_OLLAMA_PORT))
}

pub fn lounge_ollama_models_dir() -> PathBuf {
    env_nonempty("LOUNGE_OLLAMA_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root_from_crate().join("data/ollama/models"))
}

/// LMR'nin kendi runtime dizini. Host `~/.ollama` ve Ollama.app buraya girmez.
pub fn lounge_lmr_dir() -> PathBuf {
    env_nonempty("LOUNGE_LMR_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root_from_crate().join("data/lmr"))
}

pub fn lounge_nats_dir() -> PathBuf {
    env_nonempty("LOUNGE_NATS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo_root_from_crate().join("data/nats"))
}

pub fn nats_monitor_endpoint() -> String {
    http_endpoint(DEFAULT_NATS_HOST, DEFAULT_NATS_HTTP_PORT)
}

pub fn lounge_lmr_binary_path() -> PathBuf {
    env_nonempty("LOUNGE_LMR_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let name = if cfg!(windows) {
                "ollama.exe"
            } else {
                "ollama"
            };
            lounge_lmr_dir().join(name)
        })
}

pub fn find_executable(name: &str) -> Option<PathBuf> {
    let file_name = if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    };

    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();

    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        dirs.push(PathBuf::from(&home).join(".local/bin"));
        dirs.push(PathBuf::from(home).join("bin"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        dirs.push(local.join("Programs"));
        dirs.push(local.join("Programs/cursor"));
        dirs.push(local.join("Programs/Claude"));
        dirs.push(local.join("Programs/Ollama"));
    }
    if let Some(pf) = std::env::var_os("PROGRAMFILES") {
        dirs.push(PathBuf::from(pf));
    }
    dirs.extend(EXTRA_BIN_DIRS.iter().map(PathBuf::from));

    dirs.into_iter()
        .map(|dir| dir.join(&file_name))
        .find(|candidate| candidate.is_file())
}

pub fn first_existing(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths.into_iter().find(|path| path.is_file())
}

pub fn repo_root_from_crate() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn detects_open_tcp_port() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        assert!(tcp_ready("127.0.0.1", addr.port(), Duration::from_millis(200)).await);
    }

    #[tokio::test]
    async fn closed_port_is_not_ready() {
        assert!(!tcp_ready("127.0.0.1", 1, Duration::from_millis(150)).await);
    }

    #[test]
    fn normalizes_ollama_host_with_or_without_scheme() {
        assert_eq!(
            normalize_ollama_base("127.0.0.1:11434"),
            "http://127.0.0.1:11434"
        );
        assert_eq!(
            normalize_ollama_base("http://127.0.0.1:11434/"),
            "http://127.0.0.1:11434"
        );
    }

    #[test]
    fn lounge_port_is_not_system_11434() {
        assert_ne!(LOUNGE_OLLAMA_PORT, SYSTEM_OLLAMA_PORT);
        assert_eq!(SYSTEM_OLLAMA_PORT, 11434);
    }

    #[test]
    fn lmr_paths_are_app_owned() {
        let lmr = lounge_lmr_dir();
        assert!(lmr.ends_with("lmr"));
        assert!(lounge_ollama_models_dir().ends_with("models"));
        assert!(!lmr.ends_with(".ollama"));
        assert!(lounge_nats_dir().ends_with("nats"));
        assert_eq!(nats_monitor_endpoint(), "http://127.0.0.1:8222");
    }
}
