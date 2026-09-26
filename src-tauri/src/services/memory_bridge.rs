use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::probe::{find_executable, first_existing, repo_root_from_crate};
use crate::kernel::GuardedCommand;
use crate::models::{
    AstNode, CodeReference, DeadSymbol, IndexGraph, IndexSnapshot, ProjectList, ProjectSummary,
    ServiceHealth, ServiceId,
};

/// Read-only tool hard timeout (CLI + HTTP).
pub const READ_TIMEOUT: Duration = Duration::from_secs(15);
/// Index / write tools — large repos must not die at 15s.
pub const MUTATING_TIMEOUT: Duration = Duration::from_secs(300);
pub const UI_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
pub const DEFAULT_GRAPH_UI_PORT: u16 = 9749;
/// `list_projects` sonuçları en az bu süre cache'lenir.
pub const LIST_PROJECTS_CACHE_TTL: Duration = Duration::from_secs(60);

/// Semantic map UI için üst sınır — viewport kilitli listede sayfalama var.
const SEMANTIC_NODE_LIMIT: u32 = 400;
const SEMANTIC_CALL_LIMIT: u32 = 800;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransportMode {
    /// Probe `/api/ui-config`; ayaktaysa HTTP `/rpc`, değilse CLI.
    #[default]
    Auto,
    /// Asla HTTP'ye dokunma (test güvenliği).
    ForceCli,
}

#[derive(Debug, Clone)]
pub struct MemoryBridgeConfig {
    pub http_port: u16,
    pub transport: TransportMode,
    pub read_timeout: Duration,
    pub mutating_timeout: Duration,
}

impl Default for MemoryBridgeConfig {
    fn default() -> Self {
        Self {
            http_port: DEFAULT_GRAPH_UI_PORT,
            transport: TransportMode::Auto,
            read_timeout: READ_TIMEOUT,
            mutating_timeout: MUTATING_TIMEOUT,
        }
    }
}

impl MemoryBridgeConfig {
    /// Test / fake-binary yolları — gerçek :9749'a asla vurma.
    pub fn force_cli() -> Self {
        Self {
            transport: TransportMode::ForceCli,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolTransport {
    HttpRpc,
    Cli,
}

struct ListProjectsCache {
    at: Option<Instant>,
    rows: Vec<ProjectSummary>,
}

struct BridgeShared {
    config: RwLock<MemoryBridgeConfig>,
    list_cache: tokio::sync::Mutex<ListProjectsCache>,
}

#[derive(Clone)]
pub struct MemoryBridge {
    binary: PathBuf,
    shared: Arc<BridgeShared>,
}

impl MemoryBridge {
    pub fn discover() -> Result<Self> {
        Self::discover_from(repo_root_from_crate())
    }

    pub fn discover_from(repo_root: impl AsRef<Path>) -> Result<Self> {
        let binary = resolve_binary(repo_root.as_ref())?;
        Ok(Self::with_config(binary, MemoryBridgeConfig::default()))
    }

    /// Test stub: ForceCli — process-global port yok, gerçek cbm UI'ye dokunmaz.
    pub fn from_binary(binary: impl Into<PathBuf>) -> Self {
        Self::with_config(binary, MemoryBridgeConfig::force_cli())
    }

    pub fn with_config(binary: impl Into<PathBuf>, config: MemoryBridgeConfig) -> Self {
        Self {
            binary: binary.into(),
            shared: Arc::new(BridgeShared {
                config: RwLock::new(config),
                list_cache: tokio::sync::Mutex::new(ListProjectsCache {
                    at: None,
                    rows: Vec::new(),
                }),
            }),
        }
    }

    pub fn config(&self) -> MemoryBridgeConfig {
        self.shared
            .config
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn set_http_port(&self, port: u16) {
        if port == 0 {
            return;
        }
        let mut guard = self
            .shared
            .config
            .write()
            .unwrap_or_else(|p| p.into_inner());
        guard.http_port = port;
    }

    pub fn http_port(&self) -> u16 {
        self.config().http_port
    }

    pub fn set_transport(&self, transport: TransportMode) {
        let mut guard = self
            .shared
            .config
            .write()
            .unwrap_or_else(|p| p.into_inner());
        guard.transport = transport;
    }

    pub fn binary_path(&self) -> &Path {
        &self.binary
    }

    pub fn diagnose(&self) -> ServiceHealth {
        let found = self.binary.is_file();
        ServiceHealth {
            id: ServiceId::MemoryBridge,
            name: "Memory Bridge".into(),
            running: found,
            started_by_us: false,
            endpoint: self.binary.display().to_string(),
            detail: found.then(|| "codebase-memory-mcp hazır".into()),
            error: (!found).then(|| format!("binary yok: {}", self.binary.display())),
        }
    }

    /// `bridge/codebase-memory-mcp` binary'sini `std::process::Command` ile tetikler.
    pub async fn index_workspace(&self, path: String) -> Result<IndexGraph> {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            bail!("index_workspace path boş");
        }
        let repo_path = PathBuf::from(trimmed)
            .canonicalize()
            .with_context(|| format!("repo_path çözümlenemedi: {trimmed}"))?;
        let repo = repo_path.to_str().context("repo_path UTF-8 değil")?;
        let stdout = self
            .run_tool(&[
                "index_repository",
                "--repo-path",
                repo,
                "--mode",
                "fast",
                "--format",
                "json",
            ])
            .await?;
        // Index başarılı → list_projects cache bayat.
        self.invalidate_list_cache().await;
        let mut graph = parse_index_graph(&stdout, &repo_path)?;
        // index_repository yalnızca nodes/edges sayımı döner; gerçek CALLS kenarları query_graph'tan.
        if graph.nodes.is_empty() || graph.references.is_empty() {
            if let Err(err) = self.enrich_from_call_graph(&mut graph).await {
                log::warn!("semantic map call-graph enrich atlandı: {err}");
            }
        }
        Ok(graph)
    }

    /// codebase-memory-mcp `query_graph` ile Function/Method düğümleri + CALLS (caller→callee).
    async fn enrich_from_call_graph(&self, graph: &mut IndexGraph) -> Result<()> {
        let project = graph.project.trim();
        if project.is_empty() {
            return Ok(());
        }

        if graph.nodes.is_empty() {
            let nodes = self.fetch_ast_nodes(project).await?;
            if !nodes.is_empty() {
                graph.nodes = nodes;
            }
        }
        if graph.references.is_empty() {
            let edges = self.fetch_call_edges(project).await?;
            if !edges.is_empty() {
                graph.references = edges;
            }
        }

        apply_ref_counts(&mut graph.nodes, &graph.references);
        if graph.dead.is_empty() && (!graph.nodes.is_empty() || !graph.references.is_empty()) {
            graph.dead = derive_dead(&graph.nodes, &graph.references, project);
            for item in &mut graph.dead {
                if item.project_id.is_none() {
                    item.project_id = Some(project.to_string());
                }
            }
        }
        // EX-14: truncated LIMIT lists must not overwrite real graph totals.
        if let Ok((nodes, edges)) = self.fetch_graph_totals(project).await {
            if nodes > 0 {
                graph.node_count = graph.node_count.max(nodes);
            }
            if edges > 0 {
                graph.edge_count = graph.edge_count.max(edges);
            }
        }
        if graph.node_count == 0 {
            graph.node_count = graph.nodes.len() as u64;
        }
        if graph.edge_count == 0 {
            graph.edge_count = graph.references.len() as u64;
        }
        if graph.files.unwrap_or(0) == 0 {
            graph.files = Some(graph.unique_file_count());
        }
        Ok(())
    }

    /// Full graph counts via Cypher COUNT (not LIMIT-shaped list lengths).
    async fn fetch_graph_totals(&self, project: &str) -> Result<(u64, u64)> {
        let mut nodes: u64 = 0;
        for label in ["Function", "Method"] {
            let query = format!("MATCH (f:{label}) RETURN count(f) AS c");
            let payload = self.query_graph_json(project, &query).await?;
            nodes = nodes.saturating_add(count_from_query(&payload));
        }
        let edge_payload = self
            .query_graph_json(project, "MATCH ()-[r:CALLS]->() RETURN count(r) AS c")
            .await?;
        let edges = count_from_query(&edge_payload);
        Ok((nodes, edges))
    }

    async fn fetch_ast_nodes(&self, project: &str) -> Result<Vec<AstNode>> {
        let mut nodes = Vec::new();
        for (label, kind) in [("Function", "function"), ("Method", "method")] {
            let query = format!(
                "MATCH (f:{label}) RETURN f.name AS name, f.qualified_name AS id, \
                 f.file_path AS file, f.start_line AS line LIMIT {SEMANTIC_NODE_LIMIT}"
            );
            let payload = self.query_graph_json(project, &query).await?;
            nodes.extend(ast_nodes_from_query(&payload, kind));
            if nodes.len() >= SEMANTIC_NODE_LIMIT as usize {
                nodes.truncate(SEMANTIC_NODE_LIMIT as usize);
                break;
            }
        }
        Ok(nodes)
    }

    async fn fetch_call_edges(&self, project: &str) -> Result<Vec<CodeReference>> {
        let query = format!(
            "MATCH (caller)-[r:CALLS]->(callee) \
             RETURN caller.name AS caller, callee.name AS callee, \
             caller.qualified_name AS from_id, callee.qualified_name AS to_id, \
             caller.file_path AS file, r.line AS line LIMIT {SEMANTIC_CALL_LIMIT}"
        );
        let payload = self.query_graph_json(project, &query).await?;
        Ok(call_edges_from_query(&payload))
    }

    async fn query_graph_json(&self, project: &str, query: &str) -> Result<Value> {
        let stdout = self
            .run_tool(&[
                "query_graph",
                "--project",
                project,
                "--query",
                query,
                "--format",
                "json",
            ])
            .await?;
        parse_cli_json(&stdout)
    }

    pub async fn index_repository(&self, repo_path: impl AsRef<Path>) -> Result<IndexSnapshot> {
        Ok(self
            .index_workspace(repo_path.as_ref().to_string_lossy().into_owned())
            .await?
            .snapshot())
    }

    pub async fn get_dead_symbols(&self, repo_path: impl AsRef<Path>) -> Result<Vec<DeadSymbol>> {
        let repo_path = repo_path
            .as_ref()
            .canonicalize()
            .with_context(|| "repo_path çözümlenemedi")?;
        if let Some(dead) = self.try_cli_dead(&repo_path).await? {
            if !dead.is_empty() {
                return Ok(dead);
            }
        }
        Ok(self
            .index_workspace(repo_path.to_string_lossy().into_owned())
            .await?
            .dead)
    }

    pub async fn list_projects(&self) -> Result<Vec<ProjectSummary>> {
        {
            let cache = self.shared.list_cache.lock().await;
            if let Some(at) = cache.at {
                if at.elapsed() < LIST_PROJECTS_CACHE_TTL {
                    return Ok(cache.rows.clone());
                }
            }
        }
        let rows = self.list_projects_uncached().await?;
        let mut cache = self.shared.list_cache.lock().await;
        cache.at = Some(Instant::now());
        cache.rows = rows.clone();
        Ok(rows)
    }

    pub async fn list_projects_uncached(&self) -> Result<Vec<ProjectSummary>> {
        let stdout = self
            .run_tool(&["list_projects", "--format", "json"])
            .await?;
        let payload = parse_cli_json(&stdout)?;
        let list: ProjectList =
            serde_json::from_value(payload).context("project listesi çözülemedi")?;
        Ok(list.projects)
    }

    pub async fn invalidate_list_cache(&self) {
        let mut cache = self.shared.list_cache.lock().await;
        cache.at = None;
        cache.rows.clear();
    }

    /// Test yardımcısı: cache TTL durumunu oku.
    pub async fn list_projects_cache_age(&self) -> Option<Duration> {
        let cache = self.shared.list_cache.lock().await;
        cache.at.map(|at| at.elapsed())
    }

    async fn try_cli_dead(&self, repo_path: &Path) -> Result<Option<Vec<DeadSymbol>>> {
        let repo = match repo_path.to_str() {
            Some(path) => path,
            None => return Ok(None),
        };
        for tool in ["get_dead_symbols", "dead_symbols"] {
            match self
                .run_tool(&[tool, "--repo-path", repo, "--format", "json"])
                .await
            {
                Ok(stdout) => {
                    let payload = parse_cli_json(&stdout)?;
                    let dead = parse_dead_from_value(&payload, project_name(repo_path, &payload));
                    if !dead.is_empty() {
                        return Ok(Some(dead));
                    }
                }
                Err(err) => {
                    log::debug!("{tool} yok veya başarısız: {err}");
                }
            }
        }
        Ok(None)
    }

    /// HTTP `/rpc` (Auto + UI up) veya CLI. Mutating tool'da timeout sonrası CLI yok.
    pub async fn run_tool(&self, tool_args: &[&str]) -> Result<String> {
        let tool_name = tool_args.first().copied().unwrap_or("");
        let cfg = self.config();
        let timeout = tool_timeout_for(&cfg, tool_name);
        let mutating = is_mutating_tool(tool_name);

        match self.select_transport().await {
            ToolTransport::HttpRpc => match self.run_http_rpc(tool_args, timeout).await {
                Ok(out) => Ok(out),
                Err(err) if should_fallback_to_cli(mutating, &err) => {
                    log::warn!("HTTP /rpc bağlantı yok, CLI fallback: {err}");
                    self.run_cli_with_timeout(tool_args, timeout).await
                }
                Err(err) if !mutating => {
                    log::warn!("HTTP /rpc başarısız, CLI fallback (read): {err}");
                    self.run_cli_with_timeout(tool_args, timeout).await
                }
                Err(err) => {
                    // Mutating: timeout / partial failure — ikinci kez indexleme yok.
                    Err(err)
                }
            },
            ToolTransport::Cli => self.run_cli_with_timeout(tool_args, timeout).await,
        }
    }

    pub async fn select_transport(&self) -> ToolTransport {
        let cfg = self.config();
        match cfg.transport {
            TransportMode::ForceCli => ToolTransport::Cli,
            TransportMode::Auto => select_tool_transport(cfg.http_port).await,
        }
    }

    async fn run_http_rpc(&self, tool_args: &[&str], timeout: Duration) -> Result<String> {
        let port = self.http_port();
        let (name, arguments) = tool_args_to_rpc(tool_args)?;
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments,
            }
        });
        let url = format!("http://127.0.0.1:{port}/rpc");
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .context("reqwest client")?;
        let response = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|err| classify_reqwest_err(err, &url))?;
        if !response.status().is_success() {
            bail!("HTTP /rpc status {}", response.status());
        }
        let payload: Value = response.json().await.context("HTTP /rpc JSON")?;
        extract_rpc_tool_text(&payload)
    }

    pub async fn run_cli_with_timeout(
        &self,
        tool_args: &[&str],
        timeout: Duration,
    ) -> Result<String> {
        if !self.binary.is_file() {
            bail!("codebase-memory-mcp yok: {}", self.binary.display());
        }

        let binary = self.binary.clone();
        let args: Vec<String> = tool_args.iter().map(|arg| (*arg).to_string()).collect();
        let pid = Arc::new(AtomicU32::new(0));
        let pid_for_child = pid.clone();

        // Tokio runtime'ı bloklamamak için GuardedCommand → std Command spawn_blocking içinde.
        let wait = tokio::task::spawn_blocking(move || {
            let mut command = GuardedCommand::new(&binary)
                .arg("cli")
                .args(&args)
                .internal_daemon()
                .into_std_command()
                .with_context(|| format!("subprocess gate başarısız: {}", binary.display()))?;
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x0800_0000);
            }
            let child = command
                .spawn()
                .with_context(|| format!("subprocess başarısız: {}", binary.display()))?;
            pid_for_child.store(child.id(), Ordering::SeqCst);
            child.wait_with_output().context("wait_with_output")
        });

        let output = match tokio::time::timeout(timeout, wait).await {
            Ok(join) => join.context("codebase-memory-mcp join")??,
            Err(_) => {
                let child_pid = pid.load(Ordering::SeqCst);
                if child_pid != 0 {
                    kill_pid(child_pid);
                }
                bail!("codebase-memory-mcp zaman aşımı ({timeout:?})");
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            bail!(
                "codebase-memory-mcp {} ile çıktı. stderr: {}",
                output.status,
                stderr.trim()
            );
        }

        if stdout.trim().is_empty() {
            bail!("codebase-memory-mcp stdout boş. stderr: {}", stderr.trim());
        }

        Ok(stdout)
    }
}

/// Index / yazma araçları — uzun timeout; HTTP timeout sonrası CLI yok.
pub fn is_mutating_tool(name: &str) -> bool {
    matches!(
        name,
        "index_repository"
            | "index_workspace"
            | "delete_project"
            | "remove_project"
            | "clear_project"
            | "reindex"
    )
}

pub fn tool_timeout_for(config: &MemoryBridgeConfig, tool_name: &str) -> Duration {
    if is_mutating_tool(tool_name) {
        config.mutating_timeout
    } else {
        config.read_timeout
    }
}

/// Mutating tool: yalnızca connection refused / port closed → CLI.
/// Timeout veya kısmi HTTP hatası → CLI yok.
pub fn should_fallback_to_cli(mutating: bool, err: &anyhow::Error) -> bool {
    if !mutating {
        return true;
    }
    is_connection_refused_or_unreachable(err)
}

pub fn is_connection_refused_or_unreachable(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(re) = cause.downcast_ref::<reqwest::Error>() {
            if re.is_timeout() {
                return false;
            }
            if re.is_connect() {
                return true;
            }
        }
        let msg = cause.to_string().to_lowercase();
        if msg.contains("timed out") || msg.contains("timeout") || msg.contains("zaman aşımı") {
            return false;
        }
        if msg.contains("connection refused")
            || msg.contains("connect error")
            || msg.contains("dns error")
            || msg.contains("network unreachable")
            || msg.contains("connection reset")
        {
            return true;
        }
    }
    false
}

fn classify_reqwest_err(err: reqwest::Error, url: &str) -> anyhow::Error {
    if err.is_timeout() {
        anyhow::anyhow!("HTTP /rpc zaman aşımı ({url}): {err}")
    } else if err.is_connect() {
        anyhow::anyhow!("HTTP /rpc connection refused ({url}): {err}")
    } else {
        anyhow::anyhow!("POST {url}: {err}")
    }
}

/// `GET /api/ui-config` → 200 + JSON ise Graph UI ayakta.
pub async fn probe_ui_config(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/api/ui-config");
    let Ok(client) = reqwest::Client::builder().timeout(UI_PROBE_TIMEOUT).build() else {
        return false;
    };
    let Ok(response) = client.get(&url).send().await else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    response.json::<Value>().await.is_ok()
}

pub async fn select_tool_transport(port: u16) -> ToolTransport {
    if probe_ui_config(port).await {
        ToolTransport::HttpRpc
    } else {
        ToolTransport::Cli
    }
}

/// CLI argümanlarını JSON-RPC `tools/call` params'a çevirir (`--format` atlanır).
pub fn tool_args_to_rpc(tool_args: &[&str]) -> Result<(String, Value)> {
    let Some((name, rest)) = tool_args.split_first() else {
        bail!("tool adı yok");
    };
    if name.trim().is_empty() {
        bail!("tool adı boş");
    }
    let mut map = serde_json::Map::new();
    let mut idx = 0;
    while idx < rest.len() {
        let token = rest[idx];
        let Some(flag) = token.strip_prefix("--") else {
            idx += 1;
            continue;
        };
        if flag == "format" {
            idx += if idx + 1 < rest.len() && !rest[idx + 1].starts_with("--") {
                2
            } else {
                1
            };
            continue;
        }
        let key = flag.replace('-', "_");
        if idx + 1 < rest.len() && !rest[idx + 1].starts_with("--") {
            map.insert(key, Value::String(rest[idx + 1].to_string()));
            idx += 2;
        } else {
            map.insert(key, Value::Bool(true));
            idx += 1;
        }
    }
    Ok(((*name).to_string(), Value::Object(map)))
}

/// JSON-RPC tools/call yanıtından `result.content[0].text` (veya hata) çıkarır.
pub fn extract_rpc_tool_text(payload: &Value) -> Result<String> {
    if let Some(err) = payload.get("error") {
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("rpc error");
        let code = err.get("code").and_then(Value::as_i64);
        bail!(
            "JSON-RPC error{}: {message}",
            code.map(|c| format!(" ({c})")).unwrap_or_default()
        );
    }
    if let Some(text) = payload
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
    {
        return Ok(text.to_string());
    }
    if let Some(result) = payload.get("result") {
        return serde_json::to_string(result).context("rpc result serialize");
    }
    bail!("JSON-RPC yanıtında result yok");
}

fn kill_pid(pid: u32) {
    #[cfg(unix)]
    {
        let _ = GuardedCommand::new("kill")
            .args(["-KILL", &pid.to_string()])
            .internal_daemon()
            .status();
    }
    #[cfg(windows)]
    {
        let _ = GuardedCommand::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .internal_daemon()
            .status();
    }
}

fn resolve_binary(repo_root: &Path) -> Result<PathBuf> {
    if let Ok(from_env) = std::env::var("LOUNGE_MEMORY_BIN") {
        return Ok(PathBuf::from(from_env));
    }

    let mut candidates = Vec::new();

    // Tauri 2 externalBin: paket içinde sidecar, ana exe ile aynı dizinde (triple yok).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("codebase-memory-mcp"));
            candidates.push(dir.join("codebase-memory-mcp.exe"));
        }
    }

    candidates.push(repo_root.join("bridge/codebase-memory-mcp"));
    candidates.push(repo_root.join("bridge/codebase-memory-mcp.exe"));

    // Yerel paketleme / prepare-sidecar: binaries/codebase-memory-mcp-$TARGET_TRIPLE
    let binaries_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries");
    candidates.push(binaries_dir.join("codebase-memory-mcp"));
    candidates.push(binaries_dir.join("codebase-memory-mcp.exe"));
    candidates.push(binaries_dir.join(packaged_sidecar_filename()));

    if let Some(on_path) = find_executable("codebase-memory-mcp") {
        candidates.push(on_path);
    }

    // Skip build.rs / prepare-sidecar --stub placeholders (tiny shell scripts).
    first_existing(candidates.into_iter().filter(|path| !is_compile_stub(path))).ok_or_else(|| {
        anyhow::anyhow!("codebase-memory-mcp bulunamadı (sidecar, bridge/ veya PATH)")
    })
}

/// Tauri externalBin compile stubs are <4KB shell scripts that exit 127.
fn is_compile_stub(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() >= 4096 {
        return false;
    }
    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let text = String::from_utf8_lossy(&bytes);
    text.contains("codebase-memory-mcp stub")
}

/// `bundle.externalBin` ile aynı isimlendirme: name-target_triple[.exe]
fn packaged_sidecar_filename() -> String {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "codebase-memory-mcp-aarch64-apple-darwin".into()
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "codebase-memory-mcp-x86_64-apple-darwin".into()
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "codebase-memory-mcp-x86_64-pc-windows-msvc.exe".into()
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "codebase-memory-mcp-aarch64-pc-windows-msvc.exe".into()
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "codebase-memory-mcp-x86_64-unknown-linux-gnu".into()
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "codebase-memory-mcp-aarch64-unknown-linux-gnu".into()
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
    )))]
    {
        "codebase-memory-mcp".into()
    }
}

pub fn parse_index_stdout(stdout: &str) -> Result<IndexSnapshot> {
    Ok(parse_index_graph(stdout, Path::new(""))?.snapshot())
}

pub fn parse_index_graph(stdout: &str, repo_path: &Path) -> Result<IndexGraph> {
    let payload = parse_cli_json(stdout)?;
    index_graph_from_value(payload, repo_path)
}

pub fn parse_cli_json(stdout: &str) -> Result<Value> {
    let json_slice = extract_json_slice(stdout).ok_or_else(|| {
        anyhow::anyhow!("stdout içinde JSON nesnesi yok: {}", truncate(stdout, 240))
    })?;
    let value: Value = serde_json::from_str(json_slice)
        .with_context(|| format!("JSON parse edilemedi: {}", truncate(json_slice, 240)))?;
    Ok(unwrap_cli_envelope(value))
}

fn unwrap_cli_envelope(value: Value) -> Value {
    if let Some(result) = value.get("result") {
        return unwrap_cli_envelope(result.clone());
    }

    if let Some(text) = value
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
    {
        if let Ok(inner) = serde_json::from_str::<Value>(text) {
            return unwrap_cli_envelope(inner);
        }
    }

    value
}

fn index_graph_from_value(value: Value, repo_path: &Path) -> Result<IndexGraph> {
    let project = project_name(repo_path, &value);
    let mut nodes = parse_node_array(&value);
    let references = parse_reference_array(&value);
    let node_count = count_field(&value, &["nodes", "ast_nodes"]).max(nodes.len() as u64);
    let edge_count = count_field(&value, &["edges", "references"]).max(references.len() as u64);
    let files = value
        .get("files")
        .and_then(json_u64)
        .or_else(|| value.get("file_count").and_then(json_u64));
    let status = json_str(&value, &["status"]);

    apply_ref_counts(&mut nodes, &references);
    let mut dead = parse_dead_from_value(&value, project.clone());
    if dead.is_empty() && (!nodes.is_empty() || !references.is_empty()) {
        dead = derive_dead(&nodes, &references, &project);
    }
    for item in &mut dead {
        if item.project_id.is_none() {
            item.project_id = Some(project.clone());
        }
    }

    Ok(IndexGraph {
        project,
        repo_path: repo_path.display().to_string(),
        status,
        node_count,
        edge_count,
        files,
        nodes,
        references,
        dead,
    })
}

fn project_name(repo_path: &Path, value: &Value) -> String {
    if let Some(name) = json_str(value, &["project", "name", "project_id"]) {
        if !name.is_empty() {
            return name;
        }
    }
    repo_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string()
}

fn count_field(value: &Value, keys: &[&str]) -> u64 {
    for key in keys {
        if let Some(item) = value.get(*key) {
            if let Some(n) = json_u64(item) {
                return n;
            }
            if let Some(items) = item.as_array() {
                return items.len() as u64;
            }
        }
    }
    0
}

/// Extract `c` / `count` from a Cypher COUNT query JSON payload.
fn count_from_query(payload: &Value) -> u64 {
    if let Some(n) = json_u64_at(payload, &["c"])
        .or_else(|| json_u64_at(payload, &["count"]))
        .or_else(|| json_u64(payload))
    {
        return n;
    }
    // Common shapes: { "rows": [ { "c": N } ] } or [ { "c": N } ]
    if let Some(rows) = payload
        .get("rows")
        .or_else(|| payload.get("results"))
        .or_else(|| payload.get("data"))
        .and_then(Value::as_array)
    {
        if let Some(first) = rows.first() {
            if let Some(n) = json_u64_at(first, &["c"])
                .or_else(|| json_u64_at(first, &["count"]))
                .or_else(|| json_u64(first))
            {
                return n;
            }
        }
    }
    if let Some(arr) = payload.as_array() {
        if let Some(first) = arr.first() {
            if let Some(n) = json_u64_at(first, &["c"])
                .or_else(|| json_u64_at(first, &["count"]))
                .or_else(|| json_u64(first))
            {
                return n;
            }
        }
    }
    0
}

fn json_u64_at(value: &Value, keys: &[&str]) -> Option<u64> {
    let mut cur = value;
    for key in keys {
        cur = cur.get(*key)?;
    }
    json_u64(cur)
}

fn parse_node_array(value: &Value) -> Vec<AstNode> {
    let mut nodes = Vec::new();
    for key in ["ast_nodes", "nodes", "symbols"] {
        let Some(items) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            if let Some(node) = ast_node_from_value(item) {
                nodes.push(node);
            }
        }
        if !nodes.is_empty() {
            break;
        }
    }
    nodes
}

fn ast_node_from_value(value: &Value) -> Option<AstNode> {
    if !value.is_object() {
        return None;
    }
    let id = json_str(value, &["id", "name", "symbol"]).unwrap_or_default();
    let name = json_str(value, &["name", "symbol", "id"]).unwrap_or_else(|| id.clone());
    if id.is_empty() && name.is_empty() {
        return None;
    }
    Some(AstNode {
        id: if id.is_empty() { name.clone() } else { id },
        name,
        kind: json_str(value, &["kind", "type"]).unwrap_or_default(),
        file: json_str(value, &["file", "path", "file_path"]),
        line: json_i64(value, &["line", "lineno", "loc"]),
        ref_count: value.get("ref_count").and_then(json_u64).unwrap_or(0),
    })
}

fn parse_reference_array(value: &Value) -> Vec<CodeReference> {
    let mut refs = Vec::new();
    for key in ["references", "edges", "refs"] {
        let Some(items) = value.get(key).and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            if let Some(edge) = reference_from_value(item) {
                refs.push(edge);
            }
        }
        if !refs.is_empty() {
            break;
        }
    }
    refs
}

fn reference_from_value(value: &Value) -> Option<CodeReference> {
    if !value.is_object() {
        return None;
    }
    // source_id/target_id CBM kenar kimlikleri; callee property kısa isim olabilir — sonda fallback.
    let from_id =
        json_str(value, &["from_id", "source_id", "from", "source", "caller"]).unwrap_or_default();
    let mut to_id = json_str(value, &["to_id", "target_id", "to", "target"]).unwrap_or_default();
    if to_id.is_empty() {
        to_id = json_str(value, &["callee"]).unwrap_or_default();
    }
    if from_id.is_empty() && to_id.is_empty() {
        return None;
    }
    Some(CodeReference {
        from_id,
        to_id,
        file: json_str(value, &["file", "path", "file_path"]),
        line: json_i64(value, &["line", "lineno", "loc"]),
    })
}

/// `query_graph` satır tablosu → AST düğümleri.
fn ast_nodes_from_query(value: &Value, default_kind: &str) -> Vec<AstNode> {
    let rows = query_graph_maps(value);
    let mut nodes = Vec::with_capacity(rows.len());
    for row in rows {
        let id = row_str(&row, &["id", "qualified_name", "name"]).unwrap_or_default();
        let name = row_str(&row, &["name", "id"]).unwrap_or_else(|| id.clone());
        if id.is_empty() && name.is_empty() {
            continue;
        }
        nodes.push(AstNode {
            id: if id.is_empty() { name.clone() } else { id },
            name,
            kind: row_str(&row, &["kind", "label", "type"]).unwrap_or_else(|| default_kind.into()),
            file: row_str(&row, &["file", "file_path", "path"]),
            line: row_i64(&row, &["line", "start_line", "lineno"]),
            ref_count: 0,
        });
    }
    nodes
}

/// `query_graph` CALLS satırları → caller/callee referansları (`from_id`/`to_id`).
fn call_edges_from_query(value: &Value) -> Vec<CodeReference> {
    let rows = query_graph_maps(value);
    let mut edges = Vec::with_capacity(rows.len());
    for row in rows {
        let from_id =
            row_str(&row, &["from_id", "caller", "source_id", "source"]).unwrap_or_default();
        let to_id = row_str(&row, &["to_id", "callee", "target_id", "target"]).unwrap_or_default();
        if from_id.is_empty() && to_id.is_empty() {
            continue;
        }
        edges.push(CodeReference {
            from_id,
            to_id,
            file: row_str(&row, &["file", "file_path", "path"]),
            line: row_i64(&row, &["line", "lineno", "loc"]),
        });
    }
    edges
}

fn query_graph_maps(value: &Value) -> Vec<HashMap<String, Value>> {
    let columns: Vec<String> = value
        .get("columns")
        .and_then(Value::as_array)
        .map(|cols| {
            cols.iter()
                .filter_map(|col| col.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let Some(rows) = value.get("rows").and_then(Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let cells = row.as_array()?;
            let mut map = HashMap::new();
            for (idx, cell) in cells.iter().enumerate() {
                let key = columns
                    .get(idx)
                    .cloned()
                    .unwrap_or_else(|| format!("c{idx}"));
                map.insert(key, cell.clone());
            }
            Some(map)
        })
        .collect()
}

fn row_str(row: &HashMap<String, Value>, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(item) = row.get(*key) {
            if let Some(text) = item.as_str() {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
            if let Some(n) = item.as_i64() {
                return Some(n.to_string());
            }
            if let Some(n) = item.as_u64() {
                return Some(n.to_string());
            }
        }
    }
    None
}

fn row_i64(row: &HashMap<String, Value>, keys: &[&str]) -> Option<i64> {
    for key in keys {
        if let Some(item) = row.get(*key) {
            if let Some(n) = item.as_i64() {
                return Some(n);
            }
            if let Some(n) = item.as_u64() {
                return Some(n as i64);
            }
            if let Some(text) = item.as_str() {
                if let Ok(n) = text.parse::<i64>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

pub fn parse_dead_from_value(value: &Value, project: String) -> Vec<DeadSymbol> {
    let mut dead = Vec::new();
    for key in ["dead_symbols", "unused", "dead"] {
        if let Some(items) = value.get(key).and_then(Value::as_array) {
            for item in items {
                if let Some(mut symbol) = dead_symbol_from_value(item, "unused") {
                    if symbol.project_id.is_none() {
                        symbol.project_id = Some(project.clone());
                    }
                    dead.push(symbol);
                }
            }
        }
    }
    if let Some(items) = value.get("broken_references").and_then(Value::as_array) {
        for item in items {
            if let Some(mut symbol) = dead_symbol_from_value(item, "broken") {
                if symbol.project_id.is_none() {
                    symbol.project_id = Some(project.clone());
                }
                dead.push(symbol);
            }
        }
    }
    dead
}

fn dead_symbol_from_value(value: &Value, default_kind: &str) -> Option<DeadSymbol> {
    if let Some(name) = value.as_str() {
        return Some(DeadSymbol {
            name: name.to_string(),
            kind: default_kind.into(),
            ..DeadSymbol::default()
        });
    }
    if !value.is_object() {
        return None;
    }
    let name = json_str(value, &["name", "symbol", "id", "to", "target"]).unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    let raw_kind = json_str(value, &["kind", "type"]).unwrap_or_else(|| default_kind.to_string());
    let kind = if raw_kind.contains("broken") {
        "broken".to_string()
    } else {
        "unused".to_string()
    };
    Some(DeadSymbol {
        name,
        kind,
        file: json_str(value, &["file", "path", "file_path"]),
        line: json_i64(value, &["line", "lineno", "loc"]),
        detail: json_str(value, &["detail", "reason", "message"]),
        project_id: json_str(value, &["project_id", "project"]),
    })
}

fn apply_ref_counts(nodes: &mut [AstNode], references: &[CodeReference]) {
    let mut incoming: HashMap<String, u64> = HashMap::new();
    for edge in references {
        if !edge.to_id.is_empty() {
            *incoming.entry(edge.to_id.clone()).or_default() += 1;
        }
    }
    for node in nodes {
        let count = incoming
            .get(&node.id)
            .copied()
            .or_else(|| incoming.get(&node.name).copied())
            .unwrap_or(0);
        if node.ref_count == 0 {
            node.ref_count = count;
        }
    }
}

pub fn derive_dead(
    nodes: &[AstNode],
    references: &[CodeReference],
    project: &str,
) -> Vec<DeadSymbol> {
    let mut ids = HashSet::new();
    for node in nodes {
        if !node.id.is_empty() {
            ids.insert(node.id.clone());
        }
        if !node.name.is_empty() {
            ids.insert(node.name.clone());
        }
    }

    let mut dead = Vec::new();
    let mut seen_broken = HashSet::new();
    for edge in references {
        if edge.to_id.is_empty() || ids.contains(&edge.to_id) {
            continue;
        }
        if !seen_broken.insert(edge.to_id.clone()) {
            continue;
        }
        dead.push(DeadSymbol {
            name: edge.to_id.clone(),
            kind: "broken".into(),
            file: edge.file.clone(),
            line: edge.line,
            detail: Some(format!("{} → {} hedefi yok", edge.from_id, edge.to_id)),
            project_id: Some(project.to_string()),
        });
    }

    if !nodes.is_empty() {
        for node in nodes {
            if node.ref_count > 0 {
                continue;
            }
            dead.push(DeadSymbol {
                name: if node.name.is_empty() {
                    node.id.clone()
                } else {
                    node.name.clone()
                },
                kind: "unused".into(),
                file: node.file.clone(),
                line: node.line,
                detail: Some("gelen referans yok".into()),
                project_id: Some(project.to_string()),
            });
        }
    }

    dead
}

fn json_str(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(item) = value.get(*key) {
            if let Some(text) = item.as_str() {
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
            if let Some(n) = item.as_i64() {
                return Some(n.to_string());
            }
        }
    }
    None
}

fn json_i64(value: &Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        if let Some(item) = value.get(*key) {
            if let Some(n) = item.as_i64() {
                return Some(n);
            }
            if let Some(n) = item.as_u64() {
                return Some(n as i64);
            }
            if let Some(text) = item.as_str() {
                if let Ok(n) = text.parse::<i64>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| value.as_f64().map(|n| n as u64))
}

fn extract_json_slice(stdout: &str) -> Option<&str> {
    let start = stdout.find('{')?;
    let slice = stdout.get(start..)?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (idx, ch) in slice.char_indices() {
        match ch {
            '\\' if in_string => escape = !escape,
            '"' if !escape => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&slice[..=idx]);
                }
            }
            _ => escape = false,
        }
        if ch != '\\' {
            escape = false;
        }
    }
    None
}

fn truncate(input: &str, max: usize) -> String {
    let trimmed = input.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    format!("{}…", trimmed.chars().take(max).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direct_index_payload() {
        let stdout =
            r#"{"project":"agent-lounge-os","status":"indexed","nodes":42,"edges":7,"files":12}"#;
        let snap = parse_index_stdout(stdout).unwrap();
        assert_eq!(snap.project, "agent-lounge-os");
        assert_eq!(snap.nodes, 42);
        assert_eq!(snap.edges, 7);
        assert_eq!(snap.files, Some(12));
        assert_eq!(snap.status.as_deref(), Some("indexed"));
        assert_eq!(snap.dead, 0);
    }

    #[test]
    fn unwraps_mcp_content_envelope() {
        let stdout = r#"{"content":[{"type":"text","text":"{\"project\":\"crm\",\"nodes\":9540,\"edges\":27046}"}]}"#;
        let snap = parse_index_stdout(stdout).unwrap();
        assert_eq!(snap.project, "crm");
        assert_eq!(snap.nodes, 9540);
        assert_eq!(snap.edges, 27046);
    }

    #[test]
    fn unwraps_jsonrpc_result_envelope() {
        let stdout = r#"{"jsonrpc":"2.0","result":{"content":[{"type":"text","text":"{\"name\":\"hotspot\",\"nodes\":1,\"edges\":2}"}]}}"#;
        let snap = parse_index_stdout(stdout).unwrap();
        assert_eq!(snap.project, "hotspot");
        assert_eq!(snap.nodes, 1);
    }

    #[test]
    fn ignores_stderr_style_prefix_before_json() {
        let stdout = "level=info msg=mem.init\n{\"project\":\"x\",\"nodes\":3,\"edges\":1}\n";
        let snap = parse_index_stdout(stdout).unwrap();
        assert_eq!(snap.nodes, 3);
    }

    #[test]
    fn rejects_empty_stdout() {
        assert!(parse_index_stdout("   ").is_err());
    }

    #[test]
    fn parses_ast_nodes_and_derives_unused_and_broken() {
        let stdout = r#"{
            "project":"lounge",
            "ast_nodes":[
                {"id":"foo","name":"foo","kind":"fn","file":"src/lib.rs","line":10},
                {"id":"bar","name":"bar","kind":"fn","file":"src/dead.rs","line":4}
            ],
            "references":[
                {"from":"main","to":"foo","file":"src/main.rs","line":1},
                {"from":"foo","to":"ghost","file":"src/lib.rs","line":12}
            ]
        }"#;
        let graph = parse_index_graph(stdout, Path::new("/tmp/lounge")).unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.references.len(), 2);
        let foo = graph.nodes.iter().find(|node| node.name == "foo").unwrap();
        assert_eq!(foo.ref_count, 1);
        let unused: Vec<_> = graph
            .dead
            .iter()
            .filter(|row| row.kind == "unused")
            .map(|row| row.name.as_str())
            .collect();
        let broken: Vec<_> = graph
            .dead
            .iter()
            .filter(|row| row.kind == "broken")
            .map(|row| row.name.as_str())
            .collect();
        assert_eq!(unused, vec!["bar"]);
        assert_eq!(broken, vec!["ghost"]);
        assert_eq!(graph.snapshot().dead, 2);
    }

    #[test]
    fn uses_explicit_dead_symbols_when_present() {
        let stdout = r#"{"project":"x","dead_symbols":[{"name":"orphan","kind":"unused"}],"broken_references":[{"name":"lost","kind":"broken"}]}"#;
        let graph = parse_index_graph(stdout, Path::new("")).unwrap();
        assert_eq!(graph.dead.len(), 2);
        assert!(graph
            .dead
            .iter()
            .any(|row| row.kind == "unused" && row.name == "orphan"));
        assert!(graph
            .dead
            .iter()
            .any(|row| row.kind == "broken" && row.name == "lost"));
    }

    #[test]
    fn parses_query_graph_calls_as_caller_callee_edges() {
        let payload: Value = serde_json::from_str(
            r#"{
              "columns":["caller","callee","from_id","to_id","file","line"],
              "rows":[
                ["AppShell","useLounge","qn.AppShell","qn.useLounge","src/app-shell.tsx","70"],
                ["main","ghost","qn.main","qn.ghost","src/main.rs",""]
              ]
            }"#,
        )
        .unwrap();
        let edges = call_edges_from_query(&payload);
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0].from_id, "qn.AppShell");
        assert_eq!(edges[0].to_id, "qn.useLounge");
        assert_eq!(edges[0].file.as_deref(), Some("src/app-shell.tsx"));
        assert_eq!(edges[0].line, Some(70));
        assert_eq!(edges[1].from_id, "qn.main");
        assert_eq!(edges[1].to_id, "qn.ghost");
    }

    #[test]
    fn parses_query_graph_functions_as_ast_nodes() {
        let payload: Value = serde_json::from_str(
            r#"{
              "columns":["name","id","file","line"],
              "rows":[["VaultPanel","qn.VaultPanel","src/components/panels.tsx","330"]]
            }"#,
        )
        .unwrap();
        let nodes = ast_nodes_from_query(&payload, "function");
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "VaultPanel");
        assert_eq!(nodes[0].id, "qn.VaultPanel");
        assert_eq!(nodes[0].kind, "function");
        assert_eq!(nodes[0].line, Some(330));
    }

    #[test]
    fn reference_prefers_source_target_ids_over_callee_prop() {
        let edge = reference_from_value(&serde_json::json!({
            "source_id": "caller.qn",
            "target_id": "callee.qn",
            "callee": "shortName",
            "file": "a.rs",
            "line": 3
        }))
        .unwrap();
        assert_eq!(edge.from_id, "caller.qn");
        assert_eq!(edge.to_id, "callee.qn");
        assert_eq!(edge.line, Some(3));
    }

    #[tokio::test]
    async fn rejects_empty_index_path() {
        let bridge = MemoryBridge::from_binary("/tmp/missing-codebase-memory-mcp");
        let err = bridge.index_workspace("  ".into()).await.unwrap_err();
        assert!(err.to_string().contains("path boş"));
    }

    #[tokio::test]
    async fn lists_projects_when_binary_present() {
        let Ok(bridge) = MemoryBridge::discover() else {
            return;
        };
        if !bridge.binary_path().is_file() || is_compile_stub(bridge.binary_path()) {
            return;
        }
        let projects = bridge.list_projects().await.expect("list_projects parse");
        assert!(projects.iter().all(|project| !project.name.is_empty()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn index_workspace_runs_std_process_command() {
        let dir = std::env::temp_dir().join(format!("lounge-cbm-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("codebase-memory-mcp");
        std::fs::write(
            &script,
            r#"#!/bin/sh
if printf '%s' "$*" | grep -q get_dead_symbols; then
  echo '{"dead_symbols":[{"name":"cli_dead","kind":"unused"}]}'
  exit 0
fi
echo '{"project":"demo","ast_nodes":[{"id":"live","name":"live"},{"id":"dead","name":"dead"}],"references":[{"from":"main","to":"live"},{"from":"live","to":"missing"}]}'
"#,
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let bridge = MemoryBridge::from_binary(&script);
        let graph = bridge
            .index_workspace(dir.to_string_lossy().into_owned())
            .await
            .expect("index");
        assert_eq!(graph.project, "demo");
        assert_eq!(graph.nodes.len(), 2);
        assert!(graph
            .dead
            .iter()
            .any(|row| row.name == "dead" && row.kind == "unused"));
        assert!(graph
            .dead
            .iter()
            .any(|row| row.name == "missing" && row.kind == "broken"));

        let cli_dead = bridge.get_dead_symbols(&dir).await.expect("dead");
        assert!(cli_dead.iter().any(|row| row.name == "cli_dead"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn tauri_command_path_indexes_protocol_crate() {
        let Ok(bridge) = MemoryBridge::discover() else {
            return;
        };
        if !bridge.binary_path().is_file() || is_compile_stub(bridge.binary_path()) {
            return;
        }
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace")
            .join("shared/lounge_protocol");
        let graph = bridge
            .index_workspace(repo.to_string_lossy().into_owned())
            .await
            .expect("index_workspace");
        assert!(
            !graph.project.is_empty(),
            "project adı boş: {:?}",
            graph.project
        );
        let store = crate::db::ExperienceStore::memory().unwrap();
        let snapshot = store
            .save_project_index(graph)
            .await
            .expect("save_project_index");
        assert_eq!(snapshot.status.as_deref(), Some("indexed"));
        assert!(snapshot.nodes >= 1, "nodes={}", snapshot.nodes);
        let _ = store.list_dead_symbols(None).await.expect("dead symbols");
        let map = store.load_semantic_map(None).await.expect("semantic map");
        assert!(
            !map.projects.is_empty() || snapshot.nodes > 0,
            "indeks sonrası harita/sayı boş"
        );
    }

    #[test]
    fn tool_args_to_rpc_skips_format_and_snake_cases() {
        let (name, args) = tool_args_to_rpc(&[
            "index_repository",
            "--repo-path",
            "/tmp/x",
            "--mode",
            "fast",
            "--format",
            "json",
        ])
        .unwrap();
        assert_eq!(name, "index_repository");
        assert_eq!(args["repo_path"], "/tmp/x");
        assert_eq!(args["mode"], "fast");
        assert!(args.get("format").is_none());
    }

    #[test]
    fn extract_rpc_tool_text_decodes_content_json_string() {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "content": [{
                    "type": "text",
                    "text": "{\"projects\":[{\"name\":\"demo\",\"root_path\":\"/tmp/demo\"}]}"
                }]
            }
        });
        let text = extract_rpc_tool_text(&payload).unwrap();
        let inner: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(inner["projects"][0]["name"], "demo");
    }

    #[test]
    fn extract_rpc_tool_text_surfaces_error_objects() {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32601, "message": "Method not found"}
        });
        let err = extract_rpc_tool_text(&payload).unwrap_err().to_string();
        assert!(err.contains("Method not found"), "{err}");
        assert!(err.contains("-32601"), "{err}");
    }

    #[test]
    fn tool_timeout_selects_mutating_vs_read() {
        let cfg = MemoryBridgeConfig::default();
        assert_eq!(tool_timeout_for(&cfg, "index_repository"), MUTATING_TIMEOUT);
        assert_eq!(tool_timeout_for(&cfg, "list_projects"), READ_TIMEOUT);
        assert_eq!(tool_timeout_for(&cfg, "query_graph"), READ_TIMEOUT);
        assert!(is_mutating_tool("index_repository"));
        assert!(!is_mutating_tool("list_projects"));
    }

    #[test]
    fn mutating_fallback_only_on_connection_refused() {
        let refused = anyhow::anyhow!("HTTP /rpc connection refused (http://127.0.0.1:1/rpc)");
        let timed = anyhow::anyhow!("HTTP /rpc zaman aşımı (http://127.0.0.1:9/rpc): timed out");
        let partial = anyhow::anyhow!("HTTP /rpc status 500");
        assert!(should_fallback_to_cli(true, &refused));
        assert!(!should_fallback_to_cli(true, &timed));
        assert!(!should_fallback_to_cli(true, &partial));
        assert!(should_fallback_to_cli(false, &timed));
    }

    #[test]
    fn from_binary_defaults_to_force_cli() {
        let bridge = MemoryBridge::from_binary("/tmp/missing-cbm");
        assert_eq!(bridge.config().transport, TransportMode::ForceCli);
    }

    async fn spawn_fake_cbm_http(projects_text: String) -> u16 {
        use axum::routing::{get, post};
        use axum::{Json, Router};
        use std::sync::Arc;

        let text = Arc::new(projects_text);
        let text_rpc = text.clone();
        let app = Router::new()
            .route(
                "/api/ui-config",
                get(|| async { Json(serde_json::json!({"lang": "en"})) }),
            )
            .route(
                "/rpc",
                post(move |_body: String| {
                    let text_rpc = text_rpc.clone();
                    async move {
                        Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": 1,
                            "result": {
                                "content": [{ "type": "text", "text": *text_rpc }]
                            }
                        }))
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        port
    }

    async fn spawn_hanging_rpc_http() -> u16 {
        use axum::routing::{get, post};
        use axum::{Json, Router};

        let app = Router::new()
            .route(
                "/api/ui-config",
                get(|| async { Json(serde_json::json!({"lang": "en"})) }),
            )
            .route(
                "/rpc",
                post(|_body: String| async {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "result": { "content": [{ "type": "text", "text": "{}" }] }
                    }))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        port
    }

    #[tokio::test]
    async fn transport_selects_http_when_ui_config_up() {
        let port = spawn_fake_cbm_http(r#"{"projects":[]}"#.into()).await;
        assert_eq!(select_tool_transport(port).await, ToolTransport::HttpRpc);
        assert_eq!(
            select_tool_transport(1).await,
            ToolTransport::Cli,
            "kapalı port CLI"
        );
    }

    #[tokio::test]
    async fn list_projects_prefers_http_rpc_over_cli() {
        let payload = r#"{"projects":[{"name":"http-demo","root_path":"/tmp/http-demo","nodes":1,"edges":0}]}"#;
        let port = spawn_fake_cbm_http(payload.into()).await;
        let bridge = MemoryBridge::with_config(
            "/tmp/missing-codebase-memory-mcp-for-http-test",
            MemoryBridgeConfig {
                http_port: port,
                transport: TransportMode::Auto,
                ..MemoryBridgeConfig::default()
            },
        );
        let projects = bridge.list_projects().await.expect("http list_projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "http-demo");
        assert_eq!(projects[0].root_path.as_deref(), Some("/tmp/http-demo"));
    }

    #[tokio::test]
    async fn list_projects_cache_avoids_second_fetch() {
        let payload = r#"{"projects":[{"name":"cached","root_path":"/tmp/c"}]}"#;
        let port = spawn_fake_cbm_http(payload.into()).await;
        let bridge = MemoryBridge::with_config(
            "/tmp/missing-for-cache-test",
            MemoryBridgeConfig {
                http_port: port,
                transport: TransportMode::Auto,
                ..MemoryBridgeConfig::default()
            },
        );
        let first = bridge.list_projects().await.unwrap();
        assert!(bridge.list_projects_cache_age().await.is_some());
        let second = bridge.list_projects().await.unwrap();
        assert_eq!(first, second);
        assert!(bridge.list_projects_cache_age().await.unwrap() < LIST_PROJECTS_CACHE_TTL);
    }

    #[tokio::test]
    async fn mutating_http_timeout_does_not_fallback_to_cli() {
        let port = spawn_hanging_rpc_http().await;
        let bridge = MemoryBridge::with_config(
            "/tmp/should-never-run-cli-on-index-timeout",
            MemoryBridgeConfig {
                http_port: port,
                transport: TransportMode::Auto,
                read_timeout: Duration::from_millis(200),
                mutating_timeout: Duration::from_millis(250),
            },
        );
        let err = bridge
            .run_tool(&[
                "index_repository",
                "--repo-path",
                "/tmp/x",
                "--format",
                "json",
            ])
            .await
            .expect_err("timeout");
        let msg = err.to_string();
        assert!(
            msg.contains("zaman aşımı") || msg.to_lowercase().contains("timeout"),
            "unexpected: {msg}"
        );
        assert!(
            !msg.contains("yok:"),
            "CLI fallback happened unexpectedly: {msg}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cli_hard_timeout_kills_sleeping_child() {
        let dir = std::env::temp_dir().join(format!("lounge-cbm-timeout-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("codebase-memory-mcp");
        std::fs::write(
            &script,
            r#"#!/bin/sh
sleep 30
echo '{"projects":[]}'
"#,
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let bridge = MemoryBridge::from_binary(&script);
        let started = std::time::Instant::now();
        let err = bridge
            .run_cli_with_timeout(
                &["list_projects", "--format", "json"],
                Duration::from_millis(400),
            )
            .await
            .expect_err("timeout bekleniyor");
        assert!(err.to_string().contains("zaman aşımı"), "unexpected: {err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "timeout çok uzun sürdü: {:?}",
            started.elapsed()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
