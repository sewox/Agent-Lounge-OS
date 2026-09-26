//! Kernel içi MCP HTTP yüzeyi (JSON-RPC + isteğe bağlı SSE nabız).
//!
//! Varsayılan: `http://127.0.0.1:18791`
//! - `POST /mcp` — JSON-RPC istek/yanıt (`lounge-mcp` stdio shim buraya proxy eder)
//! - `GET /mcp/health` — sağlık
//! - `GET /mcp/sse` — tek seferlik ready olayı (dashboard / istemci nabız)
//!
//! İstemci kimliği istek başına: `Mcp-Session-Id` oturum eşlemesi (+ isteğe bağlı
//! `X-Lounge-Client-Name`). Tam Streamable HTTP bu PR kapsamı dışında.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;
use tokio::sync::Mutex;
use uuid::Uuid;

use super::mcp_server::{default_mcp_http_bind, ClientCtx, McpServer};
use crate::db::ExperienceStore;
use crate::kernel::WorkerRegistry;

const HDR_SESSION: &str = "mcp-session-id";
const HDR_CLIENT_NAME: &str = "x-lounge-client-name";
const HDR_CLIENT_VERSION: &str = "x-lounge-client-version";

#[derive(Clone)]
struct Hub {
    server: Arc<Mutex<McpServer>>,
    sessions: Arc<Mutex<HashMap<String, ClientCtx>>>,
}

/// Tauri setup'tan spawn edilir.
pub async fn serve(
    store: ExperienceStore,
    nats_url: impl Into<String>,
    workers: Option<WorkerRegistry>,
) -> Result<()> {
    let bind = default_mcp_http_bind();
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("MCP HTTP bind başarısız: {bind}"))?;
    log::info!("MCP HTTP dinliyor: http://{bind}");

    let mut server = McpServer::new(store, nats_url);
    if let Some(registry) = workers {
        server = server.with_workers(registry);
    }
    let hub = Hub {
        server: Arc::new(Mutex::new(server)),
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };
    let app = Router::new()
        .route("/mcp", post(mcp_post))
        .route("/mcp/health", get(health))
        .route("/mcp/sse", get(sse_ready))
        .route("/health", get(health))
        .with_state(hub);

    axum::serve(listener, app).await.context("MCP HTTP serve")?;
    Ok(())
}

async fn health(State(hub): State<Hub>) -> impl IntoResponse {
    let sessions = hub.sessions.lock().await.len();
    Json(serde_json::json!({
        "ok": true,
        "server": "agent-lounge-os",
        "transport": "http",
        "sessions": sessions,
    }))
}

async fn mcp_post(State(hub): State<Hub>, headers: HeaderMap, body: String) -> impl IntoResponse {
    let session_id = headers
        .get(HDR_SESSION)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut client = {
        let mut sessions = hub.sessions.lock().await;
        sessions
            .entry(session_id.clone())
            .or_insert_with(ClientCtx::default)
            .clone()
    };

    if let Some(name) = header_str(&headers, HDR_CLIENT_NAME) {
        client.name = name;
    }
    if let Some(version) = header_str(&headers, HDR_CLIENT_VERSION) {
        client.version = version;
    }

    let mut guard = hub.server.lock().await;
    let handled = guard.handle_line_for(body.trim(), &mut client).await;
    drop(guard);

    {
        let mut sessions = hub.sessions.lock().await;
        sessions.insert(session_id.clone(), client);
    }

    let session_header = (
        HeaderName::from_static(HDR_SESSION),
        HeaderValue::from_str(&session_id).unwrap_or_else(|_| HeaderValue::from_static("unknown")),
    );

    match handled {
        Ok(Some(response)) => {
            let value: Value = serde_json::from_str(&response).unwrap_or_else(|_| {
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32603, "message": "yanıt parse edilemedi" }
                })
            });
            (StatusCode::OK, [session_header], Json(value)).into_response()
        }
        Ok(None) => (StatusCode::NO_CONTENT, [session_header]).into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            [session_header],
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": err.to_string() }
            })),
        )
            .into_response(),
    }
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

async fn sse_ready(State(hub): State<Hub>) -> impl IntoResponse {
    let sessions = hub.sessions.lock().await.len();
    let body =
        format!("event: lounge.mcp.ready\ndata: {{\"ok\":true,\"sessions\":{sessions}}}\n\n");
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
        body,
    )
}
