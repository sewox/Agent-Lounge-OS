use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::probe::{find_executable, first_existing, repo_root_from_crate};
use crate::models::{
    AstNode, CodeReference, DeadSymbol, IndexGraph, IndexSnapshot, ProjectList, ProjectSummary,
    ServiceHealth, ServiceId,
};

const CLI_TIMEOUT: Duration = Duration::from_secs(120);
/// Semantic map UI için üst sınır — viewport kilitli listede sayfalama var.
const SEMANTIC_NODE_LIMIT: u32 = 400;
const SEMANTIC_CALL_LIMIT: u32 = 800;

#[derive(Clone)]
pub struct MemoryBridge {
    binary: PathBuf,
}

impl MemoryBridge {
    pub fn discover() -> Result<Self> {
        Self::discover_from(repo_root_from_crate())
    }

    pub fn discover_from(repo_root: impl AsRef<Path>) -> Result<Self> {
        let binary = resolve_binary(repo_root.as_ref())?;
        Ok(Self { binary })
    }

    pub fn from_binary(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
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
            .run_cli(&[
                "index_repository",
                "--repo-path",
                repo,
                "--mode",
                "fast",
                "--format",
                "json",
            ])
            .await?;
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
        graph.node_count = graph.node_count.max(graph.nodes.len() as u64);
        graph.edge_count = graph.edge_count.max(graph.references.len() as u64);
        if graph.files.unwrap_or(0) == 0 {
            graph.files = Some(graph.unique_file_count());
        }
        Ok(())
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
            .run_cli(&[
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
        let stdout = self.run_cli(&["list_projects", "--format", "json"]).await?;
        let payload = parse_cli_json(&stdout)?;
        let list: ProjectList =
            serde_json::from_value(payload).context("project listesi çözülemedi")?;
        Ok(list.projects)
    }

    async fn try_cli_dead(&self, repo_path: &Path) -> Result<Option<Vec<DeadSymbol>>> {
        let repo = match repo_path.to_str() {
            Some(path) => path,
            None => return Ok(None),
        };
        for tool in ["get_dead_symbols", "dead_symbols"] {
            match self
                .run_cli(&[tool, "--repo-path", repo, "--format", "json"])
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

    async fn run_cli(&self, tool_args: &[&str]) -> Result<String> {
        if !self.binary.is_file() {
            bail!("codebase-memory-mcp yok: {}", self.binary.display());
        }

        let binary = self.binary.clone();
        let args: Vec<String> = tool_args.iter().map(|arg| (*arg).to_string()).collect();
        let pid = Arc::new(AtomicU32::new(0));
        let pid_for_child = pid.clone();

        // Tokio runtime'ı bloklamamak için std::process::Command spawn_blocking içinde.
        let wait = tokio::task::spawn_blocking(move || {
            let mut command = std::process::Command::new(&binary);
            command
                .arg("cli")
                .args(&args)
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

        let output = match tokio::time::timeout(CLI_TIMEOUT, wait).await {
            Ok(join) => join.context("codebase-memory-mcp join")??,
            Err(_) => {
                let child_pid = pid.load(Ordering::SeqCst);
                if child_pid != 0 {
                    kill_pid(child_pid);
                }
                bail!("codebase-memory-mcp zaman aşımı");
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

fn kill_pid(pid: u32) {
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status();
    }
}

fn resolve_binary(repo_root: &Path) -> Result<PathBuf> {
    if let Ok(from_env) = std::env::var("LOUNGE_MEMORY_BIN") {
        return Ok(PathBuf::from(from_env));
    }

    let mut candidates = vec![
        repo_root.join("bridge/codebase-memory-mcp"),
        repo_root.join("bridge/codebase-memory-mcp.exe"),
    ];
    if let Some(on_path) = find_executable("codebase-memory-mcp") {
        candidates.push(on_path);
    }

    first_existing(candidates)
        .ok_or_else(|| anyhow::anyhow!("codebase-memory-mcp bulunamadı (bridge/ veya PATH)"))
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
        if !bridge.binary_path().is_file() {
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
        if !bridge.binary_path().is_file() {
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
}
