use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout, Instant};

use crate::kernel::GuardedCommand;

pub const DEFAULT_OLLAMA_HOST: &str = "127.0.0.1";
/// Host Ollama (EchoMind / Cursor) varsayılanı. LMR buna dokunmaz.
pub const SYSTEM_OLLAMA_PORT: u16 = 11434;
/// LMR — yalnızca Agent Lounge'a hizmet eden kapalı devre örnek.
pub const LOUNGE_OLLAMA_PORT: u16 = 18790;
pub const DEFAULT_NATS_HOST: &str = "127.0.0.1";
pub const DEFAULT_NATS_PORT: u16 = 4222;
pub const DEFAULT_NATS_HTTP_PORT: u16 = 8222;

/// `tauri.conf.json` `identifier` — `app.path().app_data_dir()` ile aynı sonek.
pub const APP_IDENTIFIER: &str = "com.agentlounge.os";
pub const LOUNGE_DATA_DIR_ENV: &str = "LOUNGE_DATA_DIR";

/// PATH dışında taranan bilinen kurulum dizinleri (Debian `/usr/sbin`, Homebrew, …).
pub const EXTRA_BIN_DIRS: &[&str] = &[
    "/opt/homebrew/bin",
    "/opt/homebrew/sbin",
    "/usr/local/bin",
    "/usr/local/sbin",
    "/usr/bin",
    "/usr/sbin",
    "/opt/homebrew/opt/nats-server/bin",
    "/opt/Claude",
    "/opt/claude-desktop",
    "/opt/Cursor",
    "/opt/cursor",
    "/opt/Antigravity",
    "/opt/Ollama",
    "/snap/bin",
];

/// Windows'ta `PROGRAMFILES` altına eklenen nats / araç alt dizinleri.
pub const WINDOWS_PROGRAM_FILES_SUBDIRS: &[&str] =
    &["NATS Server", "nats-server", "NATS", "Ollama", "Git/cmd"];

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

/// OS / env girdileri — birim testlerde release vs debug yolları aynı hostta.
#[derive(Debug, Clone)]
pub struct DataRootEnv {
    pub override_dir: Option<PathBuf>,
    pub debug_assertions: bool,
    /// Tauri `app.path().app_data_dir()` sonucu (varsa tercih edilir).
    pub app_data_dir: Option<PathBuf>,
    pub os: String,
    pub home: Option<PathBuf>,
    pub xdg_data_home: Option<PathBuf>,
    pub appdata: Option<PathBuf>,
    pub cargo_manifest_dir: PathBuf,
}

impl DataRootEnv {
    pub fn from_process() -> Self {
        Self {
            override_dir: env_nonempty(LOUNGE_DATA_DIR_ENV).map(PathBuf::from),
            debug_assertions: cfg!(debug_assertions),
            app_data_dir: None,
            os: std::env::consts::OS.to_string(),
            home: std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from),
            xdg_data_home: std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
            appdata: std::env::var_os("APPDATA").map(PathBuf::from),
            cargo_manifest_dir: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        }
    }
}

/// Release/bundled: Tauri app data dir (veya `LOUNGE_DATA_DIR`).
/// Debug: repo kökü (`CARGO_MANIFEST_DIR` parent).
pub fn resolve_data_root(env: &DataRootEnv) -> PathBuf {
    if let Some(over) = &env.override_dir {
        return over.clone();
    }
    if env.debug_assertions {
        return repo_root_from_manifest(&env.cargo_manifest_dir);
    }
    if let Some(app) = &env.app_data_dir {
        return app.clone();
    }
    platform_app_data_dir(env)
}

/// Tauri `app_data_dir` ile aynı kural: `{data_dir}/{identifier}`.
pub fn platform_app_data_dir(env: &DataRootEnv) -> PathBuf {
    match env.os.as_str() {
        "windows" => env
            .appdata
            .clone()
            .or_else(|| env.home.clone().map(|h| h.join("AppData").join("Roaming")))
            .unwrap_or_else(|| PathBuf::from(r"C:\Users\Default\AppData\Roaming"))
            .join(APP_IDENTIFIER),
        "linux" => {
            let base = env.xdg_data_home.clone().unwrap_or_else(|| {
                env.home
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("/home"))
                    .join(".local/share")
            });
            base.join(APP_IDENTIFIER)
        }
        _ => env
            .home
            .clone()
            .unwrap_or_else(|| PathBuf::from("/Users"))
            .join("Library/Application Support")
            .join(APP_IDENTIFIER),
    }
}

/// Process-global data root (`LOUNGE_DATA_DIR` / debug repo / release app-data).
pub fn data_root() -> PathBuf {
    resolve_data_root(&DataRootEnv::from_process())
}

/// Tauri setup: `app.path().app_data_dir()` ile doldurulmuş kök + gerekli alt dizinler.
pub fn resolve_data_root_for_app<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf> {
    use tauri::Manager;

    let mut env = DataRootEnv::from_process();
    if let Ok(dir) = app.path().app_data_dir() {
        env.app_data_dir = Some(dir);
    }
    let root = resolve_data_root(&env);
    ensure_data_layout(&root)?;
    Ok(root)
}

/// `experiences/`, `data/nats`, `data/lmr`, `data/ollama/models` — yoksa oluştur.
pub fn ensure_data_layout(root: &Path) -> Result<()> {
    ensure_dir(root, "uygulama veri")?;
    ensure_dir(&root.join("experiences"), "experiences")?;
    ensure_dir(&root.join("data/nats"), "data/nats")?;
    ensure_dir(&root.join("data/lmr"), "data/lmr")?;
    ensure_dir(&root.join("data/ollama/models"), "data/ollama/models")?;
    Ok(())
}

pub fn ensure_dir(path: &Path, label: &str) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("{label} dizini oluşturulamadı: {}", path.display()))
}

pub fn lounge_ollama_models_dir() -> PathBuf {
    env_nonempty("LOUNGE_OLLAMA_MODELS")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_root().join("data/ollama/models"))
}

/// LMR'nin kendi runtime dizini. Host `~/.ollama` ve Ollama.app buraya girmez.
pub fn lounge_lmr_dir() -> PathBuf {
    env_nonempty("LOUNGE_LMR_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_root().join("data/lmr"))
}

pub fn lounge_nats_dir() -> PathBuf {
    env_nonempty("LOUNGE_NATS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_root().join("data/nats"))
}

/// Laya DecisionGate ağırlıkları (`model.safetensors` + tokenizer).
/// Üretim: `AgentLounge/models/laya` (OS app-support). `LOUNGE_LAYA_DIR` ezer.
pub fn lounge_laya_dir() -> PathBuf {
    super::model_manager::laya_dir()
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

/// PATH + bilinen fallback dizinleri (sbin / Homebrew / Windows Program Files).
pub fn executable_search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default();

    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        dirs.push(PathBuf::from(&home).join(".local/bin"));
        dirs.push(PathBuf::from(&home).join("bin"));
        dirs.push(PathBuf::from(&home).join("scoop/shims"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        dirs.push(local.join("Programs"));
        dirs.push(local.join("Programs/cursor"));
        dirs.push(local.join("Programs/Claude"));
        dirs.push(local.join("Programs/Ollama"));
        dirs.push(local.join("Programs/NATS Server"));
        dirs.push(local.join("Programs/nats-server"));
    }
    if let Some(pf) = std::env::var_os("PROGRAMFILES") {
        let pf = PathBuf::from(pf);
        dirs.push(pf.clone());
        for sub in WINDOWS_PROGRAM_FILES_SUBDIRS {
            dirs.push(pf.join(sub));
        }
    }
    if let Some(pf86) = std::env::var_os("PROGRAMFILES(X86)") {
        let pf86 = PathBuf::from(pf86);
        dirs.push(pf86.clone());
        for sub in WINDOWS_PROGRAM_FILES_SUBDIRS {
            dirs.push(pf86.join(sub));
        }
    }
    if let Some(choco) = std::env::var_os("ChocolateyInstall") {
        dirs.push(PathBuf::from(choco).join("bin"));
    } else if cfg!(windows) {
        dirs.push(PathBuf::from(r"C:\ProgramData\chocolatey\bin"));
    }
    dirs.extend(EXTRA_BIN_DIRS.iter().map(PathBuf::from));
    dirs
}

pub fn find_executable(name: &str) -> Option<PathBuf> {
    let file_name = if cfg!(windows) && !name.ends_with(".exe") {
        format!("{name}.exe")
    } else {
        name.to_string()
    };

    executable_search_dirs()
        .into_iter()
        .map(|dir| dir.join(&file_name))
        .find(|candidate| candidate.is_file())
}

pub fn first_existing(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths.into_iter().find(|path| path.is_file())
}

pub fn repo_root_from_manifest(manifest_dir: &Path) -> PathBuf {
    manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Yalnızca debug / sidecar keşif adayları için. Release veri yollarında kullanma.
pub fn repo_root_from_crate() -> PathBuf {
    repo_root_from_manifest(Path::new(env!("CARGO_MANIFEST_DIR")))
}

/// `127.0.0.1:port` üzerinde kısa süreli bind — port boşsa true.
pub fn tcp_bind_available(port: u16) -> bool {
    if port == 0 {
        return false;
    }
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Portta LISTEN yapan süreç PID'leri (macOS/Linux: lsof, yoksa ss; Windows: netstat).
pub fn listen_pids(port: u16) -> Vec<u32> {
    #[cfg(windows)]
    {
        windows_listen_pids(port)
    }
    #[cfg(not(windows))]
    {
        unix_listen_pids(port)
    }
}

/// PID hâlâ yaşıyor mu? (sysinfo)
pub fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true);
    sys.process(Pid::from_u32(pid)).is_some()
}

/// Lounge sahipliği: spawn edilen child canlı VE porttaki dinleyici PID eşleşiyor.
pub fn port_owned_by_lounge(port: u16, child_pid: Option<u32>) -> bool {
    let Some(pid) = child_pid.filter(|p| *p > 0) else {
        return false;
    };
    if !process_alive(pid) {
        return false;
    }
    listen_pids(port).contains(&pid)
}

#[cfg(not(windows))]
fn unix_listen_pids(port: u16) -> Vec<u32> {
    let mut set = BTreeSet::new();
    for pid in lsof_listen_pids(port) {
        set.insert(pid);
    }
    if set.is_empty() {
        for pid in ss_listen_pids(port) {
            set.insert(pid);
        }
    }
    set.into_iter().collect()
}

#[cfg(not(windows))]
fn lsof_listen_pids(port: u16) -> Vec<u32> {
    let output = GuardedCommand::new("lsof")
        .args(["-nP", "-t", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
        .internal_daemon()
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

#[cfg(not(windows))]
fn ss_listen_pids(port: u16) -> Vec<u32> {
    // `ss -lptn 'sport = :PORT'` — process sütunu `pid=NNN` içerir.
    let filter = format!("sport = :{port}");
    let output = GuardedCommand::new("ss")
        .args(["-lptn", &filter])
        .internal_daemon()
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_ss_listen_pids(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(windows))]
fn parse_ss_listen_pids(stdout: &str) -> Vec<u32> {
    let mut set = BTreeSet::new();
    for line in stdout.lines() {
        let mut rest = line;
        while let Some(idx) = rest.find("pid=") {
            let after = &rest[idx + 4..];
            let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.is_empty() {
                rest = &after[1.min(after.len())..];
                continue;
            }
            if let Ok(pid) = digits.parse::<u32>() {
                if pid > 0 {
                    set.insert(pid);
                }
            }
            rest = &after[digits.len()..];
        }
    }
    set.into_iter().collect()
}

#[cfg(windows)]
fn windows_listen_pids(port: u16) -> Vec<u32> {
    let output = GuardedCommand::new("netstat")
        .args(["-ano", "-p", "tcp"])
        .internal_daemon()
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    parse_netstat_listen_pids(&String::from_utf8_lossy(&output.stdout), port)
}

/// Windows `netstat -ano -p tcp` satırlarından LISTEN PID'leri.
/// `:{port}` yalnızca token sonunda eşleşir (`:1` → `:135` yanlış pozitif yok).
#[cfg(any(test, windows))]
pub fn parse_netstat_listen_pids(stdout: &str, port: u16) -> Vec<u32> {
    let needle = format!(":{port}");
    let mut set = BTreeSet::new();
    for line in stdout.lines() {
        let upper = line.to_ascii_uppercase();
        if !upper.contains("LISTEN") {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let has_port = tokens.iter().any(|token| token.ends_with(&needle));
        if !has_port {
            continue;
        }
        if let Some(pid_tok) = tokens.last() {
            if let Ok(pid) = pid_tok.parse::<u32>() {
                if pid > 0 {
                    set.insert(pid);
                }
            }
        }
    }
    set.into_iter().collect()
}

/// Stage `lounge-test-helper` as `codebase-memory-mcp[.exe]` so GuardedCommand allowlist matches.
/// Returns `(binary_path, scratch_dir)` — caller must keep `scratch_dir` alive.
pub fn stage_codebase_memory_mcp_double(helper_bin: &Path) -> Result<(PathBuf, PathBuf)> {
    if !helper_bin.is_file() {
        anyhow::bail!(
            "lounge-test-helper missing at {} — cargo must build the bin target",
            helper_bin.display()
        );
    }
    let scratch = std::env::temp_dir().join(format!(
        "lounge-cbm-double-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&scratch).with_context(|| format!("create {}", scratch.display()))?;
    let name = if cfg!(windows) {
        "codebase-memory-mcp.exe"
    } else {
        "codebase-memory-mcp"
    };
    let dest = scratch.join(name);
    std::fs::copy(helper_bin, &dest)
        .with_context(|| format!("copy {} → {}", helper_bin.display(), dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dest)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&dest, perms)?;
    }
    Ok((dest, scratch))
}

/// Spawn allowlisted `codebase-memory-mcp` double in `tcp-hold` mode (LISTEN on port).
pub fn spawn_tcp_hold_child(binary: &Path, port: u16) -> Result<std::process::Child> {
    let mut command = GuardedCommand::new(binary)
        .arg("tcp-hold")
        .arg(format!("--port={port}"))
        .internal_daemon()
        .into_std_command()
        .with_context(|| format!("tcp-hold gate: {}", binary.display()))?;
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
        .spawn()
        .with_context(|| format!("tcp-hold spawn: {}", binary.display()))
}

/// Wait until `port_owned_by_lounge(port, Some(pid))` or timeout.
pub fn wait_until_port_owned(port: u16, pid: u32, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if port_owned_by_lounge(port, Some(pid)) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
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
        assert!(lounge_laya_dir().ends_with("laya"));
        assert_eq!(nats_monitor_endpoint(), "http://127.0.0.1:8222");
    }

    #[test]
    fn release_data_root_uses_app_data_never_cargo_manifest() {
        let manifest = PathBuf::from("/home/runner/work/Agent-Lounge-OS/Agent-Lounge-OS/src-tauri");
        let app_data = PathBuf::from("/home/user/.local/share/com.agentlounge.os");
        let env = DataRootEnv {
            override_dir: None,
            debug_assertions: false,
            app_data_dir: Some(app_data.clone()),
            os: "linux".into(),
            home: Some(PathBuf::from("/home/user")),
            xdg_data_home: Some(PathBuf::from("/home/user/.local/share")),
            appdata: None,
            cargo_manifest_dir: manifest.clone(),
        };
        let root = resolve_data_root(&env);
        assert_eq!(root, app_data);
        let root_str = root.to_string_lossy();
        assert!(
            !root_str.contains("CARGO_MANIFEST_DIR"),
            "resolved root leaked compile-time token: {root_str}"
        );
        assert!(
            !root.starts_with(manifest.parent().unwrap()),
            "release data root must not be under CARGO_MANIFEST_DIR parent: {}",
            root.display()
        );
        assert!(
            root_str.contains(APP_IDENTIFIER),
            "expected app-data-style root, got {root_str}"
        );
    }

    #[test]
    fn lounge_data_dir_env_wins_in_non_debug() {
        let override_dir = PathBuf::from("/tmp/lounge-data-override-f16");
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let env = DataRootEnv {
            override_dir: Some(override_dir.clone()),
            debug_assertions: false,
            app_data_dir: Some(PathBuf::from("/home/user/.local/share/com.agentlounge.os")),
            os: "linux".into(),
            home: Some(PathBuf::from("/home/user")),
            xdg_data_home: None,
            appdata: None,
            cargo_manifest_dir: manifest.clone(),
        };
        let root = resolve_data_root(&env);
        assert_eq!(root, override_dir);
        let root_str = root.to_string_lossy();
        assert!(!root_str.contains(env!("CARGO_MANIFEST_DIR")));
        assert!(!root_str.contains("CARGO_MANIFEST_DIR"));
    }

    #[test]
    fn platform_app_data_matches_tauri_identifier() {
        let linux = DataRootEnv {
            override_dir: None,
            debug_assertions: false,
            app_data_dir: None,
            os: "linux".into(),
            home: Some(PathBuf::from("/home/demo")),
            xdg_data_home: None,
            appdata: None,
            cargo_manifest_dir: PathBuf::from("/repo/src-tauri"),
        };
        assert_eq!(
            resolve_data_root(&linux),
            PathBuf::from("/home/demo/.local/share/com.agentlounge.os")
        );

        let windows = DataRootEnv {
            override_dir: None,
            debug_assertions: false,
            app_data_dir: None,
            os: "windows".into(),
            home: Some(PathBuf::from(r"C:\Users\demo")),
            xdg_data_home: None,
            appdata: Some(PathBuf::from(r"C:\Users\demo\AppData\Roaming")),
            cargo_manifest_dir: PathBuf::from(r"C:\repo\src-tauri"),
        };
        assert_eq!(
            resolve_data_root(&windows),
            PathBuf::from(r"C:\Users\demo\AppData\Roaming").join(APP_IDENTIFIER)
        );
    }

    #[test]
    fn debug_data_root_stays_repo_relative() {
        let manifest = PathBuf::from("/repo/Agent-Lounge-OS/src-tauri");
        let env = DataRootEnv {
            override_dir: None,
            debug_assertions: true,
            app_data_dir: Some(PathBuf::from("/home/user/.local/share/com.agentlounge.os")),
            os: "linux".into(),
            home: Some(PathBuf::from("/home/user")),
            xdg_data_home: None,
            appdata: None,
            cargo_manifest_dir: manifest,
        };
        assert_eq!(
            resolve_data_root(&env),
            PathBuf::from("/repo/Agent-Lounge-OS")
        );
    }

    #[test]
    fn nats_server_discovery_fallbacks_include_sbin() {
        let dirs = executable_search_dirs();
        let as_str: Vec<String> = dirs
            .iter()
            .map(|d| d.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(
            as_str
                .iter()
                .any(|d| d == "/usr/sbin" || d.ends_with("/usr/sbin")),
            "missing /usr/sbin in {as_str:?}"
        );
        assert!(
            as_str
                .iter()
                .any(|d| d == "/usr/local/sbin" || d.ends_with("/usr/local/sbin")),
            "missing /usr/local/sbin in {as_str:?}"
        );
        assert!(
            as_str
                .iter()
                .any(|d| d.contains("homebrew") && d.contains("sbin")),
            "missing Homebrew sbin in {as_str:?}"
        );
        assert!(EXTRA_BIN_DIRS.contains(&"/usr/sbin"));
        assert!(EXTRA_BIN_DIRS.contains(&"/usr/local/sbin"));
        assert!(EXTRA_BIN_DIRS.contains(&"/opt/homebrew/sbin"));
        assert!(WINDOWS_PROGRAM_FILES_SUBDIRS.contains(&"nats-server"));
    }

    #[test]
    fn unused_port_has_no_listen_pids() {
        assert!(listen_pids(1).is_empty());
    }

    #[test]
    fn tcp_bind_available_roundtrip() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!tcp_bind_available(port), "bound port must not be free");
        drop(listener);
        // macOS (and occasionally Windows) may briefly refuse re-bind after close
        // (TIME_WAIT / stack delay). Poll with a hard deadline — still asserts the
        // real contract that a released local listen port becomes bindable again.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if tcp_bind_available(port) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "released port {port} must become bindable within 3s (macOS/Windows TIME_WAIT)"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    fn listen_pids_matches_our_test_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let self_pid = std::process::id();
        let mut found = false;
        for _ in 0..20 {
            let pids = listen_pids(port);
            if pids.contains(&self_pid) {
                found = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            found,
            "listen_pids({port}) must include this process pid={self_pid}"
        );
        assert!(port_owned_by_lounge(port, Some(self_pid)));
        assert!(!port_owned_by_lounge(port, Some(u32::MAX)));
        assert!(!port_owned_by_lounge(port, None));
        drop(listener);
    }

    #[test]
    fn netstat_parser_matches_exact_port_token() {
        let sample = "\
  TCP    127.0.0.1:18749        0.0.0.0:0              LISTENING       4242\r\n\
  TCP    127.0.0.1:1874         0.0.0.0:0              LISTENING       1111\r\n\
  TCP    0.0.0.0:187490         0.0.0.0:0              LISTENING       2222\r\n\
  TCP    127.0.0.1:135          0.0.0.0:0              LISTENING       3333\r\n";
        assert_eq!(parse_netstat_listen_pids(sample, 18749), vec![4242]);
        assert_eq!(parse_netstat_listen_pids(sample, 1), Vec::<u32>::new());
        assert_eq!(parse_netstat_listen_pids(sample, 135), vec![3333]);
    }
}
