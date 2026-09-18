use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::{sleep, timeout, Instant};

pub const DEFAULT_OLLAMA_HOST: &str = "127.0.0.1";
pub const DEFAULT_OLLAMA_PORT: u16 = 11434;
pub const DEFAULT_NATS_HOST: &str = "127.0.0.1";
pub const DEFAULT_NATS_PORT: u16 = 4222;

const EXTRA_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/opt/homebrew/opt/nats-server/bin",
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

pub fn find_executable(name: &str) -> Option<PathBuf> {
    let file_name = if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    };

    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();

    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/bin"));
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
}
