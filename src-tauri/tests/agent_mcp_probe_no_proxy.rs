//! B7: prove probe `.no_proxy()` in a **separate process** so HTTP_PROXY /
//! HTTPS_PROXY / ALL_PROXY mutation cannot race the shared lib test suite.
//!
//! Sets proxy env to a dead loopback port, then asserts a real TcpListener on
//! 127.0.0.1 is still reached (exactly one hit, outcome reachable).

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use app_lib::services::agent_health::AgentHealthRoots;
use app_lib::services::agent_mcp_probe::{probe_agent_mcp_url_with, PROBE_OUTCOME_REACHABLE};
use app_lib::services::health_filters::mask_home_path_with;
use serde_json::json;

#[tokio::test]
async fn http_proxy_env_is_not_used_for_loopback_probe() {
    // Set proxy env *first* in this process — never in the shared lib suite.
    // SAFETY: this binary is dedicated to this check; no parallel sibling tests.
    unsafe {
        std::env::set_var("HTTP_PROXY", "http://127.0.0.1:1");
        std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:1");
        std::env::set_var("ALL_PROXY", "http://127.0.0.1:1");
    }

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local_addr");
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_bg = hits.clone();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            hits_bg.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        }
    });

    let base =
        std::env::temp_dir().join(format!("agent-mcp-probe-noproxy-{}", uuid::Uuid::new_v4()));
    let home = base.join("home");
    fs::create_dir_all(&home).expect("mkdir home");
    let roots = AgentHealthRoots {
        home: home.clone(),
        appdata: None,
        xdg_config: Some(home.join(".config")),
        workspace: base.join("ws"),
        path_dirs: Vec::new(),
    };
    fs::create_dir_all(&roots.workspace).expect("mkdir workspace");

    let cfg = roots.home.join(".cursor/mcp.json");
    fs::create_dir_all(cfg.parent().unwrap()).expect("mkdir cursor");
    let url = format!("http://127.0.0.1:{}/mcp?token=sekrit", addr.port());
    fs::write(
        &cfg,
        serde_json::to_string_pretty(&json!({
            "mcpServers": {
                "agent-lounge-os": { "url": url }
            }
        }))
        .unwrap(),
    )
    .expect("write mcp.json");
    let homes = roots.mask_homes();
    let id = mask_home_path_with(&cfg.to_string_lossy(), &homes);

    let result = probe_agent_mcp_url_with("cursor", &id, &roots, true)
        .await
        .expect("probe ok");

    assert_eq!(
        result.outcome, PROBE_OUTCOME_REACHABLE,
        "no_proxy client must reach loopback despite HTTP_PROXY → dead port"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "exactly one request to local server"
    );
    let _ = fs::remove_dir_all(&base);
}
