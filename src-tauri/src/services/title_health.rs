//! H4: LMR / experience-title backlog health (read-only probes).

use std::time::Duration;

use serde_json::Value;

use super::probe::lounge_ollama_endpoint;
use super::quota_manager::lmr_endpoint_up;
use crate::db::ExperienceStore;
use crate::models::TitleHealthReport;
use crate::services::experience_title::{
    last_title_error_type, title_backlog_count, title_model_name, TITLE_LMR_TIMEOUT,
};

/// Collect title/LMR health against the Lounge LMR endpoint.
pub async fn collect_title_health(store: &ExperienceStore) -> TitleHealthReport {
    collect_title_health_at(store, &lounge_ollama_endpoint()).await
}

pub async fn collect_title_health_at(store: &ExperienceStore, endpoint: &str) -> TitleHealthReport {
    let model = title_model_name();
    let queue_backlog = title_backlog_count();
    let db_backlog = store.count_title_backlog().await.unwrap_or(0);
    let give_ups = store.count_title_give_ups().await.unwrap_or(0);
    let last_error_type = last_title_error_type();

    let probe = probe_lmr_title(endpoint, &model, TITLE_LMR_TIMEOUT).await;

    TitleHealthReport {
        queue_backlog,
        db_backlog,
        give_ups,
        last_error_type,
        lmr_reachable: probe.reachable,
        model_installed: probe.model_installed,
        model,
        lmr_endpoint: Some(endpoint.trim_end_matches('/').to_string()),
        probe_error_type: probe.error_type,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LmrTitleProbe {
    pub reachable: bool,
    pub model_installed: bool,
    pub error_type: Option<String>,
}

/// Probe LMR: TCP/HTTP reachability + whether `model` appears in `/api/tags`.
pub async fn probe_lmr_title(endpoint: &str, model: &str, timeout: Duration) -> LmrTitleProbe {
    // Closed port / connect failures → not reachable.
    if !lmr_endpoint_up(endpoint).await {
        return LmrTitleProbe {
            reachable: false,
            model_installed: false,
            error_type: Some("connect".into()),
        };
    }

    let url = format!("{}/api/tags", endpoint.trim_end_matches('/'));
    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(c) => c,
        Err(err) => {
            return LmrTitleProbe {
                reachable: false,
                model_installed: false,
                error_type: Some(format!("other:{err}")),
            };
        }
    };

    match client.get(&url).send().await {
        Ok(resp) => {
            let status = resp.status();
            if !status.is_success() {
                return LmrTitleProbe {
                    reachable: true,
                    model_installed: false,
                    error_type: Some(format!("http:{}", status.as_u16())),
                };
            }
            match resp.json::<Value>().await {
                Ok(body) => {
                    let installed = tags_contain_model(&body, model);
                    LmrTitleProbe {
                        reachable: true,
                        model_installed: installed,
                        error_type: None,
                    }
                }
                Err(err) => LmrTitleProbe {
                    reachable: true,
                    model_installed: false,
                    error_type: Some(format!("other:{err}")),
                },
            }
        }
        Err(err) => {
            let error_type = if err.is_timeout() {
                "timeout".into()
            } else if err.is_connect() {
                "connect".into()
            } else {
                format!("other:{err}")
            };
            LmrTitleProbe {
                reachable: false,
                model_installed: false,
                error_type: Some(error_type),
            }
        }
    }
}

fn tags_contain_model(body: &Value, model: &str) -> bool {
    let want = model.trim().to_ascii_lowercase();
    body.get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("name").and_then(Value::as_str))
        .any(|name| {
            let n = name.to_ascii_lowercase();
            n == want || n.starts_with(&format!("{want}:")) || n.starts_with(&want)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    async fn serve_tags(body: Value, delay: Option<Duration>) -> (String, oneshot::Sender<()>) {
        let (tx, rx) = oneshot::channel::<()>();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/api/tags",
            get(move || {
                let body = body.clone();
                let delay = delay;
                async move {
                    if let Some(d) = delay {
                        tokio::time::sleep(d).await;
                    }
                    Json(body)
                }
            }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
                .ok();
        });
        (format!("http://{addr}"), tx)
    }

    async fn serve_status(code: u16) -> (String, oneshot::Sender<()>) {
        let (tx, rx) = oneshot::channel::<()>();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let app = Router::new().route(
            "/api/tags",
            get(move || async move { (axum::http::StatusCode::from_u16(code).unwrap(), "err") }),
        );
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                })
                .await
                .ok();
        });
        (format!("http://{addr}"), tx)
    }

    #[tokio::test]
    async fn t4_stub_lmr_success() {
        let (url, stop) =
            serve_tags(json!({ "models": [{ "name": "gemma-2-2b-it" }] }), None).await;
        let probe = probe_lmr_title(&url, "gemma-2-2b-it", Duration::from_secs(2)).await;
        assert!(probe.reachable);
        assert!(probe.model_installed);
        assert!(probe.error_type.is_none());
        let _ = stop.send(());
    }

    #[tokio::test]
    async fn t4_stub_lmr_http_500() {
        let (url, stop) = serve_status(500).await;
        let probe = probe_lmr_title(&url, "gemma-2-2b-it", Duration::from_secs(2)).await;
        assert!(probe.reachable);
        assert!(!probe.model_installed);
        assert_eq!(probe.error_type.as_deref(), Some("http:500"));
        let _ = stop.send(());
    }

    #[tokio::test]
    async fn t4_stub_lmr_timeout() {
        let (url, stop) = serve_tags(
            json!({ "models": [{ "name": "gemma-2-2b-it" }] }),
            Some(Duration::from_secs(3)),
        )
        .await;
        let probe = probe_lmr_title(&url, "gemma-2-2b-it", Duration::from_millis(200)).await;
        assert!(!probe.reachable);
        assert_eq!(probe.error_type.as_deref(), Some("timeout"));
        let _ = stop.send(());
    }

    #[tokio::test]
    async fn t4_stub_lmr_closed_port() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let url = format!("http://127.0.0.1:{port}");
        let probe = probe_lmr_title(&url, "gemma-2-2b-it", Duration::from_millis(500)).await;
        assert!(!probe.reachable);
        assert_eq!(probe.error_type.as_deref(), Some("connect"));
    }
}
