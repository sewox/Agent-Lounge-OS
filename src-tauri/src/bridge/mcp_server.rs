//! Agent Lounge OS MCP sunucusu — JSON-RPC 2.0.
//!
//! **Taşıma:** Kernel (Tauri) `http://127.0.0.1:18791` üzerinde HTTP sunar.
//! `lounge-mcp` binary Claude Desktop / Cursor için **stdio shim** olup Kernel
//! HTTP'ye proxy eder; Kernel yoksa gömülü standalone moda düşer.
//!
//! **Güvenlik sınırı:** dosya sistemi yazma tool'u yok; kota / politika / routing
//! ayarları değiştirilemez (yalnızca UI). Tool girdileri `lounge_protocol`
//! şemalarına göre doğrulanır (`additionalProperties: false`).

use std::collections::HashMap;
use std::io::BufRead;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use lounge_protocol::{validate_schema, SchemaKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio::sync::{watch, Mutex};
use uuid::Uuid;

use super::orchestration::{
    CallAgentArgs, CancelKind, NoopProgressSink, Orchestrator, ProgressSink, WaitOpts,
};
use super::timeout_manager::{
    parse_timeout_setting, TimeoutManager, ENV_MCP_TIMEOUT_SECS, SETTING_MCP_TIMEOUT_SECS,
};
use super::wait_clock::{SystemWaitClock, WaitClock};
use crate::db::{bind_write_project_id, prefer_canonical, ExperienceStore, ProjectBind};
use crate::kernel::worker_registry::{workers_status_json, WorkerRegistry};
use crate::models::{
    host_display_name, now_rfc3339, DiscoveredTool, ExperienceOutcome, ExperienceRecord,
    LoungeExperience, EXPERIENCE_REPORTED, TASK_REQUESTED,
};
use crate::services::nats_manager::default_nats_url;
use crate::services::{lounge_ollama_endpoint, system_ollama_endpoint};

/// MCP HTTP varsayılan bind (Kernel).
pub const DEFAULT_MCP_HTTP_BIND: &str = "127.0.0.1:18791";

/// MCP protokol sürümleri (istemci istediğini yansıtmayı tercih ederiz).
pub const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
pub const DEFAULT_PROTOCOL_VERSION: &str = "2025-03-26";

const SERVER_NAME: &str = "agent-lounge-os";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct ClientCtx {
    pub name: String,
    pub version: String,
    pub initialized: bool,
    /// MCP oturum kimliği (`Mcp-Session-Id` / stdio süreç kimliği).
    pub session_id: String,
    /// İsteğe bağlı workspace (initialize / list_my_tasks sahiplik kapsamı).
    pub workspace_path: String,
}

impl Default for ClientCtx {
    fn default() -> Self {
        Self {
            name: "mcp-client".into(),
            version: "0".into(),
            initialized: false,
            session_id: Uuid::new_v4().to_string(),
            workspace_path: String::new(),
        }
    }
}

impl ClientCtx {
    pub fn label(&self) -> Value {
        json!({
            "name": self.name,
            "version": self.version,
            "initialized": self.initialized,
            "session_id": self.session_id,
            "workspace_path": self.workspace_path,
        })
    }
}

/// MCP Resource — A2A işletim kılavuzu (timeout / polling / piggyback).
pub const A2A_GUIDE_URI: &str = "lounge://help/a2a-guide";
const A2A_GUIDE_MIME: &str = "text/markdown";

fn a2a_guide_markdown() -> &'static str {
    r#"# Agent Lounge OS — A2A Guide

Bu kaynak, ajanların Lounge üzerinden başka ajanlara iş vermesini ve sonuç almasını açıklar.
Masaüstü istemciler: **Grok Bot**, **Cursor**, **Antigravity**, **Claude Desktop**.

## Temel akış

1. `lounge_call_agent` ile görev verin (veya `lounge_dispatch_task` fire-and-forget).
2. Eşik içinde sonuç gelmezse yanıt `status: backgrounded` + `task_id` (+ `task_token`) döner.
3. `poll_after_secs` ve `next_action` değerlerine uyun; hemen sonsuz `lounge_wait_task` döngüsüne girmeyin.
4. Sonuç hazır olduğunda:
   - `lounge_wait_task(task_id)` ile tam sonucu alın, **veya**
   - Herhangi bir tool yanıtındaki `_meta.pending_results` piggyback özetini görün (Claude Desktop için kritik — progress token yok).
5. Süreci `lounge_list_my_tasks` ile izleyin (aynı `Mcp-Session-Id` / `source_session_id`).

## İstemci bekleme eşikleri (ClientProfile)

| İstemci | Mod A eşiği | Not |
|---|---:|---|
| Antigravity | ~150s | `notifications/cancelled` (deadline) → görev **backgrounded kalır** |
| Cursor | ~100s (progress ile ≤280s) | progressToken heartbeat |
| Claude Desktop (`claude-ai`) | ~210s | Progress yok → **piggyback zorunlu yol** |
| Grok Bot / bilinmeyen | ~45s | Bulut / unknown profil |

`long_running=true`: eşiği beklemeden hemen `backgrounded` + `task_id`.
`must_deliver=true`: kopmada iptal yok; orphan EXPIRED yok; kota aşımında `-32029`.

## Polling kuralları

- `poll_after_secs`: 15 → ×1.5 → en fazla 60.
- `still_running` / `backgrounded` iken `next_action` = `lounge_wait_task`.
- Sonuç `completed`/`failed` olmadan kullanıcıya "bitti" demeyin.
- Piggyback yalnız özet verir; tam gövde için `lounge_wait_task` çağırın (teslim `last_wait_at`).

## Sahiplik

- Görevler `source_session_id` (= `Mcp-Session-Id`) ile bağlanır.
- Workspace paylaşımı yok; `lounge_list_my_tasks` yalnız bu oturumun görevlerini listeler.
- `task_token` yeniden bağlanmada wait yetkisi verir (düz token bir kez; DB hash).

## Kaynaklar

- Bu kılavuz: `lounge://help/a2a-guide` (`resources/read`)
- Durum: `lounge_status`
"#
}

/// Uçuştaki tools/call — `notifications/cancelled` ile eşleşir.
type InFlightMap = Arc<Mutex<HashMap<String, watch::Sender<Option<CancelKind>>>>>;
type InFlightOrder = Arc<Mutex<std::collections::VecDeque<String>>>;

#[derive(Clone)]
pub struct McpServer {
    store: ExperienceStore,
    nats_url: String,
    /// Embedded stdio / tek istemci yolu.
    client: ClientCtx,
    workers: Option<WorkerRegistry>,
    orchestrator: Orchestrator,
    timeouts: Arc<TimeoutManager>,
    in_flight: InFlightMap,
    in_flight_order: InFlightOrder,
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
        let nats_url = nats_url.into();
        let timeouts = Arc::new(TimeoutManager::with_defaults());
        // Settings (varsa) sonra env — env en yüksek öncelik.
        if let Ok(conn) = store.conn.lock() {
            if let Ok(raw) = conn.query_row(
                "SELECT value_json FROM settings WHERE key = ?1",
                rusqlite::params![SETTING_MCP_TIMEOUT_SECS],
                |row| row.get::<_, String>(0),
            ) {
                if let Some(secs) = parse_timeout_setting(&raw) {
                    timeouts.set_global_override_secs(Some(secs));
                }
            }
        }
        if let Ok(raw) = std::env::var(ENV_MCP_TIMEOUT_SECS) {
            if let Some(secs) = parse_timeout_setting(&raw) {
                timeouts.set_global_override_secs(Some(secs));
            }
        }
        let clock: Arc<dyn WaitClock> = Arc::new(SystemWaitClock::default());
        let orchestrator =
            Orchestrator::new(store.clone(), nats_url.clone(), timeouts.clone(), clock);
        timeouts.attach_settings_store(store.clone());
        Self {
            store,
            nats_url,
            client: ClientCtx::default(),
            workers: None,
            orchestrator,
            timeouts,
            in_flight: Arc::new(Mutex::new(HashMap::new())),
            in_flight_order: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        }
    }

    pub fn with_workers(mut self, workers: WorkerRegistry) -> Self {
        self.workers = Some(workers);
        self
    }

    pub fn with_client(mut self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.client.name = name.into();
        self.client.version = version.into();
        self
    }

    pub fn with_orchestrator_clock(mut self, clock: Arc<dyn WaitClock>) -> Self {
        let skip = self.orchestrator.skips_nats();
        self.orchestrator = Orchestrator::new(
            self.store.clone(),
            self.nats_url.clone(),
            self.timeouts.clone(),
            clock,
        )
        .with_skip_nats(skip);
        self
    }

    pub fn with_skip_nats(mut self, skip: bool) -> Self {
        self.orchestrator = self.orchestrator.with_skip_nats(skip);
        self
    }

    pub fn timeouts(&self) -> &TimeoutManager {
        &self.timeouts
    }

    pub fn in_flight(&self) -> InFlightMap {
        self.in_flight.clone()
    }

    pub fn client_label(&self) -> Value {
        self.client.label()
    }

    /// Oturum kopması: in-flight cancel kanallarını SessionDisconnect ile bilgilendir.
    /// must_deliver → arka plan; aksi → iptal. Backgrounded map'te yok → korunur.
    pub async fn on_session_disconnect(&self, session_id: &str) -> usize {
        let keys: Vec<String> = {
            let map = self.in_flight.lock().await;
            let prefix = format!("{session_id}:");
            map.keys()
                .filter(|k| k.starts_with(&prefix) || k.as_str() == session_id)
                .cloned()
                .collect()
        };
        let mut txs = Vec::new();
        {
            let mut map = self.in_flight.lock().await;
            let mut order = self.in_flight_order.lock().await;
            for key in &keys {
                if let Some(tx) = map.remove(key) {
                    txs.push(tx);
                }
                if let Some(pos) = order.iter().position(|k| k == key) {
                    order.remove(pos);
                }
            }
        }
        self.orchestrator
            .cancel_in_flight_on_disconnect(session_id, txs)
            .await
    }

    /// Stdio EOF — gömülü tek oturumun tüm in-flight'ları.
    pub async fn on_transport_eof(&self) -> usize {
        let session = self.client.session_id.clone();
        if session.is_empty() {
            // Oturum id yoksa tüm in-flight'ları iptal et.
            let txs: Vec<_> = {
                let mut map = self.in_flight.lock().await;
                let mut order = self.in_flight_order.lock().await;
                order.clear();
                map.drain().map(|(_, tx)| tx).collect()
            };
            return self
                .orchestrator
                .cancel_in_flight_on_disconnect("_stdio_eof", txs)
                .await;
        }
        self.on_session_disconnect(&session).await
    }

    /// Tek satırlık JSON-RPC (embedded stdio — tek istemci durumu).
    pub async fn handle_line(&mut self, line: &str) -> Result<Option<String>> {
        let mut client = self.client.clone();
        let out = self.handle_line_for(line, &mut client).await?;
        self.client = client;
        Ok(out)
    }

    /// İstek başına istemci kimliği (HTTP oturum / shim).
    /// `&self` — uzun çağrılar sunucu kilidi olmadan paralel çalışabilir (Clone + paylaşılan Arc alanları).
    pub async fn handle_line_for(
        &self,
        line: &str,
        client: &mut ClientCtx,
    ) -> Result<Option<String>> {
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
            self.handle_notification(&req.method, &req.params, client)
                .await?;
            return Ok(None);
        }
        let id = req.id.clone().unwrap_or(Value::Null);
        let request_key = request_id_key(&client.session_id, &id);

        match self
            .dispatch(req.method.as_str(), &req.params, client, &request_key)
            .await
        {
            Ok(result) => Ok(Some(ok_response(id, result))),
            Err(err) => {
                let (code, message) = mcp_rpc_code_message(&err);
                Ok(Some(error_response(id, code, &message, None)))
            }
        }
    }

    async fn handle_notification(
        &self,
        method: &str,
        params: &Value,
        client: &mut ClientCtx,
    ) -> Result<()> {
        match method {
            "notifications/initialized" | "initialized" => {
                client.initialized = true;
                if let Err(err) = self.record_connecting_client(client).await {
                    eprintln!("[lounge-mcp] client kaydı: {err}");
                }
            }
            "notifications/cancelled" => {
                let reason = params.get("reason").and_then(|v| v.as_str()).unwrap_or("");
                let kind = CancelKind::from_reason(reason);
                let req_id = params.get("requestId").cloned().unwrap_or(Value::Null);
                let key = request_id_key(&client.session_id, &req_id);
                let mut map = self.in_flight.lock().await;
                if let Some(tx) = map.remove(&key) {
                    let _ = tx.send(Some(kind));
                    eprintln!(
                        "[lounge-mcp] cancelled session={} reason={reason:?} kind={kind:?}",
                        client.session_id
                    );
                } else {
                    eprintln!(
                        "[lounge-mcp] cancelled (no in-flight) session={} reason={reason:?}",
                        client.session_id
                    );
                }
            }
            other => eprintln!("[lounge-mcp] bilinmeyen bildirim: {other}"),
        }
        Ok(())
    }

    async fn dispatch(
        &self,
        method: &str,
        params: &Value,
        client: &mut ClientCtx,
        request_key: &str,
    ) -> Result<Value> {
        match method {
            "initialize" => self.initialize(params, client).await,
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({
                "tools": tool_defs(client, &self.timeouts)
            })),
            "tools/call" => self.tools_call(params, client, request_key).await,
            "resources/list" => Ok(json!({ "resources": resource_defs() })),
            "resources/read" => self.resources_read(params),
            "prompts/list" => Ok(json!({ "prompts": [] })),
            other => Err(anyhow!("method bulunamadı: {other}")),
        }
    }

    async fn initialize(&self, params: &Value, client: &mut ClientCtx) -> Result<Value> {
        let info = params.get("clientInfo").cloned().unwrap_or(json!({}));
        let name = info
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("mcp-client");
        let version = info.get("version").and_then(|v| v.as_str()).unwrap_or("0");
        client.name = name.to_string();
        client.version = version.to_string();
        if let Some(ws) = params
            .pointer("/clientInfo/workspace_path")
            .or_else(|| params.pointer("/clientInfo/workspaceRoot"))
            .or_else(|| params.get("workspace_path"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            client.workspace_path = ws.to_string();
        }

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
        if let Err(err) = self.record_connecting_client(client).await {
            eprintln!("[lounge-mcp] client kaydı (initialize): {err}");
        }

        let profile = self.timeouts.profile_for(&client.name);
        Ok(json!({
            "protocolVersion": protocol_version,
            "capabilities": {
                "tools": {},
                "resources": {}
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION,
                "title": "Agent Lounge OS"
            },
            "instructions": format!(
                "Agent Lounge OS yerel orkestrasyon. Önce resources/read ile {A2A_GUIDE_URI} okuyun \
        (timeout/polling/piggyback). Bu istemci Mod A eşiği ~{}s (profil {}). \
        lounge_call_agent → backgrounded olursa poll_after_secs'e uyun; sonraki tool yanıtlarında \
        _meta.pending_results piggyback gelebilir; lounge_list_my_tasks ile süreci izleyin.",
                profile.threshold_secs,
                profile.name
            )
        }))
    }

    async fn record_connecting_client(&self, client: &ClientCtx) -> Result<()> {
        let host = normalize_client_host(&client.name);
        let display = host_display_name(&host);
        let mut tool = DiscoveredTool::host_app(&host, &display);
        tool.detail = Some(format!("MCP · {}@{}", client.name, client.version));
        tool.origin_path = Some(format!("mcp://{}", client.name));
        tool.endpoint = Some(default_mcp_http_url());
        tool.available = true;
        self.store.upsert_connected_tool(tool).await?;

        // P1-B: MCP oturumunu agent_sessions'a bağla — yield target_agent eşleşmesi.
        // agent_id = normalize(clientInfo.name); session id = Mcp-Session-Id.
        // Not: gerçek yetki token'a bağlı değil; session id istemci seçimli (kabul edilen risk).
        // İnbound MCP istemci oturumu user-owned (Orchestrated değil); Kernel claim → lounge.
        let mut sess = crate::models::AgentSession::new(
            "mcp",
            &host,
            "mcp",
            &client.workspace_path,
            "mcp_meta",
        );
        sess.id = client.session_id.clone();
        sess.owner = "user".into();
        sess.state = "active".into();
        sess.last_seen = crate::models::now_rfc3339();
        if let Err(err) = self.store.upsert_session(&sess) {
            eprintln!("[lounge-mcp] agent_session bağlama: {err}");
        }
        Ok(())
    }

    fn resources_read(&self, params: &Value) -> Result<Value> {
        let uri = params
            .get("uri")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("resources/read: uri gerekli"))?;
        if uri != A2A_GUIDE_URI {
            return Err(anyhow!("resource bulunamadı: {uri}"));
        }
        Ok(json!({
            "contents": [{
                "uri": A2A_GUIDE_URI,
                "mimeType": A2A_GUIDE_MIME,
                "text": a2a_guide_markdown(),
            }]
        }))
    }

    async fn tools_call(
        &self,
        params: &Value,
        client: &ClientCtx,
        request_key: &str,
    ) -> Result<Value> {
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("tools/call: name gerekli"))?;
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let progress_token = params
            .pointer("/_meta/progressToken")
            .cloned()
            .or_else(|| params.get("progressToken").cloned());

        // Güvenlik sınırı: ayar / dosya yazma tool'ları yok ve reddedilir.
        if is_forbidden_tool(name) {
            return Ok(tool_result(
                json!({
                    "error": "güvenlik sınırı: bu tool yok veya yasak (dosya yazma / kota / politika / routing yalnızca UI)",
                    "tool": name
                }),
                true,
            ));
        }

        // Halt v1: refuse write/dispatch tools; read tools keep working.
        if is_halt_blocked_tool(name) && crate::services::halt::is_halted(&self.store).await {
            return Ok(tool_result(
                crate::services::halt::halted_error_payload(),
                true,
            ));
        }

        let (payload, is_error) = match name {
            "lounge_search_experience" => match self.tool_search(&args).await {
                Ok(v) => (v, false),
                Err(err) => (json!({ "error": err.to_string() }), true),
            },
            "lounge_record_experience" | "lounge_record_decision" => {
                match self.tool_record(&args, client).await {
                    Ok(v) => (v, false),
                    Err(err) => (json!({ "error": err.to_string() }), true),
                }
            }
            "lounge_ask_agent" | "lounge_dispatch_task" => {
                match self.tool_dispatch(&args, client, request_key).await {
                    Ok(v) => (v, false),
                    Err(err) => (json!({ "error": err.to_string() }), true),
                }
            }
            "lounge_call_agent" => {
                match self
                    .tool_call_agent(&args, client, request_key, progress_token.clone())
                    .await
                {
                    Ok(v) => (v, false),
                    Err(err) => (json!({ "error": err.to_string() }), true),
                }
            }
            "lounge_wait_task" => {
                match self
                    .tool_wait_task(&args, client, request_key, progress_token.clone())
                    .await
                {
                    Ok(v) => (v, false),
                    Err(err) => (json!({ "error": err.to_string() }), true),
                }
            }
            "lounge_yield_result" => match self.tool_yield_result(&args, client).await {
                Ok(v) => (v, false),
                Err(err) => (json!({ "error": err.to_string() }), true),
            },
            "lounge_status" => match self.tool_status(&args, client).await {
                Ok(v) => (v, false),
                Err(err) => (json!({ "error": err.to_string() }), true),
            },
            "lounge_health" => match self.tool_lounge_health(&args).await {
                Ok(v) => (v, false),
                Err(err) => (json!({ "error": err.to_string() }), true),
            },
            "lounge_list_my_tasks" => match self.tool_list_my_tasks(&args, client).await {
                Ok(v) => (v, false),
                Err(err) => (json!({ "error": err.to_string() }), true),
            },
            other => (
                json!({ "error": format!("bilinmeyen tool: {other}") }),
                true,
            ),
        };
        Ok(self.tool_result_with_piggyback(client, payload, is_error))
    }

    /// Tüm tools/call yanıtlarına `_meta.pending_results` ekler (Mcp-Session-Id filtreli).
    fn tool_result_with_piggyback(
        &self,
        client: &ClientCtx,
        payload: Value,
        is_error: bool,
    ) -> Value {
        let mut result = tool_result(payload, is_error);
        if client.session_id.trim().is_empty() {
            return result;
        }
        let pending = self
            .store
            .list_pending_results_for_session(&client.session_id, 8)
            .unwrap_or_default();
        if pending.is_empty() {
            return result;
        }
        if let Some(obj) = result.as_object_mut() {
            obj.insert(
                "_meta".into(),
                json!({
                    "pending_results": pending,
                    "hint": "Hazır sonuçlar için lounge_wait_task(task_id) çağırın; poll_after_secs'e uyun."
                }),
            );
        }
        result
    }

    async fn tool_list_my_tasks(&self, args: &Value, client: &ClientCtx) -> Result<Value> {
        let args = if args.is_null() {
            json!({})
        } else {
            args.clone()
        };
        validate_schema(SchemaKind::McpListMyTasks, &args).map_err(|e| anyhow!(e))?;
        let include_finished = args
            .get("include_finished")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(50)
            .clamp(1, 100) as usize;
        let workspace = arg_str(&args, "workspace_path")
            .or_else(|| arg_str(&args, "workspace_root"))
            .map(str::to_string)
            .or_else(|| {
                if client.workspace_path.trim().is_empty() {
                    None
                } else {
                    Some(client.workspace_path.clone())
                }
            });
        let tasks = self.store.list_my_a2a_tasks(
            &client.session_id,
            workspace.as_deref(),
            include_finished,
            limit,
        )?;
        Ok(json!({
            "source_session_id": client.session_id,
            "workspace_path": workspace,
            "count": tasks.len(),
            "tasks": tasks,
            "hint": "Aynı Mcp-Session-Id sahipliği. Sonuç için lounge_wait_task; özet için sonraki tool yanıtlarında _meta.pending_results."
        }))
    }

    async fn tool_search(&self, args: &Value) -> Result<Value> {
        validate_schema(SchemaKind::McpSearch, args).map_err(|e| anyhow!(e))?;
        let query = arg_str(args, "query")
            .ok_or_else(|| anyhow!("query gerekli"))?
            .to_string();
        let project = self.resolve_project_arg(args).await?;
        let restrict = args
            .get("restrict_to_project")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        let (mut scored, from_archive) = self
            .store
            .search_experiences_with_archive_fallback(query.clone(), limit.or(Some(24)))
            .await?;

        if restrict {
            if let Some(ref project_id) = project {
                scored.retain(|(row, _)| row.project_id == *project_id);
            }
        }

        // VR4 / UD-5 / Vault-1: hybrid score computed once at retrieval; only apply
        // the #119 total order here (same-project → buckets → reviewed → …).
        crate::db::sort_scored_experiences_for_search(
            &mut scored,
            if restrict { None } else { project.as_deref() },
        );

        if let Some(cap) = limit.or(Some(12)) {
            scored.truncate(cap);
        }

        for (row, _) in &scored {
            let _ = self.store.bump_experience_usage(row.id.clone()).await;
        }

        let experiences: Vec<Value> = scored
            .iter()
            .map(|(row, _)| {
                let same_project = project
                    .as_ref()
                    .map(|pid| row.project_id == *pid)
                    .unwrap_or(false);
                let mut tags = row.tags.clone();
                if from_archive && !tags.iter().any(|t| t == "archived") {
                    tags.push("archived".into());
                }
                json!({
                    "id": row.id,
                    "type": row.msg_type,
                    "agent": row.agent,
                    "project_id": row.project_id,
                    "adr_summary": row.adr_summary,
                    "outcome": row.outcome,
                    "related_task_id": row.related_task_id,
                    "tags": tags,
                    "created_at": row.created_at,
                    "same_project": same_project,
                    "archived": from_archive,
                    "reviewed": row.reviewed,
                })
            })
            .collect();

        Ok(json!({
            "query": query,
            "project_id": project,
            "restrict_to_project": restrict,
            "count": experiences.len(),
            "from_archive": from_archive,
            "experiences": experiences,
        }))
    }

    async fn tool_record(&self, args: &Value, client: &ClientCtx) -> Result<Value> {
        validate_schema(SchemaKind::McpRecord, args).map_err(|e| anyhow!(e))?;
        let bind = self
            .resolve_project_bind(args, Some(client))
            .await?
            .ok_or_else(|| {
                anyhow!(
                    "project_id çözülemedi — project / project_id verin veya active_file / workspace_root ile project_index eşleşsin"
                )
            })?;
        let project = bind.canonical_id.clone();
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
            .unwrap_or_else(|| client.name.clone());

        let topic_raw = if context.is_empty() {
            decision.clone()
        } else {
            context.clone()
        };
        let solution_raw = if decision.is_empty() {
            context.clone()
        } else {
            decision.clone()
        };

        // Structured ADR (optional). Absent → adr_record stays empty (UI: No ADR).
        let adr_raw = args
            .get("adr")
            .and_then(|v| {
                super::hygiene::format_adr(
                    v.get("context").and_then(|x| x.as_str()),
                    v.get("decision").and_then(|x| x.as_str()),
                    v.get("consequences").and_then(|x| x.as_str()),
                    v.get("alternatives").and_then(|x| x.as_str()),
                )
            })
            .unwrap_or_default();

        // Hygiene before persist (mask, do not reject).
        let topic_h = super::hygiene::scrub(&topic_raw);
        let solution_h = super::hygiene::scrub(&solution_raw);
        let adr_h = super::hygiene::scrub(&adr_raw);
        let mut warnings = Vec::new();
        let mut hygiene_masked = false;
        for part in [&topic_h, &solution_h, &adr_h] {
            if part.masked {
                hygiene_masked = true;
                for w in &part.warnings {
                    if !warnings.iter().any(|x| x == w) {
                        warnings.push(w.clone());
                    }
                }
            }
        }

        // Agent tags only + normalization; mcp/external → source metadata.
        let raw_tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let (tags, mut source_parts) = super::hygiene::normalize_tags(&raw_tags);
        if source_parts.is_empty() {
            source_parts = vec!["mcp".into(), "external".into()];
        } else if !source_parts.iter().any(|s| s == "mcp") {
            source_parts.insert(0, "mcp".into());
        }
        // Provenance column format: comma-separated lowercase tokens (e.g. `mcp,external`).
        let source = super::hygiene::format_source_metadata(&source_parts);

        let agent_title = arg_str(args, "title").map(str::to_string);
        let (title, title_source) = if let Some(t) = agent_title {
            let cleaned = super::hygiene::scrub(&t);
            if cleaned.masked {
                hygiene_masked = true;
                for w in &cleaned.warnings {
                    if !warnings.iter().any(|x| x == w) {
                        warnings.push(w.clone());
                    }
                }
            }
            let title =
                super::hygiene::fallback_title(&cleaned.text, crate::models::TITLE_MAX_CHARS);
            (title, crate::models::TitleSource::Agent)
        } else {
            (
                super::hygiene::fallback_title(&solution_h.text, crate::models::TITLE_MAX_CHARS),
                crate::models::TitleSource::Fallback,
            )
        };

        let experience = LoungeExperience {
            id: Uuid::new_v4().to_string(),
            msg_type: "experience".into(),
            agent: agent.clone(),
            project_id: project.clone(),
            adr_summary: adr_h.text.clone(),
            outcome: outcome.clone(),
            related_task_id: None,
            tags: tags.clone(),
            created_at: now_rfc3339(),
            reviewed: false,
            title: title.clone(),
            title_source,
            source: source.clone(),
            original_content: Some(solution_h.text.clone()),
            ..Default::default()
        };
        let experience_json = serde_json::to_value(&experience)?;
        validate_schema(SchemaKind::Experience, &experience_json).map_err(|e| anyhow!(e))?;

        let mut record = ExperienceRecord::from_lounge(
            &experience,
            topic_h.text.clone(),
            solution_h.text.clone(),
        );
        // O6: MCP rows are auto-approved (active) but unreviewed.
        // Hygiene mask → stay in reviewed=0 queue (already false).
        record.status = crate::models::EXPERIENCE_STATUS_ACTIVE.into();
        record.reviewed = false;
        let id = record.id.clone();
        let enqueue_content = record.solution_summary.clone();
        let enqueue_source = record.title_source;
        self.store.insert_record_atomic(record).await?;

        // Async title generation (never overwrites agent titles).
        crate::services::experience_title::enqueue_title(
            &self.store,
            &id,
            &enqueue_content,
            enqueue_source,
        )
        .await;

        // Dashboard sayacı / event pump: lounge.experience.reported
        if let Err(err) = self.publish_experience_reported(&experience).await {
            eprintln!("[lounge-mcp] experience.reported yayınlanamadı: {err}");
        }

        let mut out = json!({
            "id": id,
            "project_id": project,
            "display_name": bind.display_name,
            "identity_kind": bind.kind.as_str(),
            "agent": agent,
            "topic": topic_h.text,
            "title": title,
            "title_source": title_source.as_str(),
            "source": source,
            "recorded": true,
            "atomic": true,
            "event": EXPERIENCE_REPORTED,
            "hygiene_masked": hygiene_masked,
        });
        if !warnings.is_empty() {
            out["hygiene_warnings"] = json!(warnings);
        }
        if let Some(warning) = bind.warning {
            out.as_object_mut()
                .expect("object")
                .insert("project_warning".into(), json!(warning));
        }
        Ok(out)
    }

    async fn publish_experience_reported(&self, experience: &LoungeExperience) -> Result<()> {
        if !probe_tcp_host_port(&self.nats_url) {
            return Ok(());
        }
        let url = self.nats_url.clone();
        let subject = EXPERIENCE_REPORTED.to_string();
        let bytes = serde_json::to_vec(experience)?;
        tokio::task::spawn_blocking(move || {
            #[allow(deprecated)]
            let nc =
                crate::services::nats_connect(&url).map_err(|e| anyhow!("NATS connect: {e}"))?;
            nc.publish(&subject, bytes)
                .map_err(|e| anyhow!("NATS publish: {e}"))?;
            nc.flush().map_err(|e| anyhow!("NATS flush: {e}"))?;
            Ok::<(), anyhow::Error>(())
        })
        .await
        .context("experience.reported join")??;
        Ok(())
    }

    /// Geriye dönük: fire-and-forget; yeni altyapı (oturum + idempotency + admit).
    async fn tool_dispatch(
        &self,
        args: &Value,
        client: &ClientCtx,
        request_key: &str,
    ) -> Result<Value> {
        let normalized = normalize_dispatch_args(args);
        validate_schema(SchemaKind::McpDispatch, &normalized).map_err(|e| anyhow!(e))?;
        let call_args = self.build_call_args(&normalized, /*wait*/ false).await?;

        if !self.orchestrator_skips_nats() && !probe_tcp_host_port(&self.nats_url) {
            // P2(e): sahte UUID yok — açık durum; wait_task yetkisiz tuzağına düşmez.
            return Ok(json!({
                "published": false,
                "error": format!(
                    "NATS erişilemiyor ({}) — Lounge Kernel / NATS ayakta olmalı",
                    self.nats_url
                ),
                "status": "nats_unavailable",
                "subject": TASK_REQUESTED,
                "target_agent": call_args.target_agent,
                "project_id": call_args.project_id,
                "summary": call_args.task,
                "note": "Görev kabul edilmedi (NATS kapalı). task_id yok — lounge_wait_task çağırma."
            }));
        }

        let cancel_rx = self.register_inflight(request_key).await;
        let result = self
            .orchestrator
            .call_agent(&client.session_id, &client.name, call_args, Some(cancel_rx))
            .await;
        self.clear_inflight(request_key).await;
        let mut payload = result?;
        if let Some(obj) = payload.as_object_mut() {
            obj.entry("note".to_string()).or_insert(json!(
                "Görev NATS'a yazıldı. Kernel DecisionGate / security (PENDING_APPROVAL) / quota uygular; UI yoksa onay bekleyen işler kalabilir. Sonuç: lounge_wait_task."
            ));
        }
        Ok(payload)
    }

    async fn tool_call_agent(
        &self,
        args: &Value,
        client: &ClientCtx,
        request_key: &str,
        progress_token: Option<Value>,
    ) -> Result<Value> {
        let normalized = normalize_dispatch_args(args);
        validate_schema(SchemaKind::McpCallAgent, &normalized).map_err(|e| anyhow!(e))?;
        let wait = normalized
            .get("wait")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        let call_args = self.build_call_args(&normalized, wait).await?;

        if wait && !self.orchestrator_skips_nats() && !probe_tcp_host_port(&self.nats_url) {
            return Ok(json!({
                "published": false,
                "error": format!(
                    "NATS erişilemiyor ({}) — Lounge Kernel / NATS ayakta olmalı",
                    self.nats_url
                ),
                "subject": TASK_REQUESTED,
            }));
        }

        let cancel_rx = self.register_inflight(request_key).await;
        let profile = self.timeouts.profile_for(&client.name);
        let sink: Arc<dyn ProgressSink> = if progress_token.is_some() && profile.progress_extends {
            Arc::new(LogProgressSink {
                session_id: client.session_id.clone(),
            })
        } else {
            Arc::new(NoopProgressSink)
        };
        let opts = WaitOpts {
            progress_token,
            progress_sink: sink,
        };
        let result = self
            .orchestrator
            .call_agent_with_opts(
                &client.session_id,
                &client.name,
                call_args,
                Some(cancel_rx),
                opts,
            )
            .await;
        self.clear_inflight(request_key).await;
        result
    }

    async fn tool_wait_task(
        &self,
        args: &Value,
        client: &ClientCtx,
        request_key: &str,
        progress_token: Option<Value>,
    ) -> Result<Value> {
        validate_schema(SchemaKind::McpWaitTask, args).map_err(|e| anyhow!(e))?;
        let task_id = arg_str(args, "task_id")
            .ok_or_else(|| anyhow!("task_id gerekli"))?
            .to_string();
        let timeout_ms = args.get("timeout_ms").and_then(|v| v.as_u64());
        let task_token = arg_str(args, "task_token").map(str::to_string);
        let cancel_rx = self.register_inflight(request_key).await;
        let profile = self.timeouts.profile_for(&client.name);
        let sink: Arc<dyn ProgressSink> = if progress_token.is_some() && profile.progress_extends {
            Arc::new(LogProgressSink {
                session_id: client.session_id.clone(),
            })
        } else {
            Arc::new(NoopProgressSink)
        };
        let opts = WaitOpts {
            progress_token,
            progress_sink: sink,
        };
        let result = self
            .orchestrator
            .wait_task_with_opts(
                &client.session_id,
                &client.name,
                &task_id,
                timeout_ms,
                Some(cancel_rx),
                opts,
                task_token.as_deref(),
            )
            .await;
        self.clear_inflight(request_key).await;
        result
    }

    async fn tool_yield_result(&self, args: &Value, client: &ClientCtx) -> Result<Value> {
        validate_schema(SchemaKind::McpYieldResult, args).map_err(|e| anyhow!(e))?;
        let task_id = arg_str(args, "task_id")
            .ok_or_else(|| anyhow!("task_id gerekli"))?
            .to_string();
        let status = arg_str(args, "status").unwrap_or("completed");
        let output = args.get("output").cloned().unwrap_or(json!(null));
        self.orchestrator
            .yield_result(
                &client.session_id,
                &task_id,
                status,
                &output,
                args.get("artifacts"),
                args.get("metrics"),
            )
            .await
    }

    async fn build_call_args(&self, normalized: &Value, wait: bool) -> Result<CallAgentArgs> {
        let target = arg_str(normalized, "target_agent")
            .ok_or_else(|| anyhow!("target_agent gerekli"))?
            .to_string();
        let task_text = arg_str(normalized, "task")
            .ok_or_else(|| anyhow!("task gerekli"))?
            .to_string();
        let project = self
            .resolve_project_arg(normalized)
            .await?
            .unwrap_or_else(|| "agent-lounge-os".into());
        Ok(CallAgentArgs {
            target_agent: target,
            task: task_text,
            project_id: project,
            idempotency_key: arg_str(normalized, "idempotency_key").map(str::to_string),
            repo_path: arg_str(normalized, "repo_path")
                .or_else(|| arg_str(normalized, "workspace_root"))
                .map(str::to_string),
            parent_task_id: None,
            wait,
            long_running: normalized
                .get("long_running")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            must_deliver: normalized
                .get("must_deliver")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        })
    }

    async fn register_inflight(&self, request_key: &str) -> watch::Receiver<Option<CancelKind>> {
        let (tx, rx) = watch::channel(None);
        let mut map = self.in_flight.lock().await;
        let mut order = self.in_flight_order.lock().await;
        // Global FIFO üst sınır.
        while map.len() >= super::session_id::MAX_IN_FLIGHT {
            if let Some(evict) = order.pop_front() {
                map.remove(&evict);
            } else {
                break;
            }
        }
        // Oturum başına FIFO.
        let session_prefix = request_key
            .split_once(':')
            .map(|(s, _)| format!("{s}:"))
            .unwrap_or_default();
        if !session_prefix.is_empty() {
            let session_count = order
                .iter()
                .filter(|k| k.starts_with(&session_prefix))
                .count();
            if session_count >= super::session_id::MAX_IN_FLIGHT_PER_SESSION {
                if let Some(pos) = order.iter().position(|k| k.starts_with(&session_prefix)) {
                    let evict = order.remove(pos).unwrap();
                    map.remove(&evict);
                }
            }
        }
        if let Some(pos) = order.iter().position(|k| k == request_key) {
            order.remove(pos);
        }
        order.push_back(request_key.to_string());
        map.insert(request_key.to_string(), tx);
        rx
    }

    async fn clear_inflight(&self, request_key: &str) {
        self.in_flight.lock().await.remove(request_key);
        let mut order = self.in_flight_order.lock().await;
        if let Some(pos) = order.iter().position(|k| k == request_key) {
            order.remove(pos);
        }
    }

    fn orchestrator_skips_nats(&self) -> bool {
        self.orchestrator.skips_nats()
    }

    async fn tool_lounge_health(&self, args: &Value) -> Result<Value> {
        let args = if args.is_null() {
            json!({})
        } else {
            args.clone()
        };
        validate_schema(SchemaKind::McpLoungeHealth, &args).map_err(|e| anyhow!(e))?;
        let report = crate::services::collect_lounge_health(&self.store).await;
        let value = serde_json::to_value(&report).context("lounge_health serialize")?;
        // Soft-validate response shape (snapshot lock).
        if let Err(err) = validate_schema(SchemaKind::LoungeHealthReport, &value) {
            log::warn!("lounge_health report schema: {err}");
        }
        Ok(value)
    }

    async fn tool_status(&self, args: &Value, client: &ClientCtx) -> Result<Value> {
        let args = if args.is_null() {
            json!({})
        } else {
            args.clone()
        };
        validate_schema(SchemaKind::McpStatus, &args).map_err(|e| anyhow!(e))?;

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

        let workers = if let Some(registry) = &self.workers {
            workers_status_json(registry)
        } else {
            // Standalone: connected_tools kind=worker satırlarından türet.
            let rows = self
                .store
                .list_connected_tools()
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|t| t.kind == "worker")
                .map(|t| {
                    let online = t.is_active
                        && t.enabled
                        && t.payload
                            .get("available")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                    let bot_id = t
                        .payload
                        .get("host_id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            t.id.strip_prefix("worker:")
                                .unwrap_or(t.name.as_str())
                                .to_string()
                        });
                    json!({
                        "bot_id": bot_id,
                        "name": t.name,
                        "capabilities": [],
                        "version": "",
                        "pid": 0,
                        "online": online,
                        "last_heartbeat": t.last_synced,
                        "tasks_subject": t.endpoint,
                    })
                })
                .collect::<Vec<_>>();
            json!(rows)
        };

        let nats_up = probe_tcp_host_port(&self.nats_url);
        let lmr = lounge_ollama_endpoint();
        let lmr_up = probe_lmr_reachable(&lmr).await;
        let system_ollama = system_ollama_endpoint();
        let http_up = probe_tcp_host_port(&format!("tcp://{}", default_mcp_http_bind()));

        Ok(json!({
            "server": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION,
                "http": default_mcp_http_url(),
                "http_reachable": http_up,
                "client": client.label(),
            },
            "security_boundary": {
                "filesystem_writes": false,
                "settings_mutation": false,
                "note": "Kota / politika / routing yalnızca Tauri UI; MCP salt okunur orkestrasyon + hafıza"
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
            "timeout_manager": self.timeouts.snapshot(),
            "timeout_limit_secs": self.timeouts.timeout_limit(&client.name).as_secs(),
            "client_profile": self.timeouts.profile_for(&client.name).name,
            "connected_agents": connected,
            "workers": workers,
            "dispatch_requires_kernel": true,
            "hint": "lounge_call_agent / lounge_dispatch_task için NATS + Kernel gerekir. Sonuç: lounge_wait_task (aynı oturum). Cursor: progressToken ile Mod A ≤280sn."
        }))
    }

    async fn resolve_project_arg(&self, args: &Value) -> Result<Option<String>> {
        Ok(self
            .resolve_project_bind(args, None)
            .await?
            .map(|b| b.canonical_id))
    }

    /// Project-ID-Core write binding: aliases + path → canonical id, else sentinel.
    ///
    /// Path hints: `active_file` / `workspace_root` / `repo_path` plus optional
    /// session `ClientCtx.workspace_path`. When an explicit project id resolves to
    /// a *different* indexed canonical than those paths, the write is stored as
    /// `unclassified` (not path-wins) so the conflict is visible.
    async fn resolve_project_bind(
        &self,
        args: &Value,
        client: Option<&ClientCtx>,
    ) -> Result<Option<ProjectBind>> {
        let explicit = arg_str(args, "project_id").or_else(|| arg_str(args, "project"));
        let mut hints: Vec<String> = Vec::new();
        for key in ["active_file", "workspace_root", "repo_path"] {
            if let Some(path) = arg_str(args, key) {
                hints.push(path.to_string());
            }
        }
        if let Some(client) = client {
            let ws = client.workspace_path.trim();
            if !ws.is_empty() {
                hints.push(ws.to_string());
            }
        }
        let store = self.store.clone();
        let explicit_owned = explicit.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = store.conn.lock().expect("experience db lock");
            let hint_refs: Vec<&str> = hints.iter().map(|s| s.as_str()).collect();
            bind_write_project_id(&conn, explicit_owned.as_deref(), &hint_refs, |path| {
                match resolve_path_hint_for_bind(&conn, path)? {
                    Some(id) => prefer_canonical(&conn, &id).map(Some),
                    None => Ok(None),
                }
            })
        })
        .await
        .context("resolve_project_bind join")?
    }
}

/// Shared with `project_index::resolve_project_id_blocking` logic without async.
fn resolve_path_hint_for_bind(
    conn: &rusqlite::Connection,
    path_hint: &str,
) -> Result<Option<String>> {
    // Delegate via store helper path: duplicate longest-match using public normalize.
    use crate::db::normalize_path_str;
    use std::path::PathBuf;

    let needle = PathBuf::from(normalize_path_str(path_hint));
    if needle.as_os_str().is_empty() {
        return Ok(None);
    }
    let mut stmt = conn.prepare(
        r#"
        SELECT DISTINCT project_id, repo_path, file_path
        FROM project_index
        WHERE TRIM(repo_path) != '' OR (file_path IS NOT NULL AND TRIM(file_path) != '')
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut best: Option<(usize, String)> = None;
    for row in rows {
        let (project_id, repo_path, file_path) = row?;
        for candidate in [Some(repo_path.as_str()), file_path.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.trim().is_empty())
        {
            let root = PathBuf::from(normalize_path_str(candidate));
            if root.as_os_str().is_empty() {
                continue;
            }
            if needle == root || needle.starts_with(&root) {
                let score = root.as_os_str().len();
                if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                    best = Some((score, project_id.clone()));
                }
            }
        }
    }
    Ok(best.map(|(_, id)| id))
}

fn is_forbidden_tool(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("write_file")
        || lower.contains("set_routing")
        || lower.contains("set_quota")
        || lower.contains("set_policy")
        || lower.contains("delete_file")
        || lower.contains("fs_write")
}

/// Write / dispatch tools refused while halted. Read tools keep working.
fn is_halt_blocked_tool(name: &str) -> bool {
    matches!(
        name,
        "lounge_dispatch_task"
            | "lounge_ask_agent"
            | "lounge_call_agent"
            | "lounge_record_experience"
            | "lounge_record_decision"
    )
}

/// `MCP_RPC:<code>:<message>` — must_deliver kota vb.
fn mcp_rpc_code_message(err: &anyhow::Error) -> (i32, String) {
    let s = err.to_string();
    if let Some(rest) = s.strip_prefix("MCP_RPC:") {
        if let Some((code_s, msg)) = rest.split_once(':') {
            if let Ok(code) = code_s.parse::<i32>() {
                return (code, msg.to_string());
            }
        }
    }
    (-32000, s)
}

/// Cursor progress heartbeat — stderr log (+ ileride stdio notification çıkışı).
struct LogProgressSink {
    session_id: String,
}

impl ProgressSink for LogProgressSink {
    fn emit_progress(&self, progress_token: &Value, message: &str, elapsed_secs: u64) {
        eprintln!(
            "[lounge-mcp] progress session={} token={} elapsed={}s msg={message}",
            self.session_id, progress_token, elapsed_secs
        );
    }
}

fn resource_defs() -> Vec<Value> {
    vec![json!({
        "uri": A2A_GUIDE_URI,
        "name": "a2a-guide",
        "title": "Agent Lounge A2A Guide",
        "description": "Timeout, polling, backgrounded, task_token, piggyback pending_results — Grok Bot / Cursor / Antigravity / Claude Desktop.",
        "mimeType": A2A_GUIDE_MIME,
    })]
}

fn tool_defs(client: &ClientCtx, timeouts: &TimeoutManager) -> Vec<Value> {
    let profile = timeouts.profile_for(&client.name);
    let threshold = profile.threshold_secs;
    let call_desc = format!(
        "Mod A→B: başka ajana görev ver. Bu istemci ({profile}) için Mod A bekleme eşiği ~{threshold}s \
(Antigravity ~150s, Grok/unknown ~45s, Cursor ~100s/progress≤280s, Claude Desktop ~210s — ClientProfile). \
Eşik dolunca backgrounded+task_id; long_running=true ise eşiği beklemeden hemen backgrounded. \
poll_after_secs ve next_action'a uyun (15→×1.5→≤60); sonsuz lounge_wait_task döngüsü kurmayın. \
Sonuç gelmeden kullanıcıya bitti demeyin. task_token'ı saklayın. must_deliver: kopmada iptal yok. \
Kota aşımında -32029. Ayrıntı: resources/read {A2A_GUIDE_URI}.",
        profile = profile.name,
        threshold = threshold,
    );
    vec![
        tool_def(
            "lounge_search_experience",
            "Geçmiş tecrübe / ADR ara. project_id yerine active_file / workspace_root verebilirsiniz.",
            include_schema("mcp_search_experience.schema.json"),
        ),
        tool_def(
            "lounge_record_experience",
            "Tecrübe kaydı (SQLite + vektör atomik). Kota/politika değiştirmez; dosya yazmaz. \
Mimari kararlar için isteğe bağlı `adr` {context,decision,consequences,alternatives} doldurun; \
yoksa ADR boş kalır (UI: No ADR). `tags` ajan etiketleridir (normalize edilir; mcp/external → source). \
İsteğe bağlı `title` (≤80) LLM tarafından ezilmez.",
            include_schema("mcp_record_experience.schema.json"),
        ),
        tool_def(
            "lounge_record_decision",
            "lounge_record_experience alias — ADR kaydı. Mimari kararlar için `adr` alanını doldurun.",
            include_schema("mcp_record_experience.schema.json"),
        ),
        tool_def(
            "lounge_call_agent",
            &call_desc,
            include_schema("mcp_call_agent.schema.json"),
        ),
        tool_def(
            "lounge_wait_task",
            "Aynı oturum veya geçerli task_token ile sonuç long-poll. still_running ise next_action/poll_after_secs'e uyun (15→×1.5→≤60). Arka plan görevlerini düzenli kontrol edin. Sonuç completed/failed olmadan kullanıcıya bitti demeyin. Claude Desktop: progress yok — piggyback `_meta.pending_results` veya bu tool.",
            include_schema("mcp_wait_task.schema.json"),
        ),
        tool_def(
            "lounge_yield_result",
            "Worker oturumu görev sonucunu yazar (claim lease). Yetkisiz oturum reddedilir.",
            include_schema("mcp_yield_result.schema.json"),
        ),
        tool_def(
            "lounge_dispatch_task",
            "NATS lounge.task.requested → Kernel (fire-and-forget). Sonuç: lounge_wait_task. Arka plan görevlerini poll_after_secs'e uyarak düzenli kontrol edin.",
            include_schema("mcp_dispatch_task.schema.json"),
        ),
        tool_def(
            "lounge_ask_agent",
            "lounge_dispatch_task alias.",
            include_schema("mcp_dispatch_task.schema.json"),
        ),
        tool_def(
            "lounge_list_my_tasks",
            "Bu Mcp-Session-Id (source_session_id) + isteğe bağlı workspace_path sahipliğindeki aktif/bitmiş görevleri listeler. Süreç takibi; tam sonuç için lounge_wait_task.",
            include_schema("mcp_list_my_tasks.schema.json"),
        ),
        tool_def(
            "lounge_status",
            "Bağlı ajanlar + NATS/LMR/MCP HTTP sağlık + timeout_limit. Ayar değiştirmez. Yanıtta `_meta.pending_results` piggyback olabilir.",
            include_schema("mcp_status.schema.json"),
        ),
        tool_def(
            "lounge_health",
            "Health-1b birleşik sağlık (şema 1.1.0): dead symbols, ajan lounge-mcp config (salt okunur; her config yolu ayrı rapor), title/LMR backlog, CBM daemon + 18749 pollution tespiti (düzeltmez). URL-only girişler unverified. Ev yolları ~/… maskeli. Ayar/config yazmaz. Not: title.queue_backlog ve last_error_type bu süreçte localdir — standalone lounge-mcp'te 0/null; Tauri lounge_health canlı worker değerlerini verir. db_backlog/give_ups SQLite.",
            include_schema("mcp_lounge_health.schema.json"),
        ),
    ]
}

fn include_schema(file: &str) -> Value {
    let raw = match file {
        "mcp_search_experience.schema.json" => {
            include_str!(
                "../../../shared/lounge_protocol/schemas/mcp_search_experience.schema.json"
            )
        }
        "mcp_record_experience.schema.json" => {
            include_str!(
                "../../../shared/lounge_protocol/schemas/mcp_record_experience.schema.json"
            )
        }
        "mcp_dispatch_task.schema.json" => {
            include_str!("../../../shared/lounge_protocol/schemas/mcp_dispatch_task.schema.json")
        }
        "mcp_call_agent.schema.json" => {
            include_str!("../../../shared/lounge_protocol/schemas/mcp_call_agent.schema.json")
        }
        "mcp_wait_task.schema.json" => {
            include_str!("../../../shared/lounge_protocol/schemas/mcp_wait_task.schema.json")
        }
        "mcp_yield_result.schema.json" => {
            include_str!("../../../shared/lounge_protocol/schemas/mcp_yield_result.schema.json")
        }
        "mcp_status.schema.json" => {
            include_str!("../../../shared/lounge_protocol/schemas/mcp_status.schema.json")
        }
        "mcp_list_my_tasks.schema.json" => {
            include_str!("../../../shared/lounge_protocol/schemas/mcp_list_my_tasks.schema.json")
        }
        "mcp_lounge_health.schema.json" => {
            include_str!("../../../shared/lounge_protocol/schemas/mcp_lounge_health.schema.json")
        }
        other => panic!("bilinmeyen schema: {other}"),
    };
    serde_json::from_str(raw).expect("schema json")
}

fn normalize_dispatch_args(args: &Value) -> Value {
    let mut normalized = args.clone();
    if let Some(obj) = normalized.as_object_mut() {
        if !obj.contains_key("target_agent") {
            if let Some(agent) = obj.get("agent").cloned() {
                obj.insert("target_agent".into(), agent);
            }
        }
        if !obj.contains_key("task") {
            if let Some(summary) = obj.get("summary").cloned() {
                obj.insert("task".into(), summary);
            }
        }
    }
    normalized
}

fn request_id_key(session_id: &str, id: &Value) -> String {
    format!("{session_id}:{}", id)
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

/// clientInfo.name → dashboard host id (`cursor`, `claude_ai`, `claude_code`, …).
pub fn normalize_client_host(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    // Daha spesifik Claude eşleşmeleri önce (hepsi "claude" içerir).
    if lower.contains("claude-code") || lower.contains("claude_code") || lower == "claude-code" {
        return "claude_code".into();
    }
    if lower.contains("claude-ai") || lower.contains("claude_ai") || lower == "claude-ai" {
        return "claude_ai".into();
    }
    if lower.contains("claude") {
        // Claude Desktop / legacy "Claude Desktop" → claude_desktop (profil: claude-ai).
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

pub fn default_mcp_http_bind() -> String {
    std::env::var("LOUNGE_MCP_BIND")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_MCP_HTTP_BIND.into())
}

pub fn default_mcp_http_url() -> String {
    if let Ok(url) = std::env::var("LOUNGE_MCP_URL") {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return trimmed.trim_end_matches('/').to_string();
        }
    }
    format!("http://{}", default_mcp_http_bind())
}

/// Stdio→HTTP proxy reqwest timeout (sn). Varsayılan 320 (≥ claude-code 300 + marj).
pub fn resolve_proxy_timeout_secs() -> u64 {
    std::env::var("LOUNGE_MCP_PROXY_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map(|n| n.min(900))
        .unwrap_or(320)
}

pub fn mcp_http_reachable() -> bool {
    probe_tcp_host_port(&format!("tcp://{}", default_mcp_http_bind()))
}

/// stdin/stdout — Kernel HTTP varsa proxy, yoksa gömülü sunucu.
pub async fn run_stdio(store: ExperienceStore, nats_url: impl Into<String>) -> Result<()> {
    let force_standalone = std::env::var("LOUNGE_MCP_STANDALONE")
        .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
        .unwrap_or(false);
    if !force_standalone && mcp_http_reachable() {
        eprintln!(
            "[lounge-mcp] Kernel HTTP proxy → {}",
            default_mcp_http_url()
        );
        return run_stdio_http_proxy().await;
    }
    eprintln!("[lounge-mcp] standalone (Kernel HTTP yok veya LOUNGE_MCP_STANDALONE=1)");
    run_stdio_embedded(store, nats_url).await
}

pub async fn run_stdio_embedded(store: ExperienceStore, nats_url: impl Into<String>) -> Result<()> {
    let server = Arc::new(McpServer::new(store, nats_url));
    let (line_tx, mut line_rx) = tokio::sync::mpsc::channel::<String>(64);
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<String>(64);

    // stdout yazıcı — yanıt sırası serbest (MCP eşzamanlı isteklere izin verir).
    let writer = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut stdout = tokio::io::stdout();
        while let Some(response) = out_rx.recv().await {
            let line = format!("{response}\n");
            if stdout.write_all(line.as_bytes()).await.is_err() {
                break;
            }
            let _ = stdout.flush().await;
        }
    });

    // stdin okuyucu — istek işlenirken de cancel satırları gelir.
    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        let stdin = std::io::stdin();
        let locked = stdin.lock();
        for line in locked.lines() {
            let line = line.context("stdin okunamadı")?;
            if line_tx.blocking_send(line).is_err() {
                break;
            }
        }
        Ok(())
    });

    while let Some(line) = line_rx.recv().await {
        let server = Arc::clone(&server);
        let out_tx = out_tx.clone();
        tokio::spawn(async move {
            let mut client = server.client.clone();
            match server.handle_line_for(&line, &mut client).await {
                Ok(Some(response)) => {
                    let _ = out_tx.send(response).await;
                }
                Ok(None) => {}
                Err(err) => {
                    eprintln!("[lounge-mcp] handle_line: {err}");
                }
            }
        });
    }
    // stdin EOF — in-flight senkron beklemeleri iptal; backgrounded korunur.
    let n = server.on_transport_eof().await;
    if n > 0 {
        eprintln!("[lounge-mcp] stdio EOF: {n} in-flight iptal");
    }
    drop(out_tx);
    let _ = writer.await;
    reader.await.context("stdin join")??;
    Ok(())
}

/// Claude Desktop / Cursor stdio → Kernel `POST /mcp` köprüsü.
/// Her süreç kendi `Mcp-Session-Id` değerini taşır; initialize'daki clientInfo
/// sonraki isteklere `X-Lounge-Client-Name` ile de eklenir (kimlik karışması yok).
///
/// Okuma ile HTTP POST eşzamanlı: uzun tools/call sürerken `cancelled` okunur.
pub async fn run_stdio_http_proxy() -> Result<()> {
    let base = default_mcp_http_url();
    // En yüksek profil eşiği (claude-code 300) + marj — 290 eski değer Claude Code'u asılı bırakıyordu.
    let proxy_timeout_secs = resolve_proxy_timeout_secs();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(proxy_timeout_secs))
        .build()
        .context("HTTP client")?;
    let session_id = Uuid::new_v4().to_string();
    let client_meta = Arc::new(tokio::sync::Mutex::new((
        Option::<String>::None,
        Option::<String>::None,
    )));
    let (line_tx, mut line_rx) = tokio::sync::mpsc::channel::<String>(64);
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<String>(64);

    let writer = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let mut stdout = tokio::io::stdout();
        while let Some(line) = out_rx.recv().await {
            let one = format!("{}\n", line.replace('\n', " ").replace('\r', ""));
            if stdout.write_all(one.as_bytes()).await.is_err() {
                break;
            }
            let _ = stdout.flush().await;
        }
    });

    let reader = tokio::task::spawn_blocking(move || -> Result<()> {
        let stdin = std::io::stdin();
        let locked = stdin.lock();
        for line in locked.lines() {
            let line = line.context("stdin okunamadı")?;
            if line_tx.blocking_send(line).is_err() {
                break;
            }
        }
        Ok(())
    });

    while let Some(line) = line_rx.recv().await {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        if let Ok(val) = serde_json::from_str::<Value>(&line) {
            if val.get("method").and_then(|m| m.as_str()) == Some("initialize") {
                if let Some(info) = val.pointer("/params/clientInfo") {
                    let mut meta = client_meta.lock().await;
                    if let Some(name) = info.get("name").and_then(|v| v.as_str()) {
                        meta.0 = Some(name.to_string());
                    }
                    if let Some(ver) = info.get("version").and_then(|v| v.as_str()) {
                        meta.1 = Some(ver.to_string());
                    }
                }
            }
        }
        let client = client.clone();
        let base = base.clone();
        let session_id = session_id.clone();
        let client_meta = Arc::clone(&client_meta);
        let out_tx = out_tx.clone();
        // Cursor: progressToken varken proxy 10 sn heartbeat yazar (HTTP POST blokluyken).
        let progress_token = serde_json::from_str::<Value>(&line).ok().and_then(|v| {
            if v.get("method").and_then(|m| m.as_str()) == Some("tools/call") {
                v.pointer("/params/_meta/progressToken").cloned()
            } else {
                None
            }
        });
        let request_id = serde_json::from_str::<Value>(&line)
            .ok()
            .and_then(|v| v.get("id").cloned());
        tokio::spawn(async move {
            let meta = client_meta.lock().await.clone();
            let client_name = meta.0.clone().unwrap_or_default();
            let profile = super::timeout_manager::resolve_client_profile(&client_name);
            let emit_progress = progress_token.is_some() && profile.progress_extends;

            let url = format!("{base}/mcp");
            let mut req = client
                .post(&url)
                .header("content-type", "application/json")
                .header("mcp-session-id", &session_id);
            if let Some(ref name) = meta.0 {
                req = req.header("x-lounge-client-name", name);
            }
            if let Some(ref ver) = meta.1 {
                req = req.header("x-lounge-client-version", ver);
            }

            let send_fut = req.body(line).send();
            let result = if emit_progress {
                let token = progress_token.clone().unwrap();
                let out_hb = out_tx.clone();
                let hb_secs = profile.progress_heartbeat_secs.unwrap_or(10);
                tokio::pin!(send_fut);
                let mut ticks: u64 = 0;
                loop {
                    tokio::select! {
                        resp = &mut send_fut => break resp,
                        _ = tokio::time::sleep(Duration::from_secs(hb_secs)) => {
                            ticks += 1;
                            let elapsed = ticks * hb_secs;
                            let note = json!({
                                "jsonrpc": "2.0",
                                "method": "notifications/progress",
                                "params": {
                                    "progressToken": token,
                                    "progress": elapsed,
                                    "total": profile.progress_extended_secs.unwrap_or(280),
                                    "message": format!("lounge waiting ({elapsed}s)")
                                }
                            });
                            let _ = out_hb.send(note.to_string()).await;
                        }
                    }
                }
            } else {
                send_fut.await
            };

            match result {
                Ok(response) => {
                    let status = response.status();
                    if status == reqwest::StatusCode::NO_CONTENT {
                        return;
                    }
                    match response.text().await {
                        Ok(text) if !text.trim().is_empty() => {
                            let _ = out_tx.send(text).await;
                        }
                        Ok(_) if !status.is_success() => {
                            if let Some(id) = request_id {
                                let err = json!({
                                    "jsonrpc": "2.0",
                                    "id": id,
                                    "error": {
                                        "code": -32000,
                                        "message": format!("MCP HTTP {status}")
                                    }
                                });
                                let _ = out_tx.send(err.to_string()).await;
                            }
                        }
                        _ => {}
                    }
                    if !status.is_success() {
                        eprintln!("[lounge-mcp] HTTP {status}");
                    }
                }
                Err(err) => {
                    eprintln!("[lounge-mcp] MCP HTTP POST: {err}");
                    // İstemci asılı kalmasın — timeout/bağlantı hatasında JSON-RPC error.
                    if let Some(id) = request_id {
                        let err_body = json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32000,
                                "message": format!("MCP proxy: {err}")
                            }
                        });
                        let _ = out_tx.send(err_body.to_string()).await;
                    }
                }
            }
        });
    }
    // stdin EOF → Kernel'e oturum kapat.
    {
        let url = format!("{base}/mcp");
        let _ = client
            .delete(&url)
            .header("mcp-session-id", &session_id)
            .send()
            .await;
    }
    drop(out_tx);
    let _ = writer.await;
    reader.await.context("stdin join")??;
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

/// Experience DB / veri kökü — release'te app data (`LOUNGE_DATA_DIR` ezer).
pub fn workspace_root() -> PathBuf {
    crate::services::data_root()
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
        assert_eq!(normalize_client_host("claude-ai"), "claude_ai");
        assert_eq!(normalize_client_host("claude-code"), "claude_code");
        assert_eq!(normalize_client_host("antigravity-client"), "antigravity");
    }

    #[tokio::test]
    async fn rejects_unstructured_tool_args() {
        let store = ExperienceStore::memory().expect("db");
        let mut server = McpServer::new(store, "nats://127.0.0.1:9");
        let call = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"lounge_search_experience","arguments":{"query":"x","extra_freeform":true}}}"#,
            )
            .await
            .expect("handle")
            .expect("resp");
        let val: Value = serde_json::from_str(&call).unwrap();
        assert_eq!(val["result"]["isError"], true);
        let text = val["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("şema") || text.contains("additional") || text.contains("error"));
    }

    #[tokio::test]
    async fn auto_project_from_workspace_root() {
        use crate::db::canonicalize_repo_path;
        use crate::models::{AstNode, IndexGraph};
        let store = ExperienceStore::memory().expect("db");
        let root = std::env::temp_dir().join(format!("lounge-mcp-proj-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("src/main.rs");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "fn main() {}").unwrap();
        let graph = IndexGraph {
            project: "auto-demo".into(),
            repo_path: root.to_string_lossy().into_owned(),
            status: None,
            node_count: 1,
            edge_count: 0,
            files: Some(1),
            nodes: vec![AstNode {
                id: "main".into(),
                name: "main".into(),
                kind: "fn".into(),
                file: Some(file.to_string_lossy().into_owned()),
                line: Some(1),
                ref_count: 0,
            }],
            references: vec![],
            dead: vec![],
        };
        store.save_project_index(graph).await.expect("index");
        let expected = canonicalize_repo_path(&root.to_string_lossy());

        let mut server = McpServer::new(store.clone(), "nats://127.0.0.1:9");
        // serde_json escapes Windows `\` paths; raw format! does not.
        let args = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "lounge_record_experience",
                "arguments": {
                    "active_file": file.to_string_lossy(),
                    "context": "pool",
                    "decision": "use bb8"
                }
            }
        })
        .to_string();
        let resp = server
            .handle_line(&args)
            .await
            .expect("handle")
            .expect("resp");
        let val: Value = serde_json::from_str(&resp).unwrap();
        assert_ne!(val["result"]["isError"], true);
        assert_eq!(val["result"]["structuredContent"]["project_id"], expected);
        assert_eq!(
            val["result"]["structuredContent"]["display_name"],
            "auto-demo"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Unknown free-form slug (no indexed alias) → unclassified.
    #[tokio::test]
    async fn free_form_unknown_slug_stores_unclassified_with_warning() {
        let store = ExperienceStore::memory().expect("db");
        let mut server = McpServer::new(store.clone(), "nats://127.0.0.1:9");
        let args = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "lounge_record_experience",
                "arguments": {
                    "project_id": "totally-unknown-slug-xyz",
                    "context": "client profiles + A2A long_running/must_deliver + LMR :18790",
                    "decision": "document Agent Lounge OS orchestration flags"
                }
            }
        })
        .to_string();
        let resp = server
            .handle_line(&args)
            .await
            .expect("handle")
            .expect("resp");
        let val: Value = serde_json::from_str(&resp).unwrap();
        assert_ne!(val["result"]["isError"], true, "{val}");
        let body = &val["result"]["structuredContent"];
        assert_eq!(body["project_id"], crate::db::PROJECT_ID_UNCLASSIFIED);
        assert_eq!(body["identity_kind"], "unclassified");
        assert!(
            body["project_warning"]
                .as_str()
                .unwrap_or("")
                .contains("unclassified"),
            "warning missing: {body}"
        );
        let conn = store.conn.lock().unwrap();
        let pid: String = conn
            .query_row(
                "SELECT project_id FROM experiences ORDER BY created_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .expect("row");
        assert_eq!(pid, crate::db::PROJECT_ID_UNCLASSIFIED);
    }

    /// B1 / f5d084e2-class: photo-culling *is* indexed, but active_file lives under
    /// Agent-Lounge-OS → conflict → unclassified (not silent photo-culling bind).
    #[tokio::test]
    async fn indexed_photo_culling_explicit_conflicts_with_lounge_active_file() {
        use crate::db::canonicalize_repo_path;
        use crate::models::{AstNode, IndexGraph};
        let store = ExperienceStore::memory().expect("db");
        let photo = std::env::temp_dir().join(format!("mcp-photo-{}", Uuid::new_v4()));
        let lounge = std::env::temp_dir().join(format!("mcp-lounge-{}", Uuid::new_v4()));
        std::fs::create_dir_all(photo.join("src")).unwrap();
        std::fs::create_dir_all(lounge.join("src")).unwrap();
        let lounge_file = lounge.join("src/main.rs");
        std::fs::write(&lounge_file, "fn main() {}").unwrap();

        store
            .save_project_index(IndexGraph {
                project: "photo-culling".into(),
                repo_path: photo.to_string_lossy().into_owned(),
                status: None,
                node_count: 0,
                edge_count: 0,
                files: Some(0),
                nodes: vec![],
                references: vec![],
                dead: vec![],
            })
            .await
            .unwrap();
        store
            .save_project_index(IndexGraph {
                project: "Agent-Lounge-OS".into(),
                repo_path: lounge.to_string_lossy().into_owned(),
                status: None,
                node_count: 1,
                edge_count: 0,
                files: Some(1),
                nodes: vec![AstNode {
                    id: "main".into(),
                    name: "main".into(),
                    kind: "fn".into(),
                    file: Some(lounge_file.to_string_lossy().into_owned()),
                    line: Some(1),
                    ref_count: 0,
                }],
                references: vec![],
                dead: vec![],
            })
            .await
            .unwrap();

        let mut server = McpServer::new(store.clone(), "nats://127.0.0.1:9");
        let init = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "cursor",
                    "version": "1",
                    "workspace_path": lounge.to_string_lossy()
                }
            }
        })
        .to_string();
        let _ = server.handle_line(&init).await.unwrap();

        let args = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "lounge_record_experience",
                "arguments": {
                    "project_id": "photo-culling",
                    "active_file": lounge_file.to_string_lossy(),
                    "context": "client profiles + A2A long_running/must_deliver + LMR :18790",
                    "decision": "document Agent Lounge OS orchestration flags"
                }
            }
        })
        .to_string();
        let resp = server.handle_line(&args).await.unwrap().unwrap();
        let val: Value = serde_json::from_str(&resp).unwrap();
        assert_ne!(val["result"]["isError"], true, "{val}");
        let body = &val["result"]["structuredContent"];
        assert_eq!(body["project_id"], crate::db::PROJECT_ID_UNCLASSIFIED);
        assert_eq!(body["identity_kind"], "unclassified");
        let warning = body["project_warning"].as_str().unwrap_or("");
        assert!(
            warning.contains("photo-culling") && warning.contains("unclassified"),
            "conflict warning missing: {warning}"
        );
        let photo_canon = canonicalize_repo_path(&photo.to_string_lossy());
        let lounge_canon = canonicalize_repo_path(&lounge.to_string_lossy());
        let conn = store.conn.lock().unwrap();
        let pid: String = conn
            .query_row(
                "SELECT project_id FROM experiences ORDER BY created_at DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(pid, crate::db::PROJECT_ID_UNCLASSIFIED);
        assert_ne!(pid, photo_canon);
        assert_ne!(pid, lounge_canon);
        let _ = std::fs::remove_dir_all(&photo);
        let _ = std::fs::remove_dir_all(&lounge);
    }

    #[tokio::test]
    async fn known_alias_slug_binds_canonical_not_verbatim() {
        use crate::db::canonicalize_repo_path;
        use crate::models::IndexGraph;
        let store = ExperienceStore::memory().expect("db");
        let root = std::env::temp_dir().join(format!("lounge-alias-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        store
            .save_project_index(IndexGraph {
                project: "Agent-Lounge-OS".into(),
                repo_path: root.to_string_lossy().into_owned(),
                status: None,
                node_count: 0,
                edge_count: 0,
                files: Some(0),
                nodes: vec![],
                references: vec![],
                dead: vec![],
            })
            .await
            .expect("index");
        // Extra CBM-style slug alias.
        {
            let conn = store.conn.lock().unwrap();
            let canonical = canonicalize_repo_path(&root.to_string_lossy());
            crate::db::upsert_alias(&conn, "agent-lounge-os", &canonical).unwrap();
        }
        let expected = canonicalize_repo_path(&root.to_string_lossy());
        let mut server = McpServer::new(store, "nats://127.0.0.1:9");
        let args = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "lounge_record_experience",
                "arguments": {
                    "project": "agent-lounge-os",
                    "context": "LMR",
                    "decision": "bind to canonical path"
                }
            }
        })
        .to_string();
        let resp = server.handle_line(&args).await.unwrap().unwrap();
        let val: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(val["result"]["structuredContent"]["project_id"], expected);
        assert_ne!(
            val["result"]["structuredContent"]["project_id"],
            "agent-lounge-os"
        );
        let _ = std::fs::remove_dir_all(&root);
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
                ..Default::default()
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
        assert!(connected.iter().any(|c| c.id == "app:claude_ai"));
    }

    #[tokio::test]
    async fn cross_project_search_ranks_but_does_not_hard_filter() {
        use crate::models::{AstNode, IndexGraph};
        let store = ExperienceStore::memory().expect("db");

        store
            .insert_record(ExperienceRecord {
                id: Uuid::new_v4().to_string(),
                project_id: "project-a".into(),
                agent_id: "cursor".into(),
                topic: "redis config port conflict".into(),
                solution_summary: "6379 çakıştı, 6380 kullanıldı".into(),
                adr_record: "6379 çakıştı, 6380 kullanıldı".into(),
                outcome: ExperienceOutcome::Success,
                related_task_id: None,
                tags: vec!["redis".into()],
                created_at: now_rfc3339(),
                embedding: Vec::new(),
                ..Default::default()
            })
            .await
            .expect("seed a");

        let root_b = std::env::temp_dir().join(format!("lounge-proj-b-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root_b).unwrap();
        let file_b = root_b.join("src/app.rs");
        std::fs::create_dir_all(file_b.parent().unwrap()).unwrap();
        std::fs::write(&file_b, "fn main() {}").unwrap();
        store
            .save_project_index(IndexGraph {
                project: "project-b".into(),
                repo_path: root_b.to_string_lossy().into_owned(),
                status: None,
                node_count: 1,
                edge_count: 0,
                files: Some(1),
                nodes: vec![AstNode {
                    id: "main".into(),
                    name: "main".into(),
                    kind: "fn".into(),
                    file: Some(file_b.to_string_lossy().into_owned()),
                    line: Some(1),
                    ref_count: 0,
                }],
                references: vec![],
                dead: vec![],
            })
            .await
            .expect("index b");

        let mut server = McpServer::new(store, "nats://127.0.0.1:9");
        let args = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "lounge_search_experience",
                "arguments": {
                    "query": "redis config",
                    "active_file": file_b.to_string_lossy()
                }
            }
        })
        .to_string();
        let resp = server
            .handle_line(&args)
            .await
            .expect("handle")
            .expect("resp");
        let val: Value = serde_json::from_str(&resp).unwrap();
        assert_ne!(val["result"]["isError"], true);
        let body = &val["result"]["structuredContent"];
        let expected_b = crate::db::canonicalize_repo_path(&root_b.to_string_lossy());
        assert_eq!(body["project_id"], expected_b);
        let experiences = body["experiences"].as_array().unwrap();
        assert!(
            experiences.iter().any(|row| {
                row["project_id"] == "project-a"
                    && row["adr_summary"].as_str().unwrap_or("").contains("6380")
            }),
            "A projesindeki Redis kararı B'den active_file aramasında dönmeli: {body}"
        );
        assert!(experiences
            .iter()
            .any(|row| { row["project_id"] == "project-a" && row["same_project"] == false }));

        let restricted = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "lounge_search_experience",
                "arguments": {
                    "query": "redis config",
                    "active_file": file_b.to_string_lossy(),
                    "restrict_to_project": true
                }
            }
        })
        .to_string();
        let resp2 = server
            .handle_line(&restricted)
            .await
            .expect("handle")
            .expect("resp");
        let val2: Value = serde_json::from_str(&resp2).unwrap();
        let body2 = &val2["result"]["structuredContent"];
        let experiences2 = body2["experiences"].as_array().unwrap();
        assert!(experiences2
            .iter()
            .all(|row| row["project_id"] == expected_b));

        let _ = std::fs::remove_dir_all(&root_b);
    }

    /// T5: MCP probe — `lounge_health` tools/call returns versioned schema.
    #[tokio::test]
    async fn t5_lounge_health_mcp_tool_call() {
        let store = ExperienceStore::memory().expect("db");
        let server = McpServer::new(store, "nats://127.0.0.1:9");
        let mut client = ClientCtx {
            name: "mcp-probe".into(),
            version: "0.1".into(),
            initialized: true,
            session_id: "sess-health".into(),
            workspace_path: String::new(),
        };
        let resp = server
            .handle_line_for(
                r#"{"jsonrpc":"2.0","id":42,"method":"tools/call","params":{"name":"lounge_health","arguments":{}}}"#,
                &mut client,
            )
            .await
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        assert!(v["error"].is_null(), "unexpected error: {v}");
        let body = &v["result"]["structuredContent"];
        assert_eq!(body["schema_version"], "1.1.0");
        assert!(body["agents"].is_array());
        assert!(body["title"].is_object());
        assert!(body["dead_symbols"].is_object());
        assert!(body["cbm"].is_object());
        assert!(body["platform"].is_object());
        validate_schema(SchemaKind::LoungeHealthReport, body).expect("report schema");

        // tools/list advertises lounge_health
        let listed = server
            .handle_line_for(
                r#"{"jsonrpc":"2.0","id":43,"method":"tools/list","params":{}}"#,
                &mut client,
            )
            .await
            .unwrap()
            .unwrap();
        let list_v: Value = serde_json::from_str(&listed).unwrap();
        let tools = list_v["result"]["tools"].as_array().unwrap();
        assert!(
            tools.iter().any(|t| t["name"] == "lounge_health"),
            "lounge_health missing from tools/list"
        );
    }

    #[tokio::test]
    async fn concurrent_client_contexts_do_not_clobber_source_agent() {
        let store = ExperienceStore::memory().expect("db");
        let server = McpServer::new(store, "nats://127.0.0.1:9");

        let mut cursor = ClientCtx {
            name: "Cursor".into(),
            version: "1".into(),
            initialized: true,
            session_id: "sess-cursor".into(),
            workspace_path: String::new(),
        };
        let mut claude = ClientCtx {
            name: "Claude Desktop".into(),
            version: "2".into(),
            initialized: true,
            session_id: "sess-claude".into(),
            workspace_path: String::new(),
        };

        let status_cursor = server
            .handle_line_for(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"lounge_status","arguments":{}}}"#,
                &mut cursor,
            )
            .await
            .unwrap()
            .unwrap();
        let _ = server
            .handle_line_for(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"Claude Desktop","version":"2"}}}"#,
                &mut claude,
            )
            .await
            .unwrap();
        let status_cursor_again = server
            .handle_line_for(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lounge_status","arguments":{}}}"#,
                &mut cursor,
            )
            .await
            .unwrap()
            .unwrap();

        let v1: Value = serde_json::from_str(&status_cursor).unwrap();
        let v2: Value = serde_json::from_str(&status_cursor_again).unwrap();
        assert_eq!(
            v1["result"]["structuredContent"]["server"]["client"]["name"],
            "Cursor"
        );
        assert_eq!(
            v2["result"]["structuredContent"]["server"]["client"]["name"],
            "Cursor"
        );
        assert_eq!(cursor.name, "Cursor");
        assert_eq!(claude.name, "Claude Desktop");
    }

    #[tokio::test]
    async fn tool_record_marks_experience_reported_event() {
        let store = ExperienceStore::memory().expect("db");
        let mut server = McpServer::new(store.clone(), "nats://127.0.0.1:9");
        let resp = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"lounge_record_experience","arguments":{"project_id":"proj-x","context":"redis","decision":"6380"}}}"#,
            )
            .await
            .unwrap()
            .unwrap();
        let val: Value = serde_json::from_str(&resp).unwrap();
        assert_ne!(val["result"]["isError"], true);
        assert_eq!(
            val["result"]["structuredContent"]["event"],
            EXPERIENCE_REPORTED
        );
        let latest = store.latest(5).await.unwrap();
        assert_eq!(latest.len(), 1);
        assert!(!latest[0].reviewed, "MCP rows land unreviewed");
        assert_eq!(store.count_unreviewed_experiences().await.unwrap(), 1);
        // No structured adr → empty ADR (No ADR), solution kept separately.
        let rec = store
            .get_record(latest[0].id.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(
            rec.adr_record.trim().is_empty(),
            "adr_record must stay empty without adr: {:?}",
            rec.adr_record
        );
        assert_eq!(rec.solution_summary, "6380");
        assert!(rec.source.contains("mcp"));
        assert!(!rec.tags.iter().any(|t| t == "mcp" || t == "external"));
    }

    #[tokio::test]
    async fn tool_record_adr_tags_hygiene_and_agent_title() {
        let store = ExperienceStore::memory().expect("db");
        let mut server = McpServer::new(store.clone(), "nats://127.0.0.1:9");
        let line = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "lounge_record_experience",
                "arguments": {
                    "project_id": "proj-q",
                    "context": "connect 192.168.1.20:5555 from /Users/dev/app",
                    "decision": "use adb over wifi",
                    "tags": ["MCP", "Redis", "redis", "external"],
                    "title": "ADB wifi setup",
                    "adr": {
                        "context": "device debugging",
                        "decision": "wifi adb",
                        "consequences": "no usb cable",
                        "alternatives": "usb only"
                    }
                }
            }
        })
        .to_string();
        let resp = server.handle_line(&line).await.unwrap().unwrap();
        let val: Value = serde_json::from_str(&resp).unwrap();
        assert_ne!(val["result"]["isError"], true);
        let body = &val["result"]["structuredContent"];
        assert_eq!(body["hygiene_masked"], true);
        assert_eq!(body["title_source"], "agent");
        assert_eq!(body["title"], "ADB wifi setup");

        let latest = store.latest(1).await.unwrap();
        let rec = store
            .get_record(latest[0].id.clone())
            .await
            .unwrap()
            .unwrap();
        assert!(rec.adr_record.contains("Decision: wifi adb"));
        assert!(!rec.adr_record.contains("192.168"));
        assert!(
            rec.topic.contains("<device-ip>:5555"),
            "topic should mask IP: {}",
            rec.topic
        );
        assert!(
            rec.topic.contains("<home>"),
            "topic should mask home: {}",
            rec.topic
        );
        assert_eq!(rec.tags, vec!["redis"]);
        assert!(rec.source.contains("mcp"));
        assert!(rec.source.contains("external"));
        assert_eq!(rec.title_source, crate::models::TitleSource::Agent);
        assert_eq!(rec.title, "ADB wifi setup");
        assert!(!rec.reviewed);

        store
            .set_experience_title(
                rec.id.clone(),
                "LLM should not win".into(),
                crate::models::TitleSource::Llm,
                false,
            )
            .await
            .unwrap();
        let again = store.get_record(rec.id.clone()).await.unwrap().unwrap();
        assert_eq!(again.title, "ADB wifi setup");
        assert_eq!(again.title_source, crate::models::TitleSource::Agent);
    }

    #[test]
    fn proxy_timeout_default_covers_claude_code_threshold() {
        // B4: varsayılan ≥ claude-code 300 + marj; eski 290 asılı bırakıyordu.
        let prev = std::env::var("LOUNGE_MCP_PROXY_TIMEOUT_SECS").ok();
        std::env::remove_var("LOUNGE_MCP_PROXY_TIMEOUT_SECS");
        assert!(
            resolve_proxy_timeout_secs() >= 320,
            "default={}",
            resolve_proxy_timeout_secs()
        );
        std::env::set_var("LOUNGE_MCP_PROXY_TIMEOUT_SECS", "400");
        assert_eq!(resolve_proxy_timeout_secs(), 400);
        std::env::set_var("LOUNGE_MCP_PROXY_TIMEOUT_SECS", "9999");
        assert_eq!(resolve_proxy_timeout_secs(), 900);
        match prev {
            Some(v) => std::env::set_var("LOUNGE_MCP_PROXY_TIMEOUT_SECS", v),
            None => std::env::remove_var("LOUNGE_MCP_PROXY_TIMEOUT_SECS"),
        }
    }

    #[tokio::test]
    async fn resources_expose_a2a_guide() {
        let store = ExperienceStore::memory().expect("db");
        let mut server = McpServer::new(store, "nats://127.0.0.1:9");
        let init = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"claude-ai","version":"1"}}}"#,
            )
            .await
            .unwrap()
            .unwrap();
        let init_v: Value = serde_json::from_str(&init).unwrap();
        assert!(init_v["result"]["capabilities"]["resources"].is_object());
        assert!(init_v["result"]["instructions"]
            .as_str()
            .unwrap_or("")
            .contains(A2A_GUIDE_URI));

        let list = server
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"resources/list"}"#)
            .await
            .unwrap()
            .unwrap();
        let list_v: Value = serde_json::from_str(&list).unwrap();
        let uris: Vec<&str> = list_v["result"]["resources"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["uri"].as_str())
            .collect();
        assert!(uris.contains(&A2A_GUIDE_URI));

        let read = server
            .handle_line(&format!(
                r#"{{"jsonrpc":"2.0","id":3,"method":"resources/read","params":{{"uri":"{A2A_GUIDE_URI}"}}}}"#
            ))
            .await
            .unwrap()
            .unwrap();
        let read_v: Value = serde_json::from_str(&read).unwrap();
        let text = read_v["result"]["contents"][0]["text"].as_str().unwrap();
        assert!(text.contains("poll_after_secs"));
        assert!(text.contains("pending_results"));
        assert!(text.contains("Claude Desktop"));
    }

    #[tokio::test]
    async fn tools_list_injects_client_aware_threshold() {
        let store = ExperienceStore::memory().expect("db");
        let server = McpServer::new(store, "nats://127.0.0.1:9");
        let mut antigravity = ClientCtx {
            name: "antigravity-client".into(),
            version: "1".into(),
            initialized: true,
            session_id: "sess-ag".into(),
            workspace_path: String::new(),
        };
        let mut grok = ClientCtx {
            name: "Grok Bot".into(),
            version: "1".into(),
            initialized: true,
            session_id: "sess-grok".into(),
            workspace_path: String::new(),
        };
        let ag = server
            .handle_line_for(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
                &mut antigravity,
            )
            .await
            .unwrap()
            .unwrap();
        let grok_resp = server
            .handle_line_for(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
                &mut grok,
            )
            .await
            .unwrap()
            .unwrap();
        let ag_v: Value = serde_json::from_str(&ag).unwrap();
        let grok_v: Value = serde_json::from_str(&grok_resp).unwrap();
        let find_call = |v: &Value| {
            v["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["name"] == "lounge_call_agent")
                .unwrap()["description"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let ag_desc = find_call(&ag_v);
        let grok_desc = find_call(&grok_v);
        assert!(ag_desc.contains("150"), "{ag_desc}");
        assert!(grok_desc.contains("45"), "{grok_desc}");
        assert!(ag_v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "lounge_list_my_tasks"));
    }

    #[tokio::test]
    async fn piggyback_pending_results_filters_by_mcp_session_id() {
        use crate::models::{LoungeTask, TaskStatus};
        let store = ExperienceStore::memory().expect("db");
        let server = McpServer::new(store.clone(), "nats://127.0.0.1:9").with_skip_nats(true);

        let mut mine = ClientCtx {
            name: "claude-ai".into(),
            version: "1".into(),
            initialized: true,
            session_id: "sess-mine".into(),
            workspace_path: String::new(),
        };
        let mut other = ClientCtx {
            name: "claude-ai".into(),
            version: "1".into(),
            initialized: true,
            session_id: "sess-other".into(),
            workspace_path: String::new(),
        };

        let mut task_mine = LoungeTask::new("mcp:claude_ai", "p", "mine-ready");
        task_mine.session_id = Some("sess-mine".into());
        store.admit_a2a_task(&mut task_mine, 10).unwrap();
        store.mark_a2a_wait_timeout(&task_mine.id).unwrap();
        store
            .complete_a2a_with_result(
                &task_mine.id,
                TaskStatus::Completed,
                Some(r#"{"ok":true,"msg":"done"}"#),
                false,
            )
            .unwrap();

        let mut task_other = LoungeTask::new("mcp:claude_ai", "p", "other-ready");
        task_other.session_id = Some("sess-other".into());
        store.admit_a2a_task(&mut task_other, 10).unwrap();
        store.mark_a2a_wait_timeout(&task_other.id).unwrap();
        store
            .complete_a2a_with_result(
                &task_other.id,
                TaskStatus::Completed,
                Some(r#"{"ok":true}"#),
                false,
            )
            .unwrap();

        let status = server
            .handle_line_for(
                r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"lounge_status","arguments":{}}}"#,
                &mut mine,
            )
            .await
            .unwrap()
            .unwrap();
        let status_v: Value = serde_json::from_str(&status).unwrap();
        let pending = status_v["result"]["_meta"]["pending_results"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            pending
                .iter()
                .any(|p| p["id"] == task_mine.id && p["status"] == "ready"),
            "own ready result must piggyback: {status_v}"
        );
        assert!(
            !pending.iter().any(|p| p["id"] == task_other.id),
            "other session must not leak: {status_v}"
        );

        let other_status = server
            .handle_line_for(
                r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"lounge_status","arguments":{}}}"#,
                &mut other,
            )
            .await
            .unwrap()
            .unwrap();
        let other_v: Value = serde_json::from_str(&other_status).unwrap();
        let other_pending = other_v["result"]["_meta"]["pending_results"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(other_pending.iter().any(|p| p["id"] == task_other.id));
        assert!(!other_pending.iter().any(|p| p["id"] == task_mine.id));
    }

    #[tokio::test]
    async fn list_my_tasks_session_ownership_and_poll_hints() {
        use crate::models::LoungeTask;
        let store = ExperienceStore::memory().expect("db");
        let server = McpServer::new(store.clone(), "nats://127.0.0.1:9").with_skip_nats(true);
        let mut client = ClientCtx {
            name: "claude-ai".into(),
            version: "1".into(),
            initialized: true,
            session_id: "sess-list".into(),
            workspace_path: "/tmp/ws".into(),
        };
        let mut task = LoungeTask::new("mcp:claude_ai", "p", "bg-work");
        task.session_id = Some("sess-list".into());
        store.admit_a2a_task(&mut task, 10).unwrap();
        store.mark_a2a_wait_timeout(&task.id).unwrap();

        let mut foreign = LoungeTask::new("mcp:claude_ai", "p", "foreign");
        foreign.session_id = Some("sess-foreign".into());
        store.admit_a2a_task(&mut foreign, 10).unwrap();

        let resp = server
            .handle_line_for(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"lounge_list_my_tasks","arguments":{}}}"#,
                &mut client,
            )
            .await
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_str(&resp).unwrap();
        let tasks = v["result"]["structuredContent"]["tasks"]
            .as_array()
            .unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["id"], task.id);
        assert!(tasks[0]["poll_after_secs"].as_u64().unwrap() >= 15);
        assert_eq!(
            tasks[0]["next_action"].as_str().unwrap(),
            crate::db::NEXT_ACTION_WAIT_TASK
        );
    }

    #[tokio::test]
    async fn halted_refuses_dispatch_while_read_tools_work() {
        let store = ExperienceStore::memory().expect("db");
        crate::services::halt::halt(
            &store,
            Some("mcp-test".into()),
            crate::services::halt::HALT_BY_UI,
        )
        .await
        .unwrap();
        let mut server = McpServer::new(store, "nats://127.0.0.1:9").with_skip_nats(true);

        let dispatch = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"lounge_dispatch_task","arguments":{"target_agent":"cursor","task":"hi","project_id":"p"}}}"#,
            )
            .await
            .unwrap()
            .unwrap();
        let dval: Value = serde_json::from_str(&dispatch).unwrap();
        assert_eq!(dval["result"]["isError"], true);
        let body = &dval["result"]["structuredContent"];
        assert_eq!(body["code"], crate::services::halt::HALTED_ERROR_CODE);
        assert_eq!(body["retryable"], true);
        let msg = body["message"].as_str().unwrap();
        assert!(
            msg.contains("temporary pause") || msg.contains("temporarily halted"),
            "{msg}"
        );
        assert!(msg.contains("does not stop external") || msg.contains("does not cancel"));

        let status = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"lounge_status","arguments":{}}}"#,
            )
            .await
            .unwrap()
            .unwrap();
        let sval: Value = serde_json::from_str(&status).unwrap();
        assert_ne!(sval["result"]["isError"], true, "{sval}");

        let search = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"lounge_search_experience","arguments":{"query":"redis"}}}"#,
            )
            .await
            .unwrap()
            .unwrap();
        let search_val: Value = serde_json::from_str(&search).unwrap();
        assert_ne!(search_val["result"]["isError"], true, "{search_val}");

        let health = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"lounge_health","arguments":{}}}"#,
            )
            .await
            .unwrap()
            .unwrap();
        let hval: Value = serde_json::from_str(&health).unwrap();
        assert_ne!(hval["result"]["isError"], true, "{hval}");
    }

    #[tokio::test]
    async fn search_adds_reviewed_field_and_ranks_approved_first() {
        use crate::models::{ExperienceOutcome, ExperienceRecord, LoungeTask};

        let store = ExperienceStore::memory().expect("db");
        let mut approved = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj-rev", "vault-review-keyword-a"),
            "vault-review-keyword-a solution",
            "vault-review-keyword-a adr",
            ExperienceOutcome::Success,
            vec![],
        );
        approved.reviewed = true;
        approved.created_at = "2026-01-01T00:00:00+00:00".into();
        store.insert_record(approved).await.unwrap();

        let mut pending = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj-rev", "vault-review-keyword-b"),
            "vault-review-keyword-b solution",
            "vault-review-keyword-b adr",
            ExperienceOutcome::Success,
            vec![],
        );
        pending.reviewed = false;
        pending.created_at = "2026-06-01T00:00:00+00:00".into();
        store.insert_record(pending).await.unwrap();

        let mut server = McpServer::new(store, "nats://127.0.0.1:9").with_skip_nats(true);
        let resp = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"lounge_search_experience","arguments":{"query":"vault-review-keyword","limit":10}}}"#,
            )
            .await
            .unwrap()
            .unwrap();
        let val: Value = serde_json::from_str(&resp).unwrap();
        assert_ne!(val["result"]["isError"], true, "{val}");
        let experiences = val["result"]["structuredContent"]["experiences"]
            .as_array()
            .expect("experiences array");
        assert!(experiences.len() >= 2);
        // Additive field — every hit exposes reviewed (backward compatible).
        for row in experiences {
            assert!(
                row.get("reviewed").is_some(),
                "reviewed must be present: {row}"
            );
            assert!(row.get("id").is_some());
            assert!(row.get("project_id").is_some());
        }
        let reviewed_flags: Vec<bool> = experiences
            .iter()
            .map(|row| row["reviewed"].as_bool().unwrap())
            .collect();
        assert!(reviewed_flags.contains(&true));
        assert!(
            reviewed_flags.contains(&false),
            "unreviewed still searchable"
        );
        let first_approved = reviewed_flags.iter().position(|r| *r).unwrap();
        let first_pending = reviewed_flags.iter().position(|r| !*r).unwrap();
        assert!(
            first_approved < first_pending,
            "approved must rank above unreviewed in the same band"
        );
    }

    #[tokio::test]
    async fn search_relevance_outranks_reviewed_outside_band() {
        use crate::models::{ExperienceOutcome, ExperienceRecord, LoungeTask};

        let store = ExperienceStore::memory().expect("db");
        let needle = "vault-review-strong-needle-xyz";
        let mut strong_pending = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj-rev", needle),
            needle,
            format!("{needle} adr"),
            ExperienceOutcome::Success,
            vec![],
        );
        strong_pending.reviewed = false;
        store.insert_record(strong_pending).await.unwrap();

        let mut weak_approved = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj-rev", "other-topic"),
            "unrelated solution text",
            "unrelated adr",
            ExperienceOutcome::Success,
            vec![needle.into()],
        );
        weak_approved.reviewed = true;
        store.insert_record(weak_approved).await.unwrap();

        let mut server = McpServer::new(store, "nats://127.0.0.1:9").with_skip_nats(true);
        let resp = server
            .handle_line(&format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"lounge_search_experience","arguments":{{"query":"{needle}","limit":10}}}}}}"#
            ))
            .await
            .unwrap()
            .unwrap();
        let val: Value = serde_json::from_str(&resp).unwrap();
        assert_ne!(val["result"]["isError"], true, "{val}");
        let experiences = val["result"]["structuredContent"]["experiences"]
            .as_array()
            .expect("experiences array");
        assert!(experiences.len() >= 2);
        assert_eq!(
            experiences[0]["reviewed"].as_bool(),
            Some(false),
            "strong unreviewed must lead outside REVIEW_SCORE_BAND: {experiences:?}"
        );
        assert!(experiences.iter().any(|r| r["reviewed"] == true));
    }

    /// Vault-1 B2: MCP order must use the carried hybrid score (FTS BM25 + stored
    /// embedding), not a re-sort via LIKE lexical / on-the-fly embeddings.
    #[tokio::test]
    async fn search_mcp_hybrid_score_orders_strong_bm25_first() {
        use crate::models::{ExperienceOutcome, ExperienceRecord, LoungeTask};

        let store = ExperienceStore::memory().expect("db");
        let mut strong = ExperienceRecord::from_task(
            &LoungeTask::new("cursor", "proj-a", "hybrid-mcp-needle hybrid-mcp-needle"),
            "hybrid-mcp-needle hybrid-mcp-needle solution",
            "hybrid-mcp-needle adr",
            ExperienceOutcome::Success,
            vec![],
        );
        strong.id = "mcp-strong".into();
        strong.reviewed = false;
        strong.created_at = "2026-01-01T00:00:00+00:00".into();
        store.insert_record(strong).await.unwrap();

        let mut weak = ExperienceRecord::from_task(
            &LoungeTask::new(
                "cursor",
                "proj-b",
                "lorem ipsum dolor sit amet consectetur adipiscing",
            ),
            "unrelated padding solution body with filler words only",
            "lorem padding hybrid-mcp-needle once",
            ExperienceOutcome::Success,
            vec![],
        );
        weak.id = "mcp-weak".into();
        weak.reviewed = true;
        weak.created_at = "2026-06-01T00:00:00+00:00".into();
        store.insert_record(weak).await.unwrap();

        let mut server = McpServer::new(store, "nats://127.0.0.1:9").with_skip_nats(true);
        let resp = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"lounge_search_experience","arguments":{"query":"hybrid-mcp-needle","limit":10}}}"#,
            )
            .await
            .unwrap()
            .unwrap();
        let val: Value = serde_json::from_str(&resp).unwrap();
        assert_ne!(val["result"]["isError"], true, "{val}");
        let experiences = val["result"]["structuredContent"]["experiences"]
            .as_array()
            .expect("experiences array");
        assert!(
            experiences.len() >= 2,
            "expected both hits: {experiences:?}"
        );
        assert_eq!(
            experiences[0]["id"].as_str(),
            Some("mcp-strong"),
            "strong BM25 hybrid must lead at MCP tool level: {experiences:?}"
        );
        assert_eq!(experiences[1]["id"].as_str(), Some("mcp-weak"));
    }
}
