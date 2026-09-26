//! Kernel içi MCP HTTP yüzeyi (JSON-RPC + isteğe bağlı SSE nabız).
//!
//! Varsayılan: `http://127.0.0.1:18791`
//! - `POST /mcp` — JSON-RPC istek/yanıt (`lounge-mcp` stdio shim buraya proxy eder)
//! - `GET /mcp/health` — sağlık
//! - `GET /mcp/sse` — tek seferlik ready olayı (dashboard / istemci nabız)

use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::Value;
use tokio::sync::Mutex;

use super::mcp_server::{default_mcp_http_bind, McpServer};
use crate::db::ExperienceStore;

#[derive(Clone)]
struct Hub {
    server: Arc<Mutex<McpServer>>,
}

/// Tauri setup'tan spawn edilir.
pub async fn serve(store: ExperienceStore, nats_url: impl Into<String>) -> Result<()> {
    let bind = default_mcp_http_bind();
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("MCP HTTP bind başarısız: {bind}"))?;
    log::info!("MCP HTTP dinliyor: http://{bind}");

    let hub = Hub {
        server: Arc::new(Mutex::new(McpServer::new(store, nats_url))),
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
    let guard = hub.server.lock().await;
    Json(serde_json::json!({
        "ok": true,
        "server": "agent-lounge-os",
        "transport": "http",
        "client": guard.client_label(),
    }))
}

async fn mcp_post(State(hub): State<Hub>, body: String) -> impl IntoResponse {
    let mut guard = hub.server.lock().await;
    match guard.handle_line(body.trim()).await {
        Ok(Some(response)) => {
            let value: Value = serde_json::from_str(&response).unwrap_or_else(|_| {
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": { "code": -32603, "message": "yanıt parse edilemedi" }
                })
            });
            (StatusCode::OK, Json(value)).into_response()
        }
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": err.to_string() }
            })),
        )
            .into_response(),
    }
}

async fn sse_ready(State(hub): State<Hub>) -> impl IntoResponse {
    let label = {
        let guard = hub.server.lock().await;
        guard.client_label()
    };
    let body = format!("event: lounge.mcp.ready\ndata: {{\"ok\":true,\"client\":{label}}}\n\n");
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
        body,
    )
}
