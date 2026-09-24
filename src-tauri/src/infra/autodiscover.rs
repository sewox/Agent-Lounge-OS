//! Sistem keşfi `services::autodiscover` üzerinden yürür.

pub async fn scan_system(
    workspace: std::path::PathBuf,
    lounge_endpoint: String,
    system_endpoint: String,
) -> crate::models::DiscoveryReport {
    crate::services::autodiscover::discovery_report(workspace, lounge_endpoint, system_endpoint)
        .await
        .unwrap_or_else(|err| crate::models::DiscoveryReport {
            scanned_at: crate::models::now_rfc3339(),
            sources: vec![crate::models::DiscoverySource {
                id: "error".into(),
                available: false,
                origin_path: None,
                detail: Some(err.to_string()),
            }],
            tools: Vec::new(),
            apps: Vec::new(),
            models: Vec::new(),
            mcp_servers: Vec::new(),
            system_tools: Vec::new(),
        })
}
