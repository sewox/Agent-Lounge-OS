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

use std::collections::{HashMap, VecDeque};
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
use super::session_id::{normalize_or_mint_session_id, MAX_MCP_SESSIONS};
use crate::db::ExperienceStore;
use crate::kernel::WorkerRegistry;
use crate::services::lounge_auth::{authorize_mcp_headers, HDR_LOUNGE_TOKEN};

const HDR_SESSION: &str = "mcp-session-id";
const HDR_CLIENT_NAME: &str = "x-lounge-client-name";
const HDR_CLIENT_VERSION: &str = "x-lounge-client-version";

#[derive(Clone)]
struct Hub {
    server: Arc<Mutex<McpServer>>,
    sessions: Arc<Mutex<HashMap<String, ClientCtx>>>,
    /// LRU sıra — dokunulan oturum sona; eviction front'tan (canlı oturum düşmesin).
    session_order: Arc<Mutex<VecDeque<String>>>,
}

fn touch_session_order(order: &mut VecDeque<String>, session_id: &str) {
    if let Some(pos) = order.iter().position(|s| s == session_id) {
        order.remove(pos);
    }
    order.push_back(session_id.to_string());
}

fn evict_lru_sessions(
    sessions: &mut HashMap<String, ClientCtx>,
    order: &mut VecDeque<String>,
    keep: &str,
) {
    while sessions.len() >= MAX_MCP_SESSIONS && !sessions.contains_key(keep) {
        let Some(evict) = order.pop_front() else {
            break;
        };
        if evict == keep {
            order.push_back(evict);
            break;
        }
        sessions.remove(&evict);
    }
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
        session_order: Arc::new(Mutex::new(VecDeque::new())),
    };
    let app = Router::new()
        .route("/mcp", post(mcp_post).delete(mcp_delete))
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
        session_order: Arc::new(Mutex::new(VecDeque::new())),
    };
    Router::new()
        .route("/mcp", post(mcp_post).delete(mcp_delete))
        .route("/mcp/health", get(health))
        .route("/mcp/sse", get(sse_ready))
        .route("/health", get(health))
        .with_state(hub)
}

fn authorize_headers(headers: &HeaderMap) -> Result<(), (StatusCode, Json<Value>)> {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    let origin = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let token = headers
        .get(HDR_LOUNGE_TOKEN)
        .and_then(|v| v.to_str().ok());
    authorize_mcp_headers(host, origin, token).map_err(|err| {
        (
            StatusCode::from_u16(err.status()).unwrap_or(StatusCode::FORBIDDEN),
            Json(serde_json::json!({
                "error": err.message(),
                "code": "remote_access_denied"
            })),
        )
    })
}

async fn health(State(hub): State<Hub>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(err) = authorize_headers(&headers) {
        return err.into_response();
    }
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
    .into_response()
}

async fn mcp_post(State(hub): State<Hub>, headers: HeaderMap, body: String) -> impl IntoResponse {
    if let Err(err) = authorize_headers(&headers) {
        return err.into_response();
    }
    let raw_session = headers
        .get(HDR_SESSION)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let session_id = normalize_or_mint_session_id(raw_session);

    let mut client = {
        let mut sessions = hub.sessions.lock().await;
        let mut order = hub.session_order.lock().await;
        evict_lru_sessions(&mut sessions, &mut order, &session_id);
        let entry = sessions
            .entry(session_id.clone())
            .or_insert_with(|| ClientCtx {
                session_id: session_id.clone(),
                ..ClientCtx::default()
            })
            .clone();
        touch_session_order(&mut order, &session_id);
        entry
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
        let mut order = hub.session_order.lock().await;
        sessions.insert(session_id.clone(), client);
        touch_session_order(&mut order, &session_id);
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

async fn mcp_delete(State(hub): State<Hub>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(err) = authorize_headers(&headers) {
        return err.into_response();
    }
    let raw_session = headers
        .get(HDR_SESSION)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let Some(session_id) = raw_session.map(str::to_string) else {
        return StatusCode::BAD_REQUEST.into_response();
    };

    {
        let mut sessions = hub.sessions.lock().await;
        let mut order = hub.session_order.lock().await;
        sessions.remove(&session_id);
        if let Some(pos) = order.iter().position(|s| s == &session_id) {
            order.remove(pos);
        }
    }

    let cancelled = {
        let server = {
            let guard = hub.server.lock().await;
            guard.clone()
        };
        server.on_session_disconnect(&session_id).await
    };

    log::info!("MCP DELETE session={session_id} in_flight_cancelled={cancelled}");
    (
        StatusCode::NO_CONTENT,
        [(
            HeaderName::from_static(HDR_SESSION),
            HeaderValue::from_str(&session_id).unwrap_or_else(|_| HeaderValue::from_static("")),
        )],
    )
        .into_response()
}

fn header_str(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

async fn sse_ready(State(hub): State<Hub>, headers: HeaderMap) -> impl IntoResponse {
    if let Err(err) = authorize_headers(&headers) {
        return err.into_response();
    }
    let sessions = hub.sessions.lock().await.len();
    let body =
        format!("event: lounge.mcp.ready\ndata: {{\"ok\":true,\"sessions\":{sessions}}}\n\n");
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::wait_clock::{ManualWaitClock, WaitClock};
    use std::sync::Arc;
    use std::time::Duration;

    async fn start_test_server(server: McpServer) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router_from_server(server);
        tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        format!("http://{addr}")
    }

    async fn mcp_post_json(
        http: &reqwest::Client,
        base: &str,
        session: &str,
        client_name: &str,
        body: Value,
    ) -> reqwest::Response {
        http.post(format!("{base}/mcp"))
            .header("content-type", "application/json")
            .header("mcp-session-id", session)
            .header("x-lounge-client-name", client_name)
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    /// Üretim yolu: worker initialize → agent_sessions bağlanır → yield (elle upsert yok).
    #[tokio::test]
    async fn concurrent_call_agent_yield_via_initialize_binding() {
        let store = ExperienceStore::memory().unwrap();
        let clock = Arc::new(ManualWaitClock::new());
        let server = McpServer::new(store.clone(), "nats://127.0.0.1:9")
            .with_skip_nats(true)
            .with_orchestrator_clock(clock.clone() as Arc<dyn WaitClock>);
        server.timeouts().set_global_override_secs(Some(30));
        let base = start_test_server(server).await;
        let http = reqwest::Client::new();

        let caller = "caller-sess-http-1";
        let worker = "worker-http-1";

        // Worker üretim bağlama: initialize (clientInfo.name=worker).
        let init = mcp_post_json(
            &http,
            &base,
            worker,
            "worker",
            serde_json::json!({
                "jsonrpc":"2.0","id":0,"method":"initialize",
                "params":{"protocolVersion":"2025-03-26","capabilities":{},
                    "clientInfo":{"name":"worker","version":"1"}}
            }),
        )
        .await;
        assert!(init.status().is_success());

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

        let health = http.get(format!("{base}/health")).send().await.unwrap();
        assert!(health.status().is_success());

        let yield_res = mcp_post_json(
            &http,
            &base,
            worker,
            "worker",
            serde_json::json!({
                "jsonrpc":"2.0","id":2,"method":"tools/call",
                "params":{"name":"lounge_yield_result","arguments":{
                    "task_id": task_id, "status":"completed", "output":{"ok":true}
                }}
            }),
        )
        .await;
        let yield_json: Value = yield_res.json().await.unwrap();
        assert_ne!(yield_json["result"]["isError"], true, "{yield_json}");

        clock.advance(Duration::from_millis(100));
        let call_res = call_fut.await.unwrap();
        let call_json: Value = call_res.json().await.unwrap();
        let text = call_json["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("");
        assert!(
            text.contains("\"status\":\"completed\"") || text.contains("\"status\": \"completed\""),
            "exact completed expected: {call_json}"
        );
        assert!(
            text.contains("ok") || text.contains("result"),
            "{call_json}"
        );
    }

    /// Uçuştayken context canceled → cancelled (not completed/backgrounded).
    #[tokio::test]
    async fn in_flight_context_canceled_via_http() {
        let store = ExperienceStore::memory().unwrap();
        let clock = Arc::new(ManualWaitClock::new());
        let server = McpServer::new(store.clone(), "nats://127.0.0.1:9")
            .with_skip_nats(true)
            .with_orchestrator_clock(clock.clone() as Arc<dyn WaitClock>);
        server.timeouts().set_global_override_secs(Some(30));
        let base = start_test_server(server).await;
        let http = reqwest::Client::new();
        let caller = "caller-cancel-1";

        let call_body = serde_json::json!({
            "jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"lounge_call_agent","arguments":{
                "target_agent":"worker","task":"x","project_id":"p","wait":true
            }}
        });
        let base_c = base.clone();
        let http_c = http.clone();
        let call_fut = tokio::spawn(async move {
            http_c
                .post(format!("{base_c}/mcp"))
                .header("content-type", "application/json")
                .header("mcp-session-id", caller)
                .header("x-lounge-client-name", "cursor")
                .json(&call_body)
                .send()
                .await
                .unwrap()
        });

        for _ in 0..80 {
            clock.advance(Duration::from_millis(5));
            tokio::time::sleep(Duration::from_millis(5)).await;
            let n: i64 = {
                let conn = store.conn.lock().unwrap();
                conn.query_row("SELECT COUNT(*) FROM a2a_tasks", [], |r| r.get(0))
                    .unwrap_or(0)
            };
            if n > 0 {
                break;
            }
        }

        let cancel_res = mcp_post_json(
            &http,
            &base,
            caller,
            "cursor",
            serde_json::json!({
                "jsonrpc":"2.0","method":"notifications/cancelled",
                "params":{"requestId":7,"reason":"context canceled"}
            }),
        )
        .await;
        assert!(cancel_res.status() == StatusCode::NO_CONTENT || cancel_res.status().is_success());

        clock.advance(Duration::from_millis(100));
        let call_json: Value = call_fut.await.unwrap().json().await.unwrap();
        let text = call_json["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("");
        assert!(
            text.contains("cancelled") || text.contains("canceled"),
            "expected cancelled, got {call_json}"
        );
        assert!(
            !text.contains("\"status\":\"completed\""),
            "must not complete after user stop: {call_json}"
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
        assert!(super::super::session_id::is_valid_session_id(sid));
        assert_ne!(sid, "../evil");
    }

    #[tokio::test]
    async fn loopback_mcp_works_without_token() {
        let store = ExperienceStore::memory().unwrap();
        let server = McpServer::new(store, "nats://127.0.0.1:9").with_skip_nats(true);
        let base = start_test_server(server).await;
        let res = reqwest::Client::new()
            .get(format!("{base}/mcp/health"))
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success(), "{}", res.status());
    }

    #[tokio::test]
    async fn non_allowlisted_host_rejected() {
        crate::services::lounge_auth::clear_allowed_origins();
        let store = ExperienceStore::memory().unwrap();
        let server = McpServer::new(store, "nats://127.0.0.1:9").with_skip_nats(true);
        let base = start_test_server(server).await;
        let res = reqwest::Client::new()
            .get(format!("{base}/mcp/health"))
            .header("host", "evil.example.com")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn allowlisted_host_with_valid_token_accepted() {
        crate::services::lounge_auth::clear_allowed_origins();
        crate::services::lounge_auth::add_allowed_origin("https://tunnel.example.com").unwrap();
        let token = crate::services::lounge_auth::lounge_token();
        let store = ExperienceStore::memory().unwrap();
        let server = McpServer::new(store, "nats://127.0.0.1:9").with_skip_nats(true);
        let base = start_test_server(server).await;
        let res = reqwest::Client::new()
            .get(format!("{base}/mcp/health"))
            .header("host", "tunnel.example.com")
            .header(HDR_LOUNGE_TOKEN, &token)
            .send()
            .await
            .unwrap();
        assert!(res.status().is_success(), "{}", res.status());
        crate::services::lounge_auth::clear_allowed_origins();
    }

    #[tokio::test]
    async fn allowlisted_host_with_invalid_token_rejected() {
        crate::services::lounge_auth::clear_allowed_origins();
        crate::services::lounge_auth::add_allowed_origin("tunnel.example.com").unwrap();
        let store = ExperienceStore::memory().unwrap();
        let server = McpServer::new(store, "nats://127.0.0.1:9").with_skip_nats(true);
        let base = start_test_server(server).await;
        let res = reqwest::Client::new()
            .post(format!("{base}/mcp"))
            .header("host", "tunnel.example.com")
            .header(HDR_LOUNGE_TOKEN, "not-the-token")
            .header("content-type", "application/json")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        crate::services::lounge_auth::clear_allowed_origins();
    }
}
