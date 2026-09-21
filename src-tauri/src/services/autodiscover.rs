use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use super::lmr_runtime::{host_ollama_down_detail, host_ollama_installed};
use super::probe::find_executable;
use crate::infra::quotas::http_json;
use crate::models::{
    now_rfc3339, tool_id, DiscoveredTool, DiscoveryReport, DiscoverySource, SystemTool,
};

const SOURCE_CLAUDE: &str = "claude_desktop";
const SOURCE_CLAUDE_CLI: &str = "claude_cli";
const SOURCE_CURSOR: &str = "cursor";
const SOURCE_GROK: &str = "grok_bot";
const SOURCE_ANTIGRAVITY: &str = "antigravity";
const SOURCE_LMR: &str = "lmr";
const SOURCE_OLLAMA: &str = "ollama";
const SOURCE_SYSTEM: &str = "system";
const SYSTEM_BINARIES: &[&str] = &["gh", "docker", "git"];

pub async fn discovery_report(
    workspace: PathBuf,
    lounge_endpoint: String,
    system_endpoint: String,
) -> Result<DiscoveryReport> {
    let skip_host_ollama =
        normalize_endpoint(&lounge_endpoint) == normalize_endpoint(&system_endpoint);
    let cursor_workspace = workspace.clone();
    let antigravity_workspace = workspace;
    let (hosts, claude, cursor, antigravity, lmr, host_ollama, system) = tokio::join!(
        async {
            tokio::task::spawn_blocking(scan_subscription_hosts)
                .await
                .unwrap_or_else(|_| Vec::new())
        },
        async {
            tokio::task::spawn_blocking(scan_claude_desktop)
                .await
                .unwrap_or_else(|err| join_error(SOURCE_CLAUDE, err))
        },
        async {
            tokio::task::spawn_blocking(move || scan_cursor(&cursor_workspace))
                .await
                .unwrap_or_else(|err| join_error(SOURCE_CURSOR, err))
        },
        async {
            tokio::task::spawn_blocking(move || scan_antigravity(&antigravity_workspace))
                .await
                .unwrap_or_else(|err| join_error(SOURCE_ANTIGRAVITY, err))
        },
        scan_ollama(&lounge_endpoint, SOURCE_LMR),
        async {
            if skip_host_ollama {
                (
                    DiscoverySource {
                        id: SOURCE_OLLAMA.into(),
                        available: false,
                        origin_path: Some(system_endpoint.clone()),
                        detail: Some("Ollama uç noktası LMR ile aynı".into()),
                    },
                    Vec::new(),
                )
            } else {
                scan_host_ollama(&system_endpoint).await
            }
        },
        async {
            tokio::task::spawn_blocking(scan_system_tools)
                .await
                .unwrap_or_else(|err| {
                    (
                        DiscoverySource {
                            id: SOURCE_SYSTEM.into(),
                            available: false,
                            origin_path: None,
                            detail: Some(format!("tarama iptal: {err}")),
                        },
                        Vec::new(),
                    )
                })
        }
    );

    let (claude_source, claude_plugins) = claude;
    let (cursor_source, cursor_plugins) = cursor;
    let (antigravity_source, antigravity_plugins) = antigravity;
    let (lmr_source, lmr_models) = lmr;
    let (ollama_source, ollama_models) = host_ollama;
    let (system_source, system_tools) = system;

    let apps = hosts;
    let mcp_servers =
        merge_plugins_by_name([claude_plugins, cursor_plugins, antigravity_plugins].concat());
    let models = [lmr_models, ollama_models].concat();
    let system_discovered: Vec<DiscoveredTool> =
        system_tools.iter().map(SystemTool::to_discovered).collect();

    let mut sources = vec![
        claude_source,
        cursor_source,
        antigravity_source,
        lmr_source,
        ollama_source,
        system_source,
    ];
    for app in &apps {
        if !sources.iter().any(|row| row.id == app.source) {
            sources.push(DiscoverySource {
                id: app.source.clone(),
                available: app.available,
                origin_path: app.origin_path.clone(),
                detail: app.detail.clone(),
            });
        } else if app.available {
            if let Some(source) = sources.iter_mut().find(|row| row.id == app.source) {
                source.available = true;
                if source.origin_path.is_none() {
                    source.origin_path = app.origin_path.clone();
                }
            }
        }
    }

    let mut tools = Vec::new();
    tools.extend(apps.clone());
    tools.extend(models.clone());
    tools.extend(mcp_servers.clone());
    tools.extend(system_discovered);

    Ok(DiscoveryReport {
        scanned_at: now_rfc3339(),
        sources,
        tools,
        apps,
        models,
        mcp_servers,
        system_tools,
    })
}

fn join_error(id: &str, err: tokio::task::JoinError) -> (DiscoverySource, Vec<DiscoveredTool>) {
    (
        DiscoverySource {
            id: id.to_string(),
            available: false,
            origin_path: None,
            detail: Some(format!("tarama iptal: {err}")),
        },
        Vec::new(),
    )
}

pub fn scan_subscription_hosts() -> Vec<DiscoveredTool> {
    vec![
        find_claude_desktop().unwrap_or_else(|| unavailable_host(SOURCE_CLAUDE, "Claude Desktop")),
        find_claude_cli().unwrap_or_else(|| unavailable_host(SOURCE_CLAUDE_CLI, "Claude CLI")),
        find_cursor_app().unwrap_or_else(|| unavailable_host(SOURCE_CURSOR, "Cursor")),
        find_grok_bot().unwrap_or_else(|| unavailable_host(SOURCE_GROK, "Grok Bot")),
        find_antigravity().unwrap_or_else(|| unavailable_host(SOURCE_ANTIGRAVITY, "Antigravity")),
    ]
}

fn unavailable_host(source: &str, name: &str) -> DiscoveredTool {
    let mut tool = DiscoveredTool::host_app(source, name);
    tool.available = false;
    tool.detail = Some("kurulu değil".into());
    tool
}

pub fn find_claude_desktop() -> Option<DiscoveredTool> {
    let paths = app_candidates(&[
        "Claude.app",
        "Claude Desktop.app",
        r"Claude\Claude.exe",
        r"AnthropicClaude\Claude.exe",
    ]);
    let support = claude_support_dir();
    let origin = first_existing(&paths)
        .or_else(|| support.filter(|path| path.exists()))
        .or_else(|| find_executable("claude-desktop"))
        .or_else(|| find_executable("Claude"));
    origin.map(|path| {
        let mut tool = DiscoveredTool::host_app(SOURCE_CLAUDE, "Claude Desktop");
        tool.origin_path = Some(path.display().to_string());
        tool.detail = Some(host_detail(true, process_running(&["Claude"])));
        tool
    })
}

pub fn find_claude_cli() -> Option<DiscoveredTool> {
    let binary = find_executable("claude");
    let home_cfg = home_dir().map(|home| home.join(".claude"));
    let origin = binary
        .clone()
        .or_else(|| home_cfg.filter(|path| path.exists()));
    origin.map(|path| {
        let mut tool = DiscoveredTool::host_app(SOURCE_CLAUDE_CLI, "Claude CLI");
        tool.kind = "cli".into();
        tool.origin_path = Some(path.display().to_string());
        tool.command = binary.map(|bin| bin.display().to_string());
        tool.detail = Some(host_detail(true, false));
        tool
    })
}

pub fn find_cursor_app() -> Option<DiscoveredTool> {
    let paths = app_candidates(&["Cursor.app", r"Cursor\Cursor.exe", r"cursor\Cursor.exe"]);
    let support = Some(cursor_user_dir()).filter(|path| path.exists());
    first_existing(&paths)
        .or(support)
        .or_else(|| find_executable("cursor"))
        .or_else(|| find_executable("Cursor"))
        .map(|path| {
            let mut tool = DiscoveredTool::host_app(SOURCE_CURSOR, "Cursor");
            tool.origin_path = Some(path.display().to_string());
            tool.detail = Some(host_detail(true, process_running(&["Cursor"])));
            tool
        })
}

pub fn find_grok_bot() -> Option<DiscoveredTool> {
    let paths = app_candidates(&["Grok.app", "Grok Bot.app", r"Grok\Grok.exe"]);
    let support = grok_support_dir().filter(|path| path.exists());
    let running = process_running(&["Grok Bot", "Grok"]);
    first_existing(&paths)
        .or(support)
        .or_else(|| find_executable("grok"))
        .or_else(|| find_executable("Grok"))
        .or_else(|| running.then(|| PathBuf::from("Grok Bot")))
        .map(|path| {
            let mut tool = DiscoveredTool::host_app(SOURCE_GROK, "Grok Bot");
            if path.as_os_str() != "Grok Bot" {
                tool.origin_path = Some(path.display().to_string());
            }
            tool.detail = Some(host_detail(true, running));
            tool
        })
}

pub fn find_antigravity() -> Option<DiscoveredTool> {
    let paths = app_candidates(&[
        "Antigravity.app",
        "Antigravity IDE.app",
        "Google Antigravity.app",
        r"Antigravity\Antigravity.exe",
        r"Google\Antigravity\Antigravity.exe",
    ]);
    let support = antigravity_support_dir().filter(|path| path.exists());
    let gemini = home_dir()
        .map(|home| home.join(".gemini"))
        .filter(|path| path.exists());
    let running = process_running(&["Antigravity", "Antigravity IDE"]);
    first_existing(&paths)
        .or(support)
        .or(gemini)
        .or_else(|| find_executable("antigravity"))
        .or_else(|| find_executable("Antigravity"))
        .or_else(|| running.then(|| PathBuf::from("Antigravity")))
        .map(|path| {
            let mut tool = DiscoveredTool::host_app(SOURCE_ANTIGRAVITY, "Antigravity");
            if path.as_os_str() != "Antigravity" {
                tool.origin_path = Some(path.display().to_string());
            }
            tool.detail = Some(host_detail(true, running));
            tool
        })
}

fn host_detail(installed: bool, running: bool) -> String {
    if running {
        "abonelik · çalışıyor".into()
    } else if installed {
        "abonelik · kurulu".into()
    } else {
        "kurulu değil".into()
    }
}

fn app_candidates(names: &[&str]) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if cfg!(target_os = "macos") {
        for name in names {
            if name.ends_with(".app") {
                paths.push(PathBuf::from("/Applications").join(name));
                if let Some(home) = home_dir() {
                    paths.push(home.join("Applications").join(name));
                }
            }
        }
    }
    if cfg!(target_os = "windows") {
        let mut roots = Vec::new();
        if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
            roots.push(local.clone());
            roots.push(local.join("Programs"));
        }
        for key in ["PROGRAMFILES", "ProgramFiles(x86)"] {
            if let Some(root) = std::env::var_os(key).map(PathBuf::from) {
                roots.push(root);
            }
        }
        for root in roots {
            for name in names {
                if name.ends_with(".exe") {
                    paths.push(root.join(name));
                }
            }
        }
    }
    if cfg!(target_os = "linux") {
        for name in names {
            for stem in linux_app_stems(name) {
                if let Some(binary) = find_executable(&stem) {
                    paths.push(binary);
                }
                paths.push(PathBuf::from("/opt").join(&stem).join(&stem));
                paths.push(PathBuf::from("/usr/bin").join(&stem));
                paths.push(PathBuf::from("/usr/local/bin").join(&stem));
                paths
                    .push(PathBuf::from("/usr/share/applications").join(format!("{stem}.desktop")));
                if let Some(home) = home_dir() {
                    paths.push(home.join(".local/bin").join(&stem));
                    paths.push(
                        home.join(".local/share/applications")
                            .join(format!("{stem}.desktop")),
                    );
                }
            }
        }
    }
    paths
}

fn linux_app_stems(name: &str) -> Vec<String> {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim_end_matches(".app")
        .trim_end_matches(".exe")
        .trim_end_matches(".desktop");
    let mut stems = vec![base.to_string(), base.to_ascii_lowercase()];
    match base.to_ascii_lowercase().as_str() {
        "claude" | "claude desktop" => {
            stems.push("claude-desktop".into());
            stems.push("claude".into());
        }
        "cursor" => stems.push("cursor".into()),
        "grok" | "grok bot" => {
            stems.push("grok".into());
            stems.push("grok-bot".into());
        }
        "antigravity" | "antigravity ide" | "google antigravity" => {
            stems.push("antigravity".into());
            stems.push("antigravity-ide".into());
        }
        _ => {}
    }
    stems.sort();
    stems.dedup();
    stems
}

fn first_existing(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|path| path.exists()).cloned()
}

fn process_running(needles: &[&str]) -> bool {
    use sysinfo::{ProcessesToUpdate, System};
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    sys.processes().values().any(|proc| {
        let name = proc.name().to_string_lossy().to_lowercase();
        needles
            .iter()
            .any(|needle| name.contains(&needle.to_lowercase()))
    })
}

fn grok_support_dir() -> Option<PathBuf> {
    os_config_dir("Grok")
}

fn antigravity_support_dir() -> Option<PathBuf> {
    os_config_dir("Antigravity")
}

fn antigravity_config_paths(workspace: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = home_dir() {
        paths.push(home.join(".gemini/config/mcp_config.json"));
        paths.push(home.join(".gemini/antigravity/mcp_config.json"));
    }
    if let Some(support) = antigravity_support_dir() {
        paths.push(support.join("User/mcp_config.json"));
    }
    paths.push(workspace.join(".agents/mcp_config.json"));
    paths
}

fn claude_support_dir() -> Option<PathBuf> {
    ["Claude", "claude"]
        .into_iter()
        .filter_map(os_config_dir)
        .find(|path| path.exists())
        .or_else(|| os_config_dir("Claude"))
}

pub fn scan_claude_desktop() -> (DiscoverySource, Vec<DiscoveredTool>) {
    let (mut source, plugins) = scan_mcp_path(SOURCE_CLAUDE, &claude_config_path());
    if let Some(host) = find_claude_desktop() {
        source.available = source.available || host.available;
        if source.origin_path.is_none() {
            source.origin_path = host.origin_path.clone();
        }
        if host.available && source.detail.as_deref() == Some("config dosyası yok") {
            source.detail = Some("uygulama kurulu".into());
        }
    }
    (source, plugins)
}

pub fn scan_cursor(workspace: &Path) -> (DiscoverySource, Vec<DiscoveredTool>) {
    let paths = cursor_config_paths(workspace);
    let mut found = Vec::new();
    let mut tools = Vec::new();
    let host_installed = find_cursor_app().is_some_and(|row| row.available);

    for path in &paths {
        match read_mcp_file(path, SOURCE_CURSOR) {
            Ok(Some(rows)) => {
                found.push(path.display().to_string());
                tools.extend(rows);
            }
            Ok(None) => {}
            Err(_) => {
                found.push(path.display().to_string());
            }
        }
    }

    let available = host_installed || !found.is_empty();
    let origin_path = found.first().cloned().or_else(|| {
        find_cursor_app()
            .filter(|row| row.available)
            .and_then(|row| row.origin_path)
    });
    let detail = if !found.is_empty() {
        if tools.is_empty() {
            Some(format!("{} konum · mcpServers yok", found.len()))
        } else {
            Some(format!("{} araç", tools.len()))
        }
    } else if host_installed {
        Some("uygulama kurulu".into())
    } else {
        Some("config yok".into())
    };

    (
        DiscoverySource {
            id: SOURCE_CURSOR.into(),
            available,
            origin_path,
            detail,
        },
        dedupe_tools(tools),
    )
}

pub fn scan_antigravity(workspace: &Path) -> (DiscoverySource, Vec<DiscoveredTool>) {
    let paths = antigravity_config_paths(workspace);
    let mut found = Vec::new();
    let mut tools = Vec::new();
    let mut seen_files = std::collections::HashSet::new();
    let host_installed = find_antigravity().is_some_and(|row| row.available);

    for path in &paths {
        let key = fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        if !seen_files.insert(key) {
            continue;
        }
        match read_mcp_file(path, SOURCE_ANTIGRAVITY) {
            Ok(Some(rows)) => {
                found.push(path.display().to_string());
                tools.extend(rows);
            }
            Ok(None) => {}
            Err(_) => {
                found.push(path.display().to_string());
            }
        }
    }

    let available = host_installed || !found.is_empty();
    let origin_path = found.first().cloned().or_else(|| {
        find_antigravity()
            .filter(|row| row.available)
            .and_then(|row| row.origin_path)
    });
    let detail = if !tools.is_empty() {
        Some(format!("{} araç", tools.len()))
    } else if host_installed {
        Some("uygulama kurulu".into())
    } else if !found.is_empty() {
        Some("mcpServers yok".into())
    } else {
        Some("config yok".into())
    };

    (
        DiscoverySource {
            id: SOURCE_ANTIGRAVITY.into(),
            available,
            origin_path,
            detail,
        },
        dedupe_tools(tools),
    )
}

pub fn merge_plugins_by_name(plugins: Vec<DiscoveredTool>) -> Vec<DiscoveredTool> {
    let mut order = Vec::new();
    let mut grouped = std::collections::HashMap::<String, DiscoveredTool>::new();
    for plugin in plugins {
        let key = plugin.name.to_ascii_lowercase();
        if let Some(existing) = grouped.get_mut(&key) {
            let mut hosts = existing.hosts();
            for host in plugin.hosts() {
                if !hosts.iter().any(|row| row == &host) {
                    hosts.push(host);
                }
            }
            existing.host_ids = hosts.clone();
            existing.host_id = hosts.first().cloned();
            existing.id = format!("plugin:{}", existing.name);
            existing.detail = Some(crate::models::format_host_labels(&hosts));
            if let (Some(left), Some(right)) = (&existing.origin_path, &plugin.origin_path) {
                if left != right && !left.contains(right) {
                    existing.origin_path = Some(format!("{left}, {right}"));
                }
            }
        } else {
            order.push(key.clone());
            grouped.insert(key, plugin);
        }
    }
    order
        .into_iter()
        .filter_map(|key| grouped.remove(&key))
        .map(|mut tool| {
            let hosts = tool.hosts();
            tool.host_ids = hosts.clone();
            tool.host_id = hosts.first().cloned();
            if hosts.len() > 1 {
                tool.id = format!("plugin:{}", tool.name);
                tool.detail = Some(crate::models::format_host_labels(&hosts));
            }
            tool
        })
        .collect()
}

pub async fn scan_host_ollama(endpoint: &str) -> (DiscoverySource, Vec<DiscoveredTool>) {
    let (source, tools) = scan_ollama(endpoint, SOURCE_OLLAMA).await;
    if source.available {
        return (source, tools);
    }
    (
        DiscoverySource {
            id: SOURCE_OLLAMA.into(),
            available: false,
            origin_path: Some(endpoint.trim_end_matches('/').to_string()),
            detail: Some(host_ollama_down_detail(host_ollama_installed()).into()),
        },
        Vec::new(),
    )
}

pub async fn scan_ollama(endpoint: &str, source: &str) -> (DiscoverySource, Vec<DiscoveredTool>) {
    let url = format!("{}/api/tags", endpoint.trim_end_matches('/'));
    match http_json(&url).await {
        Ok(payload) => {
            let tools = parse_ollama_tags(&payload, endpoint, source);
            let detail = if tools.is_empty() {
                Some(format!("{} ayakta, model yok", ollama_source_label(source)))
            } else {
                Some(format!("{} model", tools.len()))
            };
            (
                DiscoverySource {
                    id: source.into(),
                    available: true,
                    origin_path: Some(endpoint.trim_end_matches('/').to_string()),
                    detail,
                },
                tools,
            )
        }
        Err(err) => {
            log::debug!("Ollama tarama başarısız ({source}): {err}");
            (
                DiscoverySource {
                    id: source.into(),
                    available: false,
                    origin_path: Some(endpoint.trim_end_matches('/').to_string()),
                    detail: Some(if source == SOURCE_LMR {
                        "ayağa kalkmadı".into()
                    } else {
                        "yanıt yok".into()
                    }),
                },
                Vec::new(),
            )
        }
    }
}

fn ollama_source_label(source: &str) -> &'static str {
    match source {
        SOURCE_LMR => "LMR",
        SOURCE_OLLAMA => "Ollama",
        _ => "Ollama",
    }
}

fn ollama_source_detail(source: &str) -> &'static str {
    match source {
        SOURCE_LMR => "Lounge Model Runner",
        SOURCE_OLLAMA => "Ollama Sunucusu",
        _ => "Ollama",
    }
}

fn normalize_endpoint(endpoint: &str) -> String {
    endpoint.trim().trim_end_matches('/').to_ascii_lowercase()
}

pub fn scan_system_tools() -> (DiscoverySource, Vec<SystemTool>) {
    scan_named_binaries(SYSTEM_BINARIES)
}

pub fn scan_named_binaries(names: &[&str]) -> (DiscoverySource, Vec<SystemTool>) {
    let tools: Vec<SystemTool> = names
        .iter()
        .map(|name| match find_executable(name) {
            Some(path) => SystemTool {
                id: tool_id(SOURCE_SYSTEM, name),
                name: (*name).to_string(),
                available: true,
                path: Some(path.display().to_string()),
                detail: None,
            },
            None => SystemTool {
                id: tool_id(SOURCE_SYSTEM, name),
                name: (*name).to_string(),
                available: false,
                path: None,
                detail: Some("PATH'te yok".into()),
            },
        })
        .collect();
    let found = tools.iter().filter(|tool| tool.available).count();
    (
        DiscoverySource {
            id: SOURCE_SYSTEM.into(),
            available: found > 0,
            origin_path: None,
            detail: Some(format!("{found} / {} PATH", tools.len())),
        },
        tools,
    )
}

pub fn parse_mcp_servers(value: &Value, source: &str, origin_path: &Path) -> Vec<DiscoveredTool> {
    let Some(servers) = value.get("mcpServers").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut tools = Vec::with_capacity(servers.len());
    for (name, spec) in servers {
        let mut tool = DiscoveredTool::new(source, name, "plugin");
        tool.origin_path = Some(origin_path.display().to_string());
        tool.command = spec
            .get("command")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        tool.args = spec
            .get("args")
            .and_then(Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        tool.endpoint = spec
            .get("url")
            .or_else(|| spec.get("endpoint"))
            .or_else(|| spec.get("serverUrl"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        tool.detail = env_key_detail(spec.get("env"));
        tool.available = true;
        tools.push(tool);
    }
    tools
}

pub fn parse_ollama_tags(payload: &Value, endpoint: &str, source: &str) -> Vec<DiscoveredTool> {
    payload
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("name").and_then(Value::as_str))
        .map(|name| {
            let mut tool = DiscoveredTool::new(source, name, "model");
            tool.endpoint = Some(endpoint.trim_end_matches('/').to_string());
            tool.origin_path = Some(format!("{}/api/tags", endpoint.trim_end_matches('/')));
            tool.detail = Some(ollama_source_detail(source).into());
            tool.available = true;
            tool.access_mode = "local".into();
            tool
        })
        .collect()
}

fn scan_mcp_path(source: &str, path: &Path) -> (DiscoverySource, Vec<DiscoveredTool>) {
    match read_mcp_file(path, source) {
        Ok(Some(tools)) => (
            DiscoverySource {
                id: source.into(),
                available: true,
                origin_path: Some(path.display().to_string()),
                detail: Some(format!("{} araç", tools.len())),
            },
            tools,
        ),
        Ok(None) => (
            DiscoverySource {
                id: source.into(),
                available: false,
                origin_path: Some(path.display().to_string()),
                detail: Some("config dosyası yok".into()),
            },
            Vec::new(),
        ),
        Err(err) => (
            DiscoverySource {
                id: source.into(),
                available: true,
                origin_path: Some(path.display().to_string()),
                detail: Some(err.to_string()),
            },
            Vec::new(),
        ),
    }
}

fn read_mcp_file(path: &Path, source: &str) -> Result<Option<Vec<DiscoveredTool>>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("MCP config okunamadı: {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("JSON parse hatası {}", path.display()))?;
    Ok(Some(parse_mcp_servers(&value, source, path)))
}

fn env_key_detail(env: Option<&Value>) -> Option<String> {
    let keys = env.and_then(Value::as_object).map(|map| {
        let mut names: Vec<&str> = map.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    })?;
    if keys.is_empty() {
        return None;
    }
    Some(format!("env keys: {}", keys.join(", ")))
}

fn dedupe_tools(tools: Vec<DiscoveredTool>) -> Vec<DiscoveredTool> {
    let mut seen = std::collections::HashSet::new();
    tools
        .into_iter()
        .filter(|tool| seen.insert(tool.id.clone()))
        .collect()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn os_config_dir(app: &str) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        return home_dir().map(|home| home.join("Library/Application Support").join(app));
    }
    if cfg!(target_os = "windows") {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return Some(PathBuf::from(appdata).join(app));
        }
        return home_dir().map(|home| home.join("AppData/Roaming").join(app));
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(xdg).join(app));
    }
    home_dir().map(|home| home.join(".config").join(app))
}

fn claude_config_path() -> PathBuf {
    claude_support_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claude_desktop_config.json")
}

fn cursor_user_dir() -> PathBuf {
    os_config_dir("Cursor").unwrap_or_else(|| PathBuf::from("Cursor"))
}

fn cursor_config_paths(workspace: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = home_dir() {
        paths.push(home.join(".cursor/mcp.json"));
    }
    paths.push(workspace.join(".cursor/mcp.json"));
    paths.push(cursor_user_dir().join("User/settings.json"));
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_claude_mcp_servers_and_redacts_env_values() {
        let secret = "sk-live-super-secret";
        let doc = json!({
            "mcpServers": {
                "github": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-github"],
                    "env": {
                        "GITHUB_TOKEN": secret,
                        "ANOTHER": "also-secret"
                    }
                }
            }
        });
        let path = PathBuf::from("/tmp/claude_desktop_config.json");
        let tools = parse_mcp_servers(&doc, SOURCE_CLAUDE, &path);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].id, "claude_desktop:github");
        assert_eq!(tools[0].kind, "plugin");
        assert_eq!(tools[0].access_mode, "plugin");
        assert_eq!(tools[0].host_id.as_deref(), Some(SOURCE_CLAUDE));
        assert_eq!(tools[0].command.as_deref(), Some("npx"));
        assert_eq!(
            tools[0].args,
            vec!["-y", "@modelcontextprotocol/server-github"]
        );
        let detail = tools[0].detail.clone().unwrap_or_default();
        assert!(detail.contains("GITHUB_TOKEN"));
        assert!(detail.contains("ANOTHER"));
        assert!(!detail.contains(secret));
        assert!(!detail.contains("also-secret"));

        let serialized = serde_json::to_string(&tools[0]).expect("json");
        assert!(!serialized.contains(secret));
        assert!(!serialized.contains("also-secret"));
        assert!(!serialized.contains("sk-live"));
    }

    #[test]
    fn parses_cursor_mcp_fixture_file() {
        let dir = std::env::temp_dir().join(format!("lounge-mcp-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("mcp.json");
        fs::write(
            &path,
            r#"{
              "mcpServers": {
                "notion": {
                  "command": "npx",
                  "args": ["-y", "@notionhq/mcp"],
                  "env": { "NOTION_TOKEN": "ntn_secret_value" }
                }
              }
            }"#,
        )
        .expect("write fixture");

        let rows = read_mcp_file(&path, SOURCE_CURSOR)
            .expect("read")
            .expect("exists");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "cursor:notion");
        assert_eq!(rows[0].kind, "plugin");
        assert_eq!(rows[0].access_mode, "plugin");
        let blob = serde_json::to_string(&rows[0]).expect("json");
        assert!(!blob.contains("ntn_secret_value"));
        assert!(!blob.to_ascii_lowercase().contains("env\":{"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_mcp_file_is_unavailable() {
        let path = PathBuf::from("/tmp/lounge-missing-claude-config.json");
        let (source, tools) = scan_mcp_path(SOURCE_CLAUDE, &path);
        assert!(!source.available);
        assert!(tools.is_empty());
    }

    #[test]
    fn ollama_tags_become_model_rows() {
        let payload = json!({
            "models": [
                { "name": "llama3.1:8b" },
                { "name": "qwen2.5:7b" }
            ]
        });
        let tools = parse_ollama_tags(&payload, "http://127.0.0.1:18790", SOURCE_LMR);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].id, "lmr:llama3.1:8b");
        assert_eq!(tools[0].kind, "model");
        assert_eq!(tools[0].access_mode, "local");
        assert_eq!(tools[0].source, "lmr");
        assert_eq!(tools[0].endpoint.as_deref(), Some("http://127.0.0.1:18790"));

        let host = parse_ollama_tags(&payload, "http://127.0.0.1:11434", SOURCE_OLLAMA);
        assert_eq!(host[0].id, "ollama:llama3.1:8b");
        assert_eq!(host[0].source, "ollama");
    }

    #[tokio::test]
    async fn ollama_down_returns_empty_models() {
        let (source, tools) = scan_ollama("http://127.0.0.1:9", SOURCE_OLLAMA).await;
        assert!(!source.available);
        assert_eq!(source.id, SOURCE_OLLAMA);
        assert_eq!(source.detail.as_deref(), Some("yanıt yok"));
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn lmr_down_is_not_host_bulunamadi() {
        let (source, tools) = scan_ollama("http://127.0.0.1:9", SOURCE_LMR).await;
        assert!(!source.available);
        assert_eq!(source.id, SOURCE_LMR);
        assert_eq!(source.detail.as_deref(), Some("ayağa kalkmadı"));
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn host_ollama_without_install_is_bulunamadi() {
        let (source, tools) = scan_host_ollama("http://127.0.0.1:9").await;
        assert!(!source.available);
        assert_eq!(source.id, SOURCE_OLLAMA);
        assert!(tools.is_empty());
        if host_ollama_installed() {
            assert_eq!(source.detail.as_deref(), Some("yüklü, çalışmıyor"));
        } else {
            assert_eq!(source.detail.as_deref(), Some("bulunamadı"));
        }
    }

    #[test]
    fn git_is_found_on_ci_path() {
        let (source, tools) = scan_named_binaries(&["git"]);
        assert!(source.available);
        assert_eq!(tools.len(), 1);
        assert!(tools[0].available);
        assert_eq!(tools[0].id, "system:git");
        assert!(tools[0].path.is_some());
    }

    #[test]
    fn subscription_hosts_use_app_ids() {
        let hosts = scan_subscription_hosts();
        assert_eq!(hosts.len(), 5);
        assert!(hosts.iter().any(|row| row.id == "app:claude_desktop"));
        assert!(hosts.iter().any(|row| row.id == "app:claude_cli"));
        assert!(hosts.iter().any(|row| row.id == "app:cursor"));
        assert!(hosts.iter().any(|row| row.id == "app:grok_bot"));
        assert!(hosts.iter().any(|row| row.id == "app:antigravity"));
        for host in &hosts {
            assert_eq!(host.access_mode, "subscription");
            assert!(host.kind == "app" || host.kind == "cli");
        }
    }

    #[test]
    fn merge_plugins_lists_every_host() {
        let cursor = DiscoveredTool::new("cursor", "notion", "plugin");
        let anti = DiscoveredTool::new("antigravity", "notion", "plugin");
        let merged = merge_plugins_by_name(vec![cursor, anti]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, "plugin:notion");
        assert_eq!(merged[0].hosts(), vec!["cursor", "antigravity"]);
        assert_eq!(merged[0].detail.as_deref(), Some("Cursor · Antigravity"));
    }

    #[test]
    fn missing_binary_is_unavailable() {
        let (source, tools) = scan_named_binaries(&["__lounge_missing_binary__"]);
        assert!(!source.available);
        assert_eq!(tools.len(), 1);
        assert!(!tools[0].available);
        assert_eq!(tools[0].detail.as_deref(), Some("PATH'te yok"));
    }

    #[test]
    fn linux_stems_cover_desktop_and_windows_exe() {
        let claude = linux_app_stems("Claude.app");
        assert!(claude.iter().any(|stem| stem == "claude-desktop"));
        assert!(claude.iter().any(|stem| stem == "claude"));
        let cursor = linux_app_stems(r"Cursor\Cursor.exe");
        assert!(cursor.iter().any(|stem| stem == "cursor"));
        let anti = linux_app_stems("Google Antigravity.app");
        assert!(anti.iter().any(|stem| stem == "antigravity"));
    }

    #[test]
    fn os_config_dir_is_not_macos_only() {
        let path = os_config_dir("Cursor").expect("config dir");
        let rendered = path.to_string_lossy();
        assert!(rendered.contains("Cursor"), "{rendered}");
        if cfg!(target_os = "windows") {
            assert!(
                rendered.contains("AppData") || rendered.contains("Cursor"),
                "{rendered}"
            );
        } else if cfg!(target_os = "macos") {
            assert!(rendered.contains("Application Support"), "{rendered}");
        } else {
            assert!(
                rendered.contains(".config") || rendered.contains("Cursor"),
                "{rendered}"
            );
        }
    }
}
