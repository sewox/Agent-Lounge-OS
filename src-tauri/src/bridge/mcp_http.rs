//! Kernel içi MCP HTTP yüzeyi (JSON-RPC + isteğe bağlı SSE nabız).
//!
//! Varsayılan: `http://127.0.0.1:18791`
//! - `POST /mcp` — JSON-RPC istek/yanıt (`lounge-mcp` stdio shim buraya proxy eder)
//! - `GET /mcp/health` — sağlık
//! - `GET /mcp/sse` — tek seferlik ready olayı (dashboard / istemci nabız)
//!
//! İstemci kimliği istek başına: `Mcp-Session-Id` oturum eşlemesi (+ isteğe bağlı
//! `X-Lounge-Client-Name`). Tam Streamable HTTP bu PR kapsamı dışında.
//!
//! ## Kilit politikası
//! `McpServer` klonlanır; `handle_line_for` sunucu kilidi **tutulmadan** çalışır.
//! Böylece uzun `lounge_call_agent` beklerken `yield_result`, `cancelled`, `/health`
//! ve diğer oturumlar ilerleyebilir.
//!
//! ## Sonuç teslimi (deadline exceeded sonrası)
//! Antigravity ölçümü: istemci `notifications/cancelled` (`deadline exceeded`) gönderir
//! ama **bağlantıyı kapatmaz**. Bridge görevi `WAIT_TIMEOUT_REACHED` / backgrounded
//! bırakır; aynı `Mcp-Session-Id` ile sonraki `lounge_wait_task` sonucu çeker.

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

use super::mcp_server::{default_mcp_http_bind, ClientCtx, McpServer};
use super::session_id::{
    is_valid_session_id, normalize_or_mint_session_id, MAX_MCP_SESSIONS,
};
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

/// Test / gömülü: mevcut sunucu örneğiyle router kur (aynı in_flight / store).
pub fn router_from_server(server: McpServer) -> Router {
    let hub = Hub {
        server: Arc::new(Mutex::new(server)),
        sessions: Arc::new(Mutex::new(HashMap::new())),
    };
    Router::new()
        .route("/mcp", post(mcp_post))
        .route("/mcp/health", get(health))
        .route("/mcp/sse", get(sse_ready))
        .route("/health", get(health))
        .with_state(hub)
}

async fn health(State(hub): State<Hub>) -> impl IntoResponse {
    let sessions = hub.sessions.lock().await.len();
    let timeout_snapshot = {
        let guard = hub.server.lock().await;
        guard.timeouts().snapshot()
    };
    Json(serde_json::json!({
        "ok": true,
        "server": "agent-lounge-os",
        "transport": "http",
        "sessions": sessions,
        "timeout_manager": timeout_snapshot,
    }))
}

async fn mcp_post(State(hub): State<Hub>, headers: HeaderMap, body: String) -> impl IntoResponse {
    let raw_session = headers
        .get(HDR_SESSION)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let client_provided = raw_session.map(|s| is_valid_session_id(s)).unwrap_or(false);
    let session_id = normalize_or_mint_session_id(raw_session);

    let mut client = {
        let mut sessions = hub.sessions.lock().await;
        // Üst sınır — en eski oturumları düşür (FIFO yaklaşık: HashMap sırası).
        while sessions.len() >= MAX_MCP_SESSIONS && !sessions.contains_key(&session_id) {
            if let Some(evict) = sessions.keys().next().cloned() {
                sessions.remove(&evict);
            } else {
                break;
            }
        }
        sessions
            .entry(session_id.clone())
            .or_insert_with(|| ClientCtx {
                session_id: session_id.clone(),
                ..ClientCtx::default()
            })
            .clone()
    };
    client.session_id = session_id.clone();

    if let Some(name) = header_str(&headers, HDR_CLIENT_NAME) {
        client.name = name;
    }
    if let Some(version) = header_str(&headers, HDR_CLIENT_VERSION) {
        client.version = version;
    }

    // P0-1: sunucuyu kilitleyip klonla, kilidi bırak, uzun işlemi kilit dışında çalıştır.
    let server = {
        let guard = hub.server.lock().await;
        guard.clone()
    };
    let handled = server.handle_line_for(body.trim(), &mut client).await;

    {
        let mut sessions = hub.sessions.lock().await;
        sessions.insert(session_id.clone(), client);
    }

    let session_header = (
        HeaderName::from_static(HDR_SESSION),
        HeaderValue::from_str(&session_id).unwrap_or_else(|_| HeaderValue::from_static("unknown")),
    );

    // Geçersiz istemci id'si mint edildiyse yine de yeni id döner (istemci günceller).
    let _ = client_provided;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::wait_clock::{ManualWaitClock, WaitClock};
    use crate::models::AgentSession;
    use std::sync::Arc;
    use std::time::Duration;

    async fn start_test_server(server: McpServer) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router_from_server(server);
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        // Kısa bind oturması.
        tokio::time::sleep(Duration::from_millis(20)).await;
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn concurrent_call_agent_allows_yield_health_and_cancel() {
        let store = ExperienceStore::memory().unwrap();
        let clock = Arc::new(ManualWaitClock::new());
        let server = McpServer::new(store.clone(), "nats://127.0.0.1:9")
            .with_skip_nats(true)
            .with_orchestrator_clock(clock.clone() as Arc<dyn WaitClock>);
        server.timeouts().set_global_override_secs(Some(30));
        let base = start_test_server(server).await;
        let http = reqwest::Client::new();

        let caller = "caller-sess-http-1";
        let call_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "lounge_call_agent",
                "arguments": {
                    "target_agent": "worker",
                    "task": "slow-job",
                    "project_id": "p",
                    "wait": true
                }
            }
        });

        let base_call = base.clone();
        let http_call = http.clone();
        let call_fut = tokio::spawn(async move {
            http_call
                .post(format!("{base_call}/mcp"))
                .header("content-type", "application/json")
                .header("mcp-session-id", caller)
                .header("x-lounge-client-name", "cursor")
                .json(&call_body)
                .send()
                .await
                .unwrap()
        });

        let mut task_id = None;
        for _ in 0..200 {
            clock.advance(Duration::from_millis(10));
            tokio::time::sleep(Duration::from_millis(5)).await;
            let id: Option<String> = {
                let conn = store.conn.lock().unwrap();
                conn.query_row("SELECT id FROM a2a_tasks LIMIT 1", [], |r| r.get(0))
                    .ok()
            };
            if id.is_some() {
                task_id = id;
                break;
            }
        }
        let task_id = task_id.expect("call_agent admit etmeli");

        let mut sess = AgentSession::new("p", "worker", "worker", "/tmp", "test");
        sess.id = "worker-http-1".into();
        store.upsert_session(&sess).unwrap();

        // call_agent sürerken health yanıt vermeli (kilit tutulmuyor).
        let health = http
            .get(format!("{base}/health"))
            .send()
            .await
            .unwrap();
        assert!(health.status().is_success());
        assert!(health.json::<Value>().await.unwrap()["ok"].as_bool().unwrap());

        // Eşzamanlı yield
        let yield_res = http
            .post(format!("{base}/mcp"))
            .header("content-type", "application/json")
            .header("mcp-session-id", "worker-http-1")
            .header("x-lounge-client-name", "worker")
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "lounge_yield_result",
                    "arguments": {
                        "task_id": task_id,
                        "status": "completed",
                        "output": {"ok": true}
                    }
                }
            }))
            .send()
            .await
            .unwrap();
        assert!(yield_res.status().is_success());
        let yield_json: Value = yield_res.json().await.unwrap();
        assert_ne!(yield_json["result"]["isError"], true, "{yield_json}");

        // cancelled bildirimi in_flight'ta kayıt bulabilmeli (kilit dışı).
        let cancel_res = http
            .post(format!("{base}/mcp"))
            .header("content-type", "application/json")
            .header("mcp-session-id", caller)
            .header("x-lounge-client-name", "cursor")
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/cancelled",
                "params": {"requestId": 1, "reason": "deadline exceeded"}
            }))
            .send()
            .await
            .unwrap();
        assert!(
            cancel_res.status() == StatusCode::NO_CONTENT || cancel_res.status().is_success()
        );

        clock.advance(Duration::from_millis(100));
        let call_res = call_fut.await.unwrap();
        assert!(call_res.status().is_success());
        let call_json: Value = call_res.json().await.unwrap();
        let text = call_json["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("");
        assert!(
            text.contains("completed") || text.contains("backgrounded"),
            "concurrent call_agent outcome: {call_json}"
        );
    }

    #[tokio::test]
    async fn invalid_session_id_is_minted() {
        let store = ExperienceStore::memory().unwrap();
        let server = McpServer::new(store, "nats://127.0.0.1:9").with_skip_nats(true);
        let base = start_test_server(server).await;
        let res = reqwest::Client::new()
            .post(format!("{base}/mcp"))
            .header("content-type", "application/json")
            .header("mcp-session-id", "../evil")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
            .send()
            .await
            .unwrap();
        let sid = res
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(is_valid_session_id(sid));
        assert_ne!(sid, "../evil");
    }
}
