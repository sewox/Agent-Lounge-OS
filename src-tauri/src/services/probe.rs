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
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let candidates = executable_name_candidates(name);
    for dir in executable_search_dirs() {
        for file_name in &candidates {
            let candidate = dir.join(file_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Build PATH lookup names: as-is, plus PATHEXT variants on Windows (`.cmd`/`.bat`/…).
fn executable_name_candidates(name: &str) -> Vec<String> {
    let mut out = vec![name.to_string()];
    if !cfg!(windows) {
        return out;
    }
    let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    let lower = name.to_ascii_lowercase();
    let has_known_ext = pathext.split(';').any(|ext| {
        let e = ext.trim().to_ascii_lowercase();
        !e.is_empty() && lower.ends_with(&e)
    });
    if has_known_ext {
        return out;
    }
    for ext in pathext.split(';') {
        let ext = ext.trim();
        if ext.is_empty() {
            continue;
        }
        let with_ext = if ext.starts_with('.') {
            format!("{name}{ext}")
        } else {
            format!("{name}.{ext}")
        };
        if !out.iter().any(|x| x.eq_ignore_ascii_case(&with_ext)) {
            out.push(with_ext);
        }
    }
    out
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

/// How to combine GetExtendedTcpTable family results with structural netstat.
/// Pure so unit tests cover the decision on every OS (compiled under `test`
/// even when the Win32 callers are `cfg(windows)`-only).
#[cfg(any(test, windows))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListenPidMergeKind {
    /// Both AF_INET and AF_INET6 succeeded — trust API PIDs only.
    ApiOnly,
    /// One family failed — merge successful-family PIDs with netstat (deduped).
    MergeWithNetstat,
    /// Both families failed — netstat only.
    NetstatOnly,
}

#[cfg(any(test, windows))]
fn listen_pid_merge_kind(ipv4_ok: bool, ipv6_ok: bool) -> ListenPidMergeKind {
    match (ipv4_ok, ipv6_ok) {
        (true, true) => ListenPidMergeKind::ApiOnly,
        (false, false) => ListenPidMergeKind::NetstatOnly,
        _ => ListenPidMergeKind::MergeWithNetstat,
    }
}

/// Deduped union of API listener PIDs and structural-netstat PIDs.
#[cfg(any(test, windows))]
fn merge_listen_pid_sets(api_pids: &[u32], netstat_pids: &[u32]) -> Vec<u32> {
    let mut set = BTreeSet::new();
    for pid in api_pids.iter().chain(netstat_pids.iter()) {
        if *pid > 0 {
            set.insert(*pid);
        }
    }
    set.into_iter().collect()
}

#[cfg(windows)]
fn warn_extended_tcp_table_once(port: u16, detail: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if LOGGED.swap(true, Ordering::Relaxed) {
        return;
    }
    // `detail` must carry the Win32 status DWORD(s) from GetExtendedTcpTable.
    log::warn!(
        "listen_pids({port}): GetExtendedTcpTable failed ({detail}); falling back to netstat"
    );
}

#[cfg(windows)]
fn windows_listen_pids(port: u16) -> Vec<u32> {
    // Prefer GetExtendedTcpTable — language-independent (netstat state text is localized).
    let scanned = listen_pids_via_extended_tcp_table(port);
    let kind = listen_pid_merge_kind(scanned.ipv4_ok(), scanned.ipv6_ok());
    match kind {
        ListenPidMergeKind::ApiOnly => scanned.pids,
        ListenPidMergeKind::NetstatOnly => {
            warn_extended_tcp_table_once(
                port,
                &format!(
                    "AF_INET: {}; AF_INET6: {}",
                    scanned.v4_error.as_deref().unwrap_or("?"),
                    scanned.v6_error.as_deref().unwrap_or("?")
                ),
            );
            windows_listen_pids_via_netstat(port)
        }
        ListenPidMergeKind::MergeWithNetstat => {
            // Partial family failure must not be silent — log both sides + consult netstat.
            let detail = match (&scanned.v4_error, &scanned.v6_error) {
                (Some(e4), Some(e6)) => format!("AF_INET: {e4}; AF_INET6: {e6}"),
                (Some(e4), None) => format!("AF_INET: {e4}"),
                (None, Some(e6)) => format!("AF_INET6: {e6}"),
                (None, None) => "unexpected MergeWithNetstat without family error".into(),
            };
            warn_extended_tcp_table_once(port, &detail);
            let via_netstat = windows_listen_pids_via_netstat(port);
            merge_listen_pid_sets(&scanned.pids, &via_netstat)
        }
    }
}

#[cfg(windows)]
fn windows_listen_pids_via_netstat(port: u16) -> Vec<u32> {
    let output = GuardedCommand::new("netstat")
        .args(["-ano", "-p", "tcp"])
        .internal_daemon()
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    parse_netstat_listen_pids(&String::from_utf8_lossy(&output.stdout), port)
}

/// Per-family GetExtendedTcpTable scan (AF_INET + AF_INET6).
#[cfg(windows)]
struct ExtendedTcpTableListenScan {
    pids: Vec<u32>,
    v4_error: Option<String>,
    v6_error: Option<String>,
}

#[cfg(windows)]
impl ExtendedTcpTableListenScan {
    fn ipv4_ok(&self) -> bool {
        self.v4_error.is_none()
    }
    fn ipv6_ok(&self) -> bool {
        self.v6_error.is_none()
    }
}

/// Owner-PID TCP listeners via `GetExtendedTcpTable` (AF_INET + AF_INET6).
/// Partial family failures are returned in the error fields — caller must log
/// and merge with netstat (see [`listen_pid_merge_kind`]).
#[cfg(windows)]
fn listen_pids_via_extended_tcp_table(port: u16) -> ExtendedTcpTableListenScan {
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    let mut set = BTreeSet::new();
    let v4 = collect_ipv4_owner_pid_listeners(AF_INET as u32, port, &mut set);
    let v6 = collect_ipv6_owner_pid_listeners(AF_INET6 as u32, port, &mut set);
    ExtendedTcpTableListenScan {
        pids: set.into_iter().collect(),
        v4_error: v4.err(),
        v6_error: v6.err(),
    }
}

#[cfg(windows)]
fn net_order_port(dw: u32) -> u16 {
    // MSDN: dwLocalPort is network byte order; only the low 16 bits are valid.
    u16::from_be((dw & 0xFFFF) as u16)
}

/// Fill `GetExtendedTcpTable` with retries when the table grows between the
/// size query and the read (`ERROR_INSUFFICIENT_BUFFER` race).
#[cfg(windows)]
fn get_extended_tcp_table_bytes(family: u32) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER,
    };

    unsafe {
        let mut size: u32 = 0;
        let status = GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0,
            family,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
        // Null buffer: expect INSUFFICIENT_BUFFER with a required size. NO_ERROR
        // with size==0 means an empty table for this family.
        if status == 0 && size == 0 {
            return Ok(Vec::new());
        }
        if status != ERROR_INSUFFICIENT_BUFFER || size == 0 {
            return Err(format!("size query win32_status={status} size={size}"));
        }
        for _ in 0..8 {
            let mut buf = vec![0u8; size as usize];
            let mut needed = size;
            let status = GetExtendedTcpTable(
                buf.as_mut_ptr().cast(),
                &mut needed,
                0,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            );
            if status == ERROR_INSUFFICIENT_BUFFER {
                // Table grew — resize and retry (do not silently fall through).
                size = needed.max(size.saturating_add(256));
                continue;
            }
            if status != 0 {
                return Err(format!("fill win32_status={status} needed={needed}"));
            }
            buf.truncate(needed as usize);
            return Ok(buf);
        }
        Err(format!(
            "ERROR_INSUFFICIENT_BUFFER (win32_status={ERROR_INSUFFICIENT_BUFFER}) persisted after retries (last size={size})"
        ))
    }
}

#[cfg(windows)]
fn collect_ipv4_owner_pid_listeners(
    family: u32,
    port: u16,
    out: &mut BTreeSet<u32>,
) -> Result<(), String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        MIB_TCPROW_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
    };

    let buf = get_extended_tcp_table_bytes(family)?;
    if buf.is_empty() {
        return Ok(());
    }
    if buf.len() < std::mem::size_of::<MIB_TCPTABLE_OWNER_PID>() {
        return Err(format!(
            "buffer too small for MIB_TCPTABLE_OWNER_PID ({} bytes)",
            buf.len()
        ));
    }
    unsafe {
        let table = &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID);
        let count = table.dwNumEntries as usize;
        let rows_ptr = std::ptr::addr_of!(table.table) as *const MIB_TCPROW_OWNER_PID;
        for i in 0..count {
            let row = &*rows_ptr.add(i);
            if net_order_port(row.dwLocalPort) != port {
                continue;
            }
            if row.dwOwningPid > 0 {
                out.insert(row.dwOwningPid);
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn collect_ipv6_owner_pid_listeners(
    family: u32,
    port: u16,
    out: &mut BTreeSet<u32>,
) -> Result<(), String> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID,
    };

    let buf = get_extended_tcp_table_bytes(family)?;
    if buf.is_empty() {
        return Ok(());
    }
    if buf.len() < std::mem::size_of::<MIB_TCP6TABLE_OWNER_PID>() {
        return Err(format!(
            "buffer too small for MIB_TCP6TABLE_OWNER_PID ({} bytes)",
            buf.len()
        ));
    }
    unsafe {
        let table = &*(buf.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID);
        let count = table.dwNumEntries as usize;
        let rows_ptr = std::ptr::addr_of!(table.table) as *const MIB_TCP6ROW_OWNER_PID;
        for i in 0..count {
            let row = &*rows_ptr.add(i);
            if net_order_port(row.dwLocalPort) != port {
                continue;
            }
            if row.dwOwningPid > 0 {
                out.insert(row.dwOwningPid);
            }
        }
    }
    Ok(())
}

/// Windows `netstat -ano -p tcp` listener PIDs — **language-independent**.
///
/// Does not match the localized State column (`LISTENING` / `DİNLEME` / `ABHÖREN`).
/// A LISTEN row is identified structurally: TCP proto, local address ends with `:{port}`,
/// foreign address is unbound (`0.0.0.0:0` / `[::]:0` / `*:*`), last token is PID.
/// `:{port}` only matches at the end of a token (`:1` must not hit `:135`).
#[cfg(any(test, windows))]
pub fn parse_netstat_listen_pids(stdout: &str, port: u16) -> Vec<u32> {
    let needle = format!(":{port}");
    let mut set = BTreeSet::new();
    for line in stdout.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        // Proto Local Foreign State PID  → ≥5 tokens (State may be multi-byte localized).
        if tokens.len() < 5 {
            continue;
        }
        let proto = tokens[0];
        if !proto.eq_ignore_ascii_case("TCP") {
            continue;
        }
        let local = tokens[1];
        let foreign = tokens[2];
        if !local.ends_with(&needle) {
            continue;
        }
        if !netstat_foreign_is_unbound_listener(foreign) {
            continue;
        }
        if let Ok(pid) = tokens[tokens.len() - 1].parse::<u32>() {
            if pid > 0 {
                set.insert(pid);
            }
        }
    }
    set.into_iter().collect()
}

/// Foreign address column for a listening socket (not an established connection).
#[cfg(any(test, windows))]
fn netstat_foreign_is_unbound_listener(foreign: &str) -> bool {
    matches!(foreign, "0.0.0.0:0" | "[::]:0" | "*:*" | "*:0" | "[::0]:0")
}

/// Stage `lounge-test-helper` as `codebase-memory-mcp[.exe]` so GuardedCommand allowlist matches.
/// Returns `(binary_path, scratch_dir)` — caller must keep `scratch_dir` alive.
#[cfg(any(test, feature = "test-helpers"))]
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
    // Prefer hard_link of the already-built helper: avoids copy→exec ETXTBSY
    // under parallel --test-threads (Linux "Text file busy").
    if std::fs::hard_link(helper_bin, &dest).is_err() {
        std::fs::copy(helper_bin, &dest)
            .with_context(|| format!("copy {} → {}", helper_bin.display(), dest.display()))?;
        // fsync after copy only on Unix — Windows denies FlushFileBuffers on a
        // freshly copied .exe opened read-only (os error 5).
        #[cfg(unix)]
        {
            let file = std::fs::File::open(&dest)
                .with_context(|| format!("open staged {}", dest.display()))?;
            file.sync_all()
                .with_context(|| format!("fsync staged {}", dest.display()))?;
        }
    }
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
///
/// Stderr is piped for the optional `tcp-hold-ready` line. Primary readiness is
/// still a successful loopback connect + PID ownership (see [`wait_tcp_hold_ready`])
/// because Windows `CREATE_NO_WINDOW` can EOF piped stdio while the child lives.
///
/// `port == 0` binds an ephemeral loopback port in the child (no parent reserve/free).
#[cfg(any(test, feature = "test-helpers"))]
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
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
        .spawn()
        .with_context(|| format!("tcp-hold spawn: {}", binary.display()))
}

/// Ephemeral tcp-hold: child binds `port=0` — no parent reserve → free → rebind race.
#[cfg(any(test, feature = "test-helpers"))]
pub fn spawn_tcp_hold_ephemeral(binary: &Path) -> Result<std::process::Child> {
    spawn_tcp_hold_child(binary, 0)
}

/// Hand a reserved `std::net::TcpListener` to a tcp-hold child (no rebind).
///
/// Returns a [`super::listen_handoff::ListenHandoffGuard`] that must be held
/// until the child is accept-ready (Windows: parent keeps exclusive LISTEN
/// until the child's `--reuse-bind` emits `listen-adopted`).
#[cfg(feature = "test-helpers")]
pub fn spawn_tcp_hold_on_std_listener(
    binary: &Path,
    listener: std::net::TcpListener,
) -> Result<(
    std::process::Child,
    u16,
    super::listen_handoff::ListenHandoffGuard,
)> {
    let port = listener
        .local_addr()
        .context("tcp-hold handoff local_addr")?
        .port();
    let mut command = GuardedCommand::new(binary)
        .arg("tcp-hold")
        .internal_daemon()
        .into_std_command()
        .with_context(|| format!("tcp-hold gate: {}", binary.display()))?;
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    // Unix: --listen-fd + stdin null. Windows: attach adds --reuse-bind --port=N.
    #[cfg(unix)]
    {
        command.stdin(std::process::Stdio::null());
        command.arg(format!("--port={port}"));
    }
    // Avoid CREATE_NO_WINDOW on handoff spawns — it breaks piped stdio on Windows CI.
    let pending = super::listen_handoff::attach_inherited_listener_owned(&mut command, listener)
        .context("attach inherited listener")?;
    let mut child = command
        .spawn()
        .with_context(|| format!("tcp-hold handoff spawn: {}", binary.display()))?;
    let guard = super::listen_handoff::complete_listen_handoff(pending, &mut child)
        .context("complete listen handoff")?;
    Ok((child, port, guard))
}

/// Block until the tcp-hold child is accept-ready on `port` and
/// `listen_pids` includes `child.id()`, or until `timeout`.
///
/// Real readiness signals (not a sleep-as-fix):
/// 1. Optional `tcp-hold-ready` line on stderr (after bind+listen), and/or
/// 2. Successful `TcpStream` connect to `127.0.0.1:port`.
///
/// Then confirm ownership via [`wait_until_port_owned`].
#[cfg(any(test, feature = "test-helpers"))]
pub fn wait_tcp_hold_ready(
    child: &mut std::process::Child,
    port: u16,
    timeout: Duration,
) -> Result<()> {
    let bound = wait_tcp_hold_ready_inner(child, Some(port), timeout)?;
    if bound != port {
        anyhow::bail!("tcp-hold ready port mismatch: want {port}, got {bound}");
    }
    Ok(())
}

/// Wait for an ephemeral (`--port=0`) tcp-hold child; returns the bound port.
#[cfg(any(test, feature = "test-helpers"))]
pub fn wait_tcp_hold_ephemeral_ready(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<u16> {
    wait_tcp_hold_ready_inner(child, None, timeout)
}

#[cfg(any(test, feature = "test-helpers"))]
fn wait_tcp_hold_ready_inner(
    child: &mut std::process::Child,
    expected_port: Option<u16>,
    timeout: Duration,
) -> Result<u16> {
    use std::io::{BufRead, BufReader};
    use std::net::{SocketAddr, TcpStream};
    use std::sync::mpsc;

    let expected_pid = child.id();
    let start = std::time::Instant::now();

    let stderr_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let line_rx = child.stderr.take().map(|stderr| {
        let (tx, rx) = mpsc::channel::<u16>();
        let stderr_buf = stderr_buf.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => return,
                    Ok(_) => {
                        let trimmed = line.trim();
                        if let Ok(mut buf) = stderr_buf.lock() {
                            if !buf.is_empty() {
                                buf.push('\n');
                            }
                            buf.push_str(trimmed);
                            if buf.len() > 800 {
                                let drain = buf.len() - 800;
                                buf.drain(..drain);
                            }
                        }
                        if let Some(port) = parse_tcp_hold_ready_port(trimmed, expected_pid) {
                            let port_ok = match expected_port {
                                None => true,
                                Some(want) => want == port,
                            };
                            if port_ok {
                                let _ = tx.send(port);
                                return;
                            }
                        }
                    }
                    Err(_) => return,
                }
            }
        });
        rx
    });

    let mut ready_port: Option<u16> = expected_port;
    let mut saw_accept = false;
    while start.elapsed() < timeout {
        if let Some(status) = child.try_wait().context("tcp-hold try_wait")? {
            let stderr_tail = stderr_buf.lock().map(|g| g.clone()).unwrap_or_default();
            anyhow::bail!(
                "tcp-hold exited before accept-ready: status={status:?}; listen_pids={:?}; stderr={stderr_tail}",
                ready_port.map(listen_pids).unwrap_or_default()
            );
        }

        if let Some(rx) = line_rx.as_ref() {
            if let Ok(port) = rx.try_recv() {
                ready_port = Some(port);
                saw_accept = true;
            }
        }

        if let Some(port) = ready_port {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            if !saw_accept && TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
                saw_accept = true;
            }

            if saw_accept {
                let remain = timeout.saturating_sub(start.elapsed());
                if remain.is_zero() {
                    break;
                }
                if wait_until_port_owned(port, expected_pid, remain.min(Duration::from_millis(200)))
                {
                    return Ok(port);
                }
                let pids = listen_pids(port);
                if !pids.is_empty() && !pids.contains(&expected_pid) {
                    anyhow::bail!(
                        "tcp-hold foreign_port_collision: port={port} accept-ready but listen_pids={pids:?} (want child {expected_pid})"
                    );
                }
            }
        }

        std::thread::sleep(Duration::from_millis(25));
    }

    let status = child.try_wait().ok().flatten();
    anyhow::bail!(
        "tcp-hold not ready within {timeout:?} for port={expected_port:?} pid={expected_pid}; saw_accept={saw_accept}; child_status={status:?}; listen_pids={:?}",
        ready_port.map(listen_pids).unwrap_or_default()
    );
}

#[cfg(any(test, feature = "test-helpers"))]
fn parse_tcp_hold_ready_port(line: &str, expected_pid: u32) -> Option<u16> {
    // tcp-hold-ready port=12345 pid=67890
    let rest = line.strip_prefix("tcp-hold-ready ")?;
    let mut port = None;
    let mut pid = None;
    for part in rest.split_whitespace() {
        if let Some(v) = part.strip_prefix("port=") {
            port = v.parse().ok();
        } else if let Some(v) = part.strip_prefix("pid=") {
            pid = v.parse().ok();
        }
    }
    if pid == Some(expected_pid) {
        port
    } else {
        None
    }
}

/// True when [`wait_tcp_hold_ready`] failed because another process holds the port
/// (safe to retry with a new ephemeral port).
#[cfg(any(test, feature = "test-helpers"))]
pub fn tcp_hold_ready_err_is_port_collision(err: &anyhow::Error) -> bool {
    let text = err.to_string();
    text.contains("foreign_port_collision") || text.contains("exited before accept-ready")
}

/// Wait until `port_owned_by_lounge(port, Some(pid))` or timeout.
/// Prefer [`wait_tcp_hold_ready`] (or an HTTP probe) first so this is ownership
/// confirmation after a real bind signal — not the sole readiness mechanism.
#[cfg(any(test, feature = "test-helpers"))]
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

/// Wait until `pid` is no longer listed in `listen_pids(port)` (or never was).
#[cfg(any(test, feature = "test-helpers"))]
pub fn wait_until_port_not_owned(port: u16, pid: u32, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !listen_pids(port).contains(&pid) {
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

    /// Localized Windows netstat State words must still yield listen PIDs so
    /// NATS `--pass` migration does not silently skip non-English hosts.
    /// Uses #92's `parse_netstat_listen_pids` (foreign-unbound structural match).
    #[test]
    fn localized_netstat_listen_pids_support_migration_detection() {
        let sample = "\
  Proto  Local Address          Foreign Address        State           PID
  TCP    0.0.0.0:4222           0.0.0.0:0              DİNLEME         4242
  TCP    127.0.0.1:4222         0.0.0.0:0              ABHÖREN         4243
  TCP    0.0.0.0:4222           0.0.0.0:0              LISTENING       4244
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       892
  TCP    127.0.0.1:4222         10.0.0.1:54321         ESTABLISHED     9999
  TCP    0.0.0.0:1              0.0.0.0:0              DİNLEME         111
";
        let pids = parse_netstat_listen_pids(sample, 4222);
        assert_eq!(
            pids,
            vec![4242, 4243, 4244],
            "Turkish/German/English listen rows must all resolve (no LISTEN word required)"
        );
        assert!(
            parse_netstat_listen_pids(sample, 1).contains(&111),
            ":1 must not false-positive on :135"
        );
        assert!(
            !pids.contains(&9999),
            "ESTABLISHED foreign must not count as listen"
        );
        assert!(!pids.contains(&892), "unrelated listen port must not match");
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

    /// Prove the Win32 API path itself (not netstat fallback) sees a live listener.
    #[cfg(windows)]
    #[test]
    fn extended_tcp_table_api_finds_live_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let self_pid = std::process::id();
        let mut last = String::from("(no attempt)");
        let mut found = false;
        for _ in 0..40 {
            let scan = listen_pids_via_extended_tcp_table(port);
            if scan.pids.contains(&self_pid) {
                found = true;
                break;
            }
            last = format!(
                "pids={:?} v4_err={:?} v6_err={:?}",
                scan.pids, scan.v4_error, scan.v6_error
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(
            found,
            "GetExtendedTcpTable must find pid={self_pid} on :{port} without netstat; last={last}"
        );
        drop(listener);
    }

    #[test]
    fn listen_pid_merge_kind_covers_family_outcomes() {
        assert_eq!(
            listen_pid_merge_kind(true, true),
            ListenPidMergeKind::ApiOnly
        );
        assert_eq!(
            listen_pid_merge_kind(false, false),
            ListenPidMergeKind::NetstatOnly
        );
        assert_eq!(
            listen_pid_merge_kind(true, false),
            ListenPidMergeKind::MergeWithNetstat
        );
        assert_eq!(
            listen_pid_merge_kind(false, true),
            ListenPidMergeKind::MergeWithNetstat
        );
    }

    #[test]
    fn merge_listen_pid_sets_dedupes_and_drops_zero() {
        assert_eq!(
            merge_listen_pid_sets(&[10, 20], &[20, 30, 0]),
            vec![10, 20, 30]
        );
        assert_eq!(merge_listen_pid_sets(&[], &[7]), vec![7]);
        assert_eq!(merge_listen_pid_sets(&[5], &[]), vec![5]);
        assert!(merge_listen_pid_sets(&[], &[]).is_empty());
        // Partial-family failure path: API found v4 pid, netstat supplies v6-only peer.
        assert_eq!(
            merge_listen_pid_sets(&[4242], &[5150]),
            vec![4242, 5150],
            "MergeWithNetstat must keep PIDs from both sources"
        );
    }

    #[cfg(any(test, feature = "test-helpers"))]
    #[test]
    fn tcp_hold_ready_err_is_port_collision_matches_markers() {
        let foreign = anyhow::anyhow!(
            "tcp-hold foreign_port_collision: port=1 accept-ready but listen_pids=[9]"
        );
        let exited = anyhow::anyhow!("tcp-hold exited before accept-ready: status=exit");
        let other = anyhow::anyhow!("tcp-hold not ready within 5s");
        assert!(tcp_hold_ready_err_is_port_collision(&foreign));
        assert!(tcp_hold_ready_err_is_port_collision(&exited));
        assert!(!tcp_hold_ready_err_is_port_collision(&other));
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

    #[test]
    fn netstat_parser_accepts_turkish_dinleme_state() {
        // Turkish Windows localizes LISTENING → DİNLEME; must still resolve PID.
        let sample = "\
  TCP    127.0.0.1:18749        0.0.0.0:0              DİNLEME         4242\r\n\
  TCP    0.0.0.0:18749          0.0.0.0:0              DİNLEME         4242\r\n\
  TCP    127.0.0.1:18749        10.0.0.5:443          ESTABLISHED     9999\r\n";
        assert_eq!(parse_netstat_listen_pids(sample, 18749), vec![4242]);
        assert!(
            !parse_netstat_listen_pids(sample, 18749).contains(&9999),
            "established connection must not count as listen"
        );
    }

    #[test]
    fn netstat_parser_accepts_german_abhoren_state() {
        // German Windows localizes LISTENING → ABHÖREN; must still resolve PID.
        let sample = "\
  TCP    127.0.0.1:18749        0.0.0.0:0              ABHÖREN         5150\r\n\
  TCP    [::]:18749             [::]:0                 ABHÖREN         5150\r\n\
  TCP    127.0.0.1:80           0.0.0.0:0              ABHÖREN         80\r\n";
        assert_eq!(parse_netstat_listen_pids(sample, 18749), vec![5150]);
        assert_eq!(parse_netstat_listen_pids(sample, 80), vec![80]);
        assert_eq!(parse_netstat_listen_pids(sample, 1), Vec::<u32>::new());
    }
}
