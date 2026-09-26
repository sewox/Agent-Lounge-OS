//! Agent Lounge OS MCP sunucusu — JSON-RPC 2.0 / stdio.
//!
//! Cursor ve Claude Desktop `command` + `args` ile bu binary'yi alt süreç olarak
//! başlatır. Protokol mesajları yalnızca stdout'a yazılır; loglar stderr'e gider.
//!
//! Tauri UI gerekmez: `lounge_search_experience` / `lounge_record_*` doğrudan
//! experience store (SQLite) kullanır. `lounge_dispatch_task` NATS'a yayınlar;
//! DecisionGate / security / quota için **Lounge Kernel** (masaüstü uygulaması)
//! aynı NATS bus'ında dinliyor olmalıdır.

use std::io::{BufRead, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::db::ExperienceStore;
use crate::models::{
    host_display_name, now_rfc3339, DiscoveredTool, ExperienceOutcome, ExperienceRecord,
    LoungeTask, TASK_REQUESTED,
};
use crate::services::nats_manager::default_nats_url;
use crate::services::{lounge_ollama_endpoint, system_ollama_endpoint};

/// MCP protokol sürümleri (istemci istediğini yansıtmayı tercih ederiz).
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
pub const DEFAULT_PROTOCOL_VERSION: &str = "2025-03-26";

const SERVER_NAME: &str = "agent-lounge-os";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone)]
pub struct McpServer {
    store: ExperienceStore,
    nats_url: String,
    client_name: String,
    client_version: String,
    initialized: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct JsonRpcRequest {
    #[serde(default = "default_jsonrpc")]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

fn default_jsonrpc() -> String {
    "2.0".into()
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl McpServer {
    pub fn new(store: ExperienceStore, nats_url: impl Into<String>) -> Self {
        Self {
            store,
            nats_url: nats_url.into(),
            client_name: "mcp-client".into(),
            client_version: "0".into(),
            initialized: false,
        }
    }

    pub fn with_client(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.client_name = name.into();
        self.client_version = version.into();
        self
    }

    /// Tek satırlık JSON-RPC isteğini işle; yanıt satırı (veya None = notification).
    pub async fn handle_line(&mut self, line: &str) -> Result<Option<String>> {
        let line = line.trim();
        if line.is_empty() {
            return Ok(None);
        }
        let req: JsonRpcRequest =
            serde_json::from_str(line).with_context(|| format!("geçersiz JSON-RPC: {line}"))?;
        if req.jsonrpc != "2.0" {
            return Ok(Some(error_response(
                req.id.unwrap_or(Value::Null),
                -32600,
                "jsonrpc 2.0 gerekli",
                None,
            )));
        }

        // Bildirimler (id yok): yanıt yok.
        if req.id.is_none() {
            self.handle_notification(&req.method, &req.params).await?;
            return Ok(None);
        }
        let id = req.id.clone().unwrap_or(Value::Null);

        match self.dispatch(req.method.as_str(), &req.params).await {
            Ok(result) => Ok(Some(ok_response(id, result))),
            Err(err) => Ok(Some(error_response(id, -32000, &err.to_string(), None))),
        }
    }

    async fn handle_notification(&mut self, method: &str, _params: &Value) -> Result<()> {
        match method {
            "notifications/initialized" | "initialized" => {
                self.initialized = true;
                if let Err(err) = self.record_connecting_client().await {
                    eprintln!("[lounge-mcp] client kaydı: {err}");
                }
            }
            "notifications/cancelled" => {}
            other => eprintln!("[lounge-mcp] bilinmeyen bildirim: {other}"),
        }
        Ok(())
    }

    async fn dispatch(&mut self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "initialize" => self.initialize(params).await,
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tool_defs() })),
            "tools/call" => self.tools_call(params).await,
            "resources/list" => Ok(json!({ "resources": [] })),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            other => Err(anyhow!("method bulunamadı: {other}")),
        }
    }

    async fn initialize(&mut self, params: &Value) -> Result<Value> {
        let client = params.get("clientInfo").cloned().unwrap_or(json!({}));
        let name = client
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("mcp-client");
        let version = client
            .get("version")
            .and_then(|v| v.as_str())
            .unwrap_or("0");
        self.client_name = name.to_string();
        self.client_version = version.to_string();

        let requested = params
            .get("protocolVersion")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_PROTOCOL_VERSION);
        let protocol_version = if PROTOCOL_VERSIONS.contains(&requested) {
            requested
        } else {
            DEFAULT_PROTOCOL_VERSION
        };

        // Erken kayıt: bazı istemciler initialized bildirimini atlayabilir.
        if let Err(err) = self.record_connecting_client().await {
            eprintln!("[lounge-mcp] client kaydı (initialize): {err}");
        }

        Ok(json!({
            "protocolVersion": protocol_version,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION,
                "title": "Agent Lounge OS"
            },
            "instructions": "Agent Lounge OS yerel orkestrasyon katmanı. Tecrübe ara/kaydet; görevleri NATS üzerinden Kernel'e ilet (güvenlik/kota kapıları uygulanır)."
        }))
    }

    async fn record_connecting_client(&self) -> Result<()> {
        let host = normalize_client_host(&self.client_name);
        let display = host_display_name(&host);
        let mut tool = DiscoveredTool::host_app(&host, &display);
        tool.detail = Some(format!(
            "MCP stdio · {}@{}",
            self.client_name, self.client_version
        ));
        tool.origin_path = Some(format!("mcp://{}", self.client_name));
        tool.endpoint = Some("stdio".into());
        tool.available = true;
        self.store.upsert_connected_tool(tool).await?;
        Ok(())
    }

    async fn tools_call(&self, params: &Value) -> Result<Value> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("tools/call: name gerekli"))?;
        let args = params.get("arguments").cloned().unwrap_or(json!({}));

        let (payload, is_error) = match name {
            "lounge_search_experience" => (self.tool_search(&args).await?, false),
            "lounge_record_experience" | "lounge_record_decision" => {
                (self.tool_record(&args).await?, false)
            }
            "lounge_ask_agent" | "lounge_dispatch_task" => {
                (self.tool_dispatch(&args).await?, false)
            }
            "lounge_status" => (self.tool_status().await?, false),
            other => {
                return Ok(tool_result(
                    json!({ "error": format!("bilinmeyen tool: {other}") }),
                    true,
                ));
            }
        };
        Ok(tool_result(payload, is_error))
    }

    async fn tool_search(&self, args: &Value) -> Result<Value> {
        let query = arg_str(args, "query")
            .ok_or_else(|| anyhow!("query gerekli"))?
            .to_string();
        let project = arg_str(args, "project")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        let mut hits = self
            .store
            .search_experiences(query.clone(), limit.or(Some(12)))
            .await?;
        if let Some(ref project_id) = project {
            hits.retain(|row| row.project_id == *project_id);
        }

        Ok(json!({
            "query": query,
            "project": project,
            "count": hits.len(),
            "experiences": hits,
        }))
    }

    async fn tool_record(&self, args: &Value) -> Result<Value> {
        let project = arg_str(args, "project")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("project gerekli"))?;
        let context = arg_str(args, "context")
            .or_else(|| arg_str(args, "topic"))
            .unwrap_or("")
            .trim()
            .to_string();
        let decision = arg_str(args, "decision")
            .or_else(|| arg_str(args, "outcome_text"))
            .or_else(|| arg_str(args, "summary"))
            .unwrap_or("")
            .trim()
            .to_string();
        if context.is_empty() && decision.is_empty() {
            return Err(anyhow!("context veya decision gerekli"));
        }
        let outcome = parse_outcome(arg_str(args, "outcome").unwrap_or("success"));
        let agent = arg_str(args, "agent")
            .map(str::to_string)
            .unwrap_or_else(|| self.client_name.clone());

        let topic = if context.is_empty() {
            decision.clone()
        } else {
            context.clone()
        };
        let solution = if decision.is_empty() {
            context.clone()
        } else {
            decision.clone()
        };

        let record = ExperienceRecord {
            id: Uuid::new_v4().to_string(),
            project_id: project.clone(),
            agent_id: agent.clone(),
            topic: topic.clone(),
            solution_summary: solution.clone(),
            adr_record: solution.clone(),
            outcome,
            related_task_id: None,
            tags: vec!["mcp".into(), "external".into()],
            created_at: now_rfc3339(),
            embedding: Vec::new(),
        };
        let id = record.id.clone();
        self.store.insert_record(record).await?;

        Ok(json!({
            "id": id,
            "project_id": project,
            "agent": agent,
            "topic": topic,
            "recorded": true,
        }))
    }

    async fn tool_dispatch(&self, args: &Value) -> Result<Value> {
        let target = arg_str(args, "target_agent")
            .or_else(|| arg_str(args, "agent"))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("target_agent gerekli"))?;
        let task_text = arg_str(args, "task")
            .or_else(|| arg_str(args, "summary"))
            .unwrap_or("")
            .trim()
            .to_string();
        if task_text.is_empty() {
            return Err(anyhow!("task gerekli"));
        }
        let project = arg_str(args, "project")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "agent-lounge-os".into());

        let source = format!("mcp:{}", normalize_client_host(&self.client_name));
        let mut task = LoungeTask::new(source, project.clone(), task_text.clone());
        task.target_agent = Some(target.clone());

        let nats_ok = probe_tcp_host_port(&self.nats_url);
        if !nats_ok {
            return Ok(json!({
                "published": false,
                "error": format!(
                    "NATS erişilemiyor ({}) — Lounge Kernel / NATS ayakta olmalı",
                    self.nats_url
                ),
                "task_id": task.id,
                "subject": TASK_REQUESTED,
                "note": "Görev Kernel dispatcher üzerinden işlenir; PENDING_APPROVAL / kota kapıları atlanmaz."
            }));
        }

        let url = self.nats_url.clone();
        let subject = TASK_REQUESTED.to_string();
        let bytes = serde_json::to_vec(&task)?;
        tokio::task::spawn_blocking(move || {
            #[allow(deprecated)]
            let nc = nats::connect(&url).map_err(|e| anyhow!("NATS connect: {e}"))?;
            nc.publish(&subject, bytes)
                .map_err(|e| anyhow!("NATS publish: {e}"))?;
            nc.flush().map_err(|e| anyhow!("NATS flush: {e}"))?;
            Ok::<(), anyhow::Error>(())
        })
        .await
        .context("NATS publish join")??;

        Ok(json!({
            "published": true,
            "task_id": task.id,
            "subject": TASK_REQUESTED,
            "target_agent": target,
            "project_id": project,
            "summary": task_text,
            "note": "Görev NATS'a yazıldı. Kernel DecisionGate / security (PENDING_APPROVAL) / quota uygular; UI yoksa onay bekleyen işler kalabilir."
        }))
    }

    async fn tool_status(&self) -> Result<Value> {
        let connected = self
            .store
            .list_connected_tools()
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|t| t.is_active && t.enabled)
            .map(|t| {
                json!({
                    "id": t.id,
                    "name": t.name,
                    "type": t.tool_type,
                    "source": t.source,
                    "last_synced": t.last_synced,
                    "detail": t.payload.get("detail"),
                })
            })
            .collect::<Vec<_>>();

        let nats_up = probe_tcp_host_port(&self.nats_url);
        let lmr = lounge_ollama_endpoint();
        let lmr_up = probe_lmr_reachable(&lmr).await;
        let system_ollama = system_ollama_endpoint();

        Ok(json!({
            "server": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION,
                "transport": "stdio",
                "client": {
                    "name": self.client_name,
                    "version": self.client_version,
                    "initialized": self.initialized,
                }
            },
            "nats": {
                "url": self.nats_url,
                "reachable": nats_up,
            },
            "lmr": {
                "endpoint": lmr,
                "reachable": lmr_up,
                "note": "Host Ollama (:11434) bilinçli olarak dokunulmaz",
                "system_ollama": system_ollama,
            },
            "connected_agents": connected,
            "dispatch_requires_kernel": true,
            "hint": "lounge_dispatch_task için NATS + çalışan Agent Lounge OS (Kernel) gerekir. Arama/kayıt yalnızca SQLite ile çalışır."
        }))
    }
}

fn tool_defs() -> Vec<Value> {
    vec![
        tool_def(
            "lounge_search_experience",
            "Geçmiş tecrübe / ADR kayıtlarını experience store üzerinde ara (projeler arası).",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Arama metni" },
                    "project": { "type": "string", "description": "İsteğe bağlı proje filtresi" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50 }
                },
                "required": ["query"]
            }),
        ),
        tool_def(
            "lounge_record_experience",
            "Yeni bir tecrübe / karar kaydı ekle; diğer ajanlar öğrenebilir.",
            record_schema(),
        ),
        tool_def(
            "lounge_record_decision",
            "lounge_record_experience ile aynı — mimari karar (ADR) kaydı için alias.",
            record_schema(),
        ),
        tool_def(
            "lounge_dispatch_task",
            "Görevi NATS lounge.task.requested üzerinden Kernel dispatcher'a ilet. Security/quota kapıları uygulanır; PENDING_APPROVAL atlanmaz.",
            dispatch_schema(),
        ),
        tool_def(
            "lounge_ask_agent",
            "lounge_dispatch_task alias — hedef ajana iş gönder.",
            dispatch_schema(),
        ),
        tool_def(
            "lounge_status",
            "Bağlı ajanlar / MCP istemcileri ve NATS·LMR sağlık özeti.",
            json!({
                "type": "object",
                "properties": {}
            }),
        ),
    ]
}

fn record_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": { "type": "string" },
            "context": { "type": "string", "description": "Konu / bağlam" },
            "decision": { "type": "string", "description": "Karar / çözüm özeti" },
            "outcome": {
                "type": "string",
                "enum": ["success", "failure", "partial"],
                "description": "Varsayılan: success"
            },
            "agent": { "type": "string", "description": "Kayıt ajanı (varsayılan: MCP istemci adı)" }
        },
        "required": ["project"]
    })
}

fn dispatch_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "target_agent": { "type": "string", "description": "Örn. grok_bot, claude_desktop" },
            "task": { "type": "string" },
            "project": { "type": "string" }
        },
        "required": ["target_agent", "task"]
    })
}

fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

fn tool_result(payload: Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string());
    let mut map = Map::new();
    map.insert("content".into(), json!([{ "type": "text", "text": text }]));
    map.insert("structuredContent".into(), payload);
    if is_error {
        map.insert("isError".into(), Value::Bool(true));
    }
    Value::Object(map)
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str()).map(str::trim)
}

fn parse_outcome(raw: &str) -> ExperienceOutcome {
    match raw.trim().to_ascii_lowercase().as_str() {
        "failure" | "fail" | "failed" => ExperienceOutcome::Failure,
        "partial" => ExperienceOutcome::Partial,
        _ => ExperienceOutcome::Success,
    }
}

/// clientInfo.name → dashboard host id (`cursor`, `claude_desktop`, …).
pub fn normalize_client_host(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    if lower.contains("claude") {
        return "claude_desktop".into();
    }
    if lower.contains("cursor") {
        return "cursor".into();
    }
    if lower.contains("grok") {
        return "grok_bot".into();
    }
    if lower.contains("antigravity") {
        return "antigravity".into();
    }
    let slug: String = lower
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if slug.is_empty() {
        "mcp_client".into()
    } else {
        slug
    }
}

fn probe_tcp_host_port(nats_url: &str) -> bool {
    let addr = nats_url
        .trim()
        .trim_start_matches("nats://")
        .trim_start_matches("tcp://")
        .split('/')
        .next()
        .unwrap_or("127.0.0.1:4222");
    let Ok(mut addrs) = addr.to_socket_addrs() else {
        return false;
    };
    let Some(sock) = addrs.next() else {
        return false;
    };
    TcpStream::connect_timeout(&sock, Duration::from_millis(400)).is_ok()
}

async fn probe_lmr_reachable(endpoint: &str) -> bool {
    let url = format!("{}/api/tags", endpoint.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(600))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    match client.get(url).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

fn ok_response(id: Value, result: Value) -> String {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: Some(result),
        error: None,
    };
    serde_json::to_string(&resp).expect("json serialize")
}

fn error_response(id: Value, code: i32, message: &str, data: Option<Value>) -> String {
    let resp = JsonRpcResponse {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.into(),
            data,
        }),
    };
    serde_json::to_string(&resp).expect("json serialize")
}

/// stdin/stdout üzerinden MCP döngüsü.
pub async fn run_stdio(store: ExperienceStore, nats_url: impl Into<String>) -> Result<()> {
    let mut server = McpServer::new(store, nats_url);
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let locked = stdin.lock();
    for line in locked.lines() {
        let line = line.context("stdin okunamadı")?;
        if let Some(response) = server.handle_line(&line).await? {
            writeln!(stdout, "{response}").context("stdout yazılamadı")?;
            stdout.flush().ok();
        }
    }
    Ok(())
}

/// Ortam / CLI argümanlarından experience DB yolu.
pub fn resolve_db_path(workspace: PathBuf) -> PathBuf {
    if let Ok(path) = std::env::var("LOUNGE_DB_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    if let Ok(path) = std::env::var("LOUNGE_EXPERIENCE_DB") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    crate::db::default_db_path(workspace)
}

pub fn resolve_nats_url() -> String {
    std::env::var("LOUNGE_NATS_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(default_nats_url)
}

pub fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[test]
    fn normalizes_known_hosts() {
        assert_eq!(normalize_client_host("Cursor"), "cursor");
        assert_eq!(normalize_client_host("claude-desktop"), "claude_desktop");
        assert_eq!(normalize_client_host("Claude Desktop"), "claude_desktop");
    }

    /// Gerçek transport: newline-delimited JSON-RPC over async duplex streams.
    #[tokio::test]
    async fn stdio_transport_roundtrip() {
        let store = ExperienceStore::memory().expect("db");
        store
            .insert_record(ExperienceRecord {
                id: Uuid::new_v4().to_string(),
                project_id: "p1".into(),
                agent_id: "seed".into(),
                topic: "NATS subject naming".into(),
                solution_summary: "lounge.task.requested kullan".into(),
                adr_record: "lounge.task.requested kullan".into(),
                outcome: ExperienceOutcome::Success,
                related_task_id: None,
                tags: vec![],
                created_at: now_rfc3339(),
                embedding: Vec::new(),
            })
            .await
            .expect("seed");

        let (mut client_write, server_read) = tokio::io::duplex(16_384);
        let (server_write, client_read) = tokio::io::duplex(16_384);

        let store_for_server = store.clone();
        let serve = tokio::spawn(async move {
            let mut server = McpServer::new(store_for_server, "nats://127.0.0.1:9");
            let mut lines = BufReader::new(server_read).lines();
            let mut out = server_write;
            while let Some(line) = lines.next_line().await.expect("read") {
                if let Some(resp) = server.handle_line(&line).await.expect("handle") {
                    out.write_all(resp.as_bytes()).await.unwrap();
                    out.write_all(b"\n").await.unwrap();
                    out.flush().await.unwrap();
                }
            }
        });

        let mut reader = BufReader::new(client_read);
        let mut buf = String::new();

        async fn rpc(
            write: &mut tokio::io::DuplexStream,
            reader: &mut BufReader<tokio::io::DuplexStream>,
            buf: &mut String,
            line: &str,
        ) -> Value {
            write.write_all(line.as_bytes()).await.unwrap();
            write.write_all(b"\n").await.unwrap();
            write.flush().await.unwrap();
            buf.clear();
            reader.read_line(buf).await.unwrap();
            serde_json::from_str(buf.trim()).unwrap()
        }

        let init = rpc(
            &mut client_write,
            &mut reader,
            &mut buf,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"claude-ai","version":"0.9"}}}"#,
        )
        .await;
        assert_eq!(init["result"]["protocolVersion"], "2024-11-05");

        client_write
            .write_all(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await
            .unwrap();
        client_write.write_all(b"\n").await.unwrap();
        client_write.flush().await.unwrap();

        let tools = rpc(
            &mut client_write,
            &mut reader,
            &mut buf,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        )
        .await;
        assert!(!tools["result"]["tools"].as_array().unwrap().is_empty());

        let call = rpc(
            &mut client_write,
            &mut reader,
            &mut buf,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"lounge_search_experience","arguments":{"query":"NATS subject"}}}"#,
        )
        .await;
        let body = call["result"]["structuredContent"].clone();
        assert!(body["count"].as_u64().unwrap() >= 1);

        drop(client_write);
        let _ = serve.await;

        let connected = store.list_connected_tools().await.unwrap();
        assert!(connected.iter().any(|c| c.id == "app:claude_desktop"));
    }
}
