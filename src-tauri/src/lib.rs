pub mod bridge;
pub mod db;
pub mod infra;
pub mod kernel;
pub mod models;
pub mod services;

use std::path::PathBuf;

use db::ExperienceStore;
use infra::BusManager;
use kernel::{
    default_model_lock, inject_knowledge_hit, DecisionGate, DecisionGatePhase, DecisionGateStatus,
    Dispatcher, InferMeter, LoungeTelemetry, WorkerRegistry, WorkflowEngine, DECISION_GATE_EVENT,
};
use lounge_protocol::LoungeMessage;
use models::{
    merge_project_summaries, AstNode, ConnectedTool, DeadSymbol, DeviceProfile, DiscoveredTool,
    DiscoveryReport, IndexSnapshot, LoungeExperience, LoungeTask, ProjectSummary, QuotaState,
    RecommendedModels, RoutingPolicy, RoutingVote, SemanticMap, ServiceReport, TaskKind, ToolQuota,
    TASK_REQUESTED,
};
use services::autodiscover::discovery_report;
use services::{
    api_keys_from_store, build_agent_efficiency_report, collect_quota_state_with_keys,
    enable_graph_ui, graph_ui_status, load_port_from_store, on_main_window_closed,
    open_or_focus_graph_window, persist_port, record_dead_snapshot, record_whisper_injection,
    spawn_event_pump, spawn_quota_pump, spawn_supervisor, AgentEfficiencyReport,
    EfficiencyReportQuery, GraphUiState, GraphUiStatus, LayaEngineStatus, MemoryBridge,
    ModelManager, ServiceManager, SharedServices, GRAPH_WINDOW_LABEL,
};
use tauri::{Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const ONBOARDING_ROUTE: &str = "/onboarding";
const DASHBOARD_ROUTE: &str = "/dashboard";

/// `connected_tools` boşsa onboarding, doluysa dashboard.
pub fn initial_window_route() -> &'static str {
    match ExperienceStore::open(db::default_db_path(workspace_root())) {
        Ok(store) => window_route_for_store(&store),
        Err(err) => {
            log::warn!("connected_tools okunamadı, onboarding: {err}");
            ONBOARDING_ROUTE
        }
    }
}

pub(crate) fn window_route_for_store(store: &ExperienceStore) -> &'static str {
    match store.connected_tools_empty() {
        Ok(false) => DASHBOARD_ROUTE,
        Ok(true) => ONBOARDING_ROUTE,
        Err(err) => {
            log::warn!("connected_tools sayısı okunamadı, onboarding: {err}");
            ONBOARDING_ROUTE
        }
    }
}

fn open_main_window(app: &tauri::App, start_route: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut window_config = app
        .config()
        .app
        .windows
        .first()
        .cloned()
        .ok_or("main window config missing")?;
    window_config.url = WebviewUrl::App(PathBuf::from(start_route.trim_start_matches('/')));
    WebviewWindowBuilder::from_config(app.handle(), &window_config)?.build()?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    run_with_start_route(initial_window_route());
}

pub fn run_with_start_route(start_route: &'static str) {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }

            let workspace = workspace_root();
            let store = ExperienceStore::open(db::default_db_path(&workspace))
                .map_err(|err| err.to_string())?;
            open_main_window(app, start_route)?;
            let model = default_model_lock();
            let memory = MemoryBridge::discover().unwrap_or_else(|err| {
                log::warn!("{err}");
                MemoryBridge::from_binary("codebase-memory-mcp")
            });
            let services = ServiceManager::shared_with_memory(memory.clone());
            let graph_ui = GraphUiState::new();
            let (decision_tx, mut decision_rx) = tokio::sync::mpsc::channel(64);
            let gate = DecisionGate::new(decision_tx);
            gate.attach_bias_store(store.clone());
            let workers = WorkerRegistry::with_store(store.clone());
            let dispatcher = Dispatcher::new(
                "nats://127.0.0.1:4222",
                services::lounge_ollama_endpoint(),
                model,
                memory.clone(),
                store.clone(),
                workspace,
            )
            .with_decision_cache(gate.cache())
            .with_workers(workers.clone());
            dispatcher.attach_app(app.handle().clone());
            let workflow = WorkflowEngine::new("nats://127.0.0.1:4222");
            let bus = BusManager::new("nats://127.0.0.1:4222");
            let models = ModelManager::with_nats_url(bus.nats_url());
            let handle = app.handle().clone();
            let supervisor_handle = app.handle().clone();
            let supervisor_services = services.clone();
            let quota_handle = app.handle().clone();
            let quota_services = services.clone();
            let quota_store = store.clone();
            let load_gate = gate.clone();
            let load_app = app.handle().clone();
            let load_models = models.clone();
            let watch_gate = gate.clone();
            let watch_app = app.handle().clone();
            let retrieve_store = store.clone();
            let retrieve_bus = bus.clone();
            let registry_listen = workers.clone();

            app.manage(services.clone());
            app.manage(dispatcher.clone());
            app.manage(workers);
            app.manage(gate);
            app.manage(models);
            app.manage(store.clone());
            app.manage(bus.clone());
            app.manage(graph_ui);
            app.manage(memory.clone());

            // Graph UI port — settings'ten MemoryBridge config'e yükle (process-global yok).
            let port_store = store.clone();
            let port_bridge = memory.clone();
            tauri::async_runtime::spawn(async move {
                let port = load_port_from_store(&port_store).await;
                port_bridge.set_http_port(port);
                log::info!("graph UI port={port}");
            });

            // MCP HTTP — Cursor/Claude stdio shim buraya proxy eder (dashboard sync).
            let mcp_store = store.clone();
            let mcp_nats = bus.nats_url().to_string();
            let mcp_workers = registry_listen.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(err) =
                    bridge::mcp_http::serve(mcp_store, mcp_nats, Some(mcp_workers)).await
                {
                    log::warn!("MCP HTTP durdu: {err}");
                }
            });

            tauri::async_runtime::spawn(async move {
                let mut meter = InferMeter::new();
                while let Some(result) = decision_rx.recv().await {
                    let msg_per_min = meter.record();
                    log::info!(
                        "DecisionGate {} routing={:?} security={:?} hit={:.2} {:.1}ms {}",
                        result.message_id,
                        result.routing.value,
                        result.security.value,
                        result.knowledge_hit,
                        result.latency_ms(),
                        result.device
                    );
                    let telemetry = LoungeTelemetry::from_decision(&result, msg_per_min);
                    if let Err(err) = retrieve_bus.publish(&telemetry.envelope()).await {
                        log::warn!("DecisionGate telemetry: {err}");
                    }
                    match inject_knowledge_hit(&retrieve_store, &retrieve_bus, &result).await {
                        Ok(Some(addon)) => {
                            if let Err(err) =
                                record_whisper_injection(&retrieve_store, &result, &addon).await
                            {
                                log::warn!("efficiency whisper counter: {err}");
                            }
                        }
                        Ok(None) => {}
                        Err(err) => log::warn!("Cross-Project Memory: {err}"),
                    }
                }
            });
            tauri::async_runtime::spawn(async move {
                run_laya_load(&load_app, &load_gate, &load_models).await;
            });
            spawn_laya_watchdog(watch_app, watch_gate);

            tauri::async_runtime::spawn(async move {
                {
                    let mut manager = services.lock().await;
                    let report = manager.ensure_all().await;
                    log::info!(
                        "bootstrap lmr={} nats={} memory={} plugin={}",
                        report.ollama.running,
                        report.nats.running,
                        report.memory.running,
                        report.plugin.running
                    );
                }
                spawn_quota_pump(quota_handle, quota_services, quota_store);
                spawn_supervisor(supervisor_handle, supervisor_services);
                spawn_event_pump(handle);
                tauri::async_runtime::spawn(async move { bus.run().await });
                let workflow_listen = workflow.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = workflow_listen.listen().await {
                        log::error!("workflow_engine durdu: {err}");
                    }
                });
                tauri::async_runtime::spawn(async move {
                    if let Err(err) = registry_listen.listen("nats://127.0.0.1:4222").await {
                        log::error!("worker_registry durdu: {err}");
                    }
                });
                if let Err(err) = dispatcher.listen().await {
                    log::error!("dispatcher durdu: {err}");
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ensure_services,
            service_status,
            index_workspace,
            get_dead_symbols,
            get_semantic_map,
            get_kernel_model,
            set_kernel_model,
            list_ollama_models,
            get_decision_gate_status,
            enable_decision_gate,
            decline_decision_gate,
            get_laya_engine_status,
            ensure_laya_engine,
            get_device_profile,
            list_recommended_models,
            pull_lmr_model,
            list_experiences,
            search_experiences,
            search_index_nodes,
            trigger_grok_test,
            list_projects,
            list_quotas,
            get_quota_state,
            get_routing_policy,
            set_routing_policy,
            resolve_routing,
            pending_approvals,
            record_whisper_feedback,
            probe_bus,
            discover_system,
            get_discovery_report,
            save_connected_tools,
            save_selected_tools,
            list_connected_tools,
            agent_efficiency_report,
            get_graph_ui_status,
            open_graph_ui,
            enable_graph_ui_cmd,
            get_graph_ui_port,
            set_graph_ui_port,
            write_text_file
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| match &event {
            RunEvent::WindowEvent { label, event, .. } if label == "main" => {
                if matches!(
                    event,
                    WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed
                ) {
                    if let Some(state) = app_handle.try_state::<GraphUiState>() {
                        on_main_window_closed(app_handle, state.inner());
                    } else if let Some(window) = app_handle.get_webview_window(GRAPH_WINDOW_LABEL) {
                        let _ = window.destroy();
                    }
                }
            }
            RunEvent::Exit | RunEvent::ExitRequested { .. } => {
                if let Some(state) = app_handle.try_state::<GraphUiState>() {
                    state.kill_spawned_child();
                }
            }
            _ => {}
        });
}

#[tauri::command]
fn write_text_file(path: String, contents: String) -> Result<(), String> {
    let target = PathBuf::from(&path);
    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
    }
    std::fs::write(&target, contents).map_err(|err| err.to_string())
}

#[tauri::command]
async fn ensure_services(state: tauri::State<'_, SharedServices>) -> Result<ServiceReport, String> {
    let mut manager = state.lock().await;
    Ok(manager.ensure_all().await)
}

#[tauri::command]
async fn service_status(state: tauri::State<'_, SharedServices>) -> Result<ServiceReport, String> {
    let manager = state.lock().await;
    Ok(manager.snapshot().await)
}

#[tauri::command]
async fn index_workspace(
    services: tauri::State<'_, SharedServices>,
    store: tauri::State<'_, ExperienceStore>,
    path: String,
) -> Result<IndexSnapshot, String> {
    if path.trim().is_empty() {
        return Err("path boş".into());
    }
    let bridge = {
        let manager = services.lock().await;
        manager.memory().clone()
    };
    let graph = bridge
        .index_workspace(path)
        .await
        .map_err(|err| err.to_string())?;
    let snapshot = store
        .save_project_index(graph.clone())
        .await
        .map_err(|err| err.to_string())?;
    if !snapshot.project.is_empty() {
        if let Err(err) =
            record_dead_snapshot(&store, snapshot.project.clone(), snapshot.dead).await
        {
            log::warn!("dead snapshot: {err}");
        }
    }
    Ok(snapshot)
}

#[tauri::command]
async fn get_dead_symbols(
    store: tauri::State<'_, ExperienceStore>,
    project_id: Option<String>,
) -> Result<Vec<DeadSymbol>, String> {
    let symbols = store
        .list_dead_symbols(project_id.clone())
        .await
        .map_err(|err| err.to_string())?;
    if let Some(pid) = project_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Err(err) = record_dead_snapshot(&store, pid, symbols.len() as u64).await {
            log::warn!("dead snapshot: {err}");
        }
    }
    Ok(symbols)
}

#[tauri::command]
async fn agent_efficiency_report(
    store: tauri::State<'_, ExperienceStore>,
    weekly: Option<bool>,
    project_id: Option<String>,
) -> Result<AgentEfficiencyReport, String> {
    let query = EfficiencyReportQuery {
        weekly: weekly.unwrap_or(true),
        project_id: project_id
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    };
    build_agent_efficiency_report(&store, query)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn get_semantic_map(
    store: tauri::State<'_, ExperienceStore>,
    project_id: Option<String>,
) -> Result<SemanticMap, String> {
    store
        .load_semantic_map(project_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn get_kernel_model(state: tauri::State<'_, Dispatcher>) -> Result<String, String> {
    Ok(state.selected_model().await)
}

#[tauri::command]
async fn set_kernel_model(
    state: tauri::State<'_, Dispatcher>,
    model: String,
) -> Result<String, String> {
    let model = model.trim().to_string();
    if model.is_empty() {
        return Err("model boş olamaz".into());
    }
    state.set_model(model.clone()).await;
    Ok(model)
}

#[tauri::command]
async fn list_ollama_models(
    state: tauri::State<'_, SharedServices>,
) -> Result<Vec<String>, String> {
    let mut manager = state.lock().await;
    let _ = manager.ensure_all().await;
    manager.ollama_models().await.map_err(|err| err.to_string())
}

#[tauri::command]
fn get_decision_gate_status(
    gate: tauri::State<'_, DecisionGate>,
) -> Result<DecisionGateStatus, String> {
    Ok(gate.status())
}

#[tauri::command]
fn get_laya_engine_status(
    models: tauri::State<'_, ModelManager>,
) -> Result<LayaEngineStatus, String> {
    Ok(models.status())
}

#[tauri::command]
async fn ensure_laya_engine(
    app: tauri::AppHandle,
    models: tauri::State<'_, ModelManager>,
) -> Result<LayaEngineStatus, String> {
    let models = models.inner().clone();
    let app_clone = app.clone();
    tokio::task::spawn_blocking(move || models.ensure(Some(&app_clone)))
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn enable_decision_gate(
    app: tauri::AppHandle,
    gate: tauri::State<'_, DecisionGate>,
    models: tauri::State<'_, ModelManager>,
) -> Result<DecisionGateStatus, String> {
    run_laya_load(&app, &gate, &models).await;
    Ok(gate.status())
}

#[tauri::command]
fn decline_decision_gate(
    app: tauri::AppHandle,
    gate: tauri::State<'_, DecisionGate>,
) -> Result<DecisionGateStatus, String> {
    gate.decline_offer();
    let status = gate.status();
    emit_decision_gate(&app, &status);
    Ok(status)
}

async fn run_laya_load(app: &tauri::AppHandle, gate: &DecisionGate, models: &ModelManager) {
    // Ağırlık indirmesi RAM yüklemesinden bağımsızdır; Worker Fleet Downloading/Ready görür.
    let models_for_files = models.clone();
    let app_for_files = app.clone();
    let engine = tokio::task::spawn_blocking(move || models_for_files.ensure(Some(&app_for_files)))
        .await
        .unwrap_or_else(|err| {
            LayaEngineStatus::failed(&kernel::decision_engine::laya_dir(), err.to_string())
        });
    emit_laya_engine(app, &engine);

    if !gate.request_enable() {
        emit_decision_gate(app, &gate.status());
        return;
    }
    emit_decision_gate(app, &gate.status());

    let available = tokio::task::spawn_blocking(kernel::decision_engine::available_memory_bytes)
        .await
        .unwrap_or(0);
    if !kernel::decision_engine::ram_is_sufficient(available) {
        gate.fail_load(&kernel::decision_engine::format_ram_blocked(available));
        if let Some(dispatcher) = app.try_state::<Dispatcher>() {
            dispatcher.set_gate_ready(false);
        }
        emit_decision_gate(app, &gate.status());
        return;
    }

    if !engine.is_ready() {
        let detail = engine
            .error
            .clone()
            .unwrap_or_else(|| "Laya ağırlıkları yok".into());
        gate.fail_load(&detail);
        if let Some(dispatcher) = app.try_state::<Dispatcher>() {
            dispatcher.set_gate_ready(false);
        }
        emit_decision_gate(app, &gate.status());
        return;
    }
    let dir = PathBuf::from(&engine.path);
    match tokio::task::spawn_blocking(move || kernel::decision_engine::load_session(&dir)).await {
        Ok(Ok(session)) => {
            gate.install(session);
            if let Some(dispatcher) = app.try_state::<Dispatcher>() {
                dispatcher.set_gate_ready(true);
            }
        }
        Ok(Err(err)) => {
            gate.fail_load(&err.to_string());
            if let Some(dispatcher) = app.try_state::<Dispatcher>() {
                dispatcher.set_gate_ready(false);
            }
        }
        Err(err) => {
            gate.fail_load(&err.to_string());
            if let Some(dispatcher) = app.try_state::<Dispatcher>() {
                dispatcher.set_gate_ready(false);
            }
        }
    }
    emit_decision_gate(app, &gate.status());
}

fn spawn_laya_watchdog(app: tauri::AppHandle, gate: DecisionGate) {
    tauri::async_runtime::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(15));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            let status = gate.status();
            if matches!(
                status.phase,
                DecisionGatePhase::Ready | DecisionGatePhase::Loading
            ) {
                continue;
            }
            if !status.ram_blocked() {
                continue;
            }
            let available =
                tokio::task::spawn_blocking(kernel::decision_engine::available_memory_bytes)
                    .await
                    .unwrap_or(0);
            let ram_ok = kernel::decision_engine::ram_is_sufficient(available);
            if gate.declined() {
                if !ram_ok {
                    gate.clear_declined();
                }
                continue;
            }
            if ram_ok {
                if gate.mark_available() {
                    emit_decision_gate(&app, &gate.status());
                }
            } else if status.phase == DecisionGatePhase::Available {
                gate.fail_load(&kernel::decision_engine::format_ram_blocked(available));
                emit_decision_gate(&app, &gate.status());
            }
        }
    });
}

fn emit_decision_gate(app: &tauri::AppHandle, status: &DecisionGateStatus) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(err) = window.emit(DECISION_GATE_EVENT, status) {
            log::debug!("{DECISION_GATE_EVENT} window emit: {err}");
        }
        return;
    }
    if let Err(err) = app.emit(DECISION_GATE_EVENT, status) {
        log::debug!("{DECISION_GATE_EVENT} app emit: {err}");
    }
}

fn emit_laya_engine(app: &tauri::AppHandle, status: &LayaEngineStatus) {
    let nats_url = app
        .try_state::<BusManager>()
        .map(|bus| bus.nats_url().to_string());
    services::model_manager::emit_engine(Some(app), nats_url.as_deref(), status);
}

#[tauri::command]
fn get_device_profile() -> DeviceProfile {
    services::hardware::device_profile()
}

#[tauri::command]
async fn list_recommended_models(
    state: tauri::State<'_, SharedServices>,
) -> Result<RecommendedModels, String> {
    let device = services::hardware::device_profile();
    let installed = {
        let mut manager = state.lock().await;
        let _ = manager.ensure_all().await;
        manager.ollama_models().await.unwrap_or_default()
    };
    services::hf_catalog::list_recommended(device, &installed)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn pull_lmr_model(
    app: tauri::AppHandle,
    services: tauri::State<'_, SharedServices>,
    dispatcher: tauri::State<'_, Dispatcher>,
    hf_id: String,
) -> Result<String, String> {
    let name = {
        let mut manager = services.lock().await;
        manager
            .pull_hf_model(&app, &hf_id)
            .await
            .map_err(|err| err.to_string())?
    };
    dispatcher.set_model(name.clone()).await;
    Ok(name)
}

#[tauri::command]
async fn list_experiences(
    state: tauri::State<'_, ExperienceStore>,
    limit: Option<usize>,
) -> Result<Vec<LoungeExperience>, String> {
    state
        .latest(limit.unwrap_or(20))
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn search_experiences(
    state: tauri::State<'_, ExperienceStore>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<LoungeExperience>, String> {
    state
        .search_experiences(query, limit)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn search_index_nodes(
    state: tauri::State<'_, ExperienceStore>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<AstNode>, String> {
    state
        .search_index_nodes(query, limit)
        .await
        .map_err(|err| err.to_string())
}

#[derive(serde::Serialize)]
struct GrokTestResult {
    task_id: String,
    subject: String,
    target_agent: String,
    chain_label: String,
}

/// Command Palette: Grok Bot varsa `lounge.task.requested` → grok_bot (workflow ile aynı yol).
#[tauri::command]
#[allow(deprecated)]
async fn trigger_grok_test(
    bus: tauri::State<'_, BusManager>,
    project_id: Option<String>,
) -> Result<GrokTestResult, String> {
    use kernel::workflow_engine::{grok_bot_in_fleet, GROK_BOT_WORKER};

    if !grok_bot_in_fleet() {
        return Err("Grok Bot fleet'te yok — test tetiklenemedi".into());
    }

    let project = project_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "agent-lounge-os".into());

    let mut task = LoungeTask::new("command_palette", &project, "Manual Grok Test");
    task.kind = TaskKind::Test;
    task.target_agent = Some(GROK_BOT_WORKER.into());
    let chain = format!("{project} -> Triggered Manual Grok Test");
    task.workflow_chain = Some(chain.clone());

    let url = bus.nats_url().to_string();
    let subject = TASK_REQUESTED.to_string();
    let bytes = serde_json::to_vec(&task).map_err(|err| err.to_string())?;
    let subject_err = subject.clone();
    tokio::task::spawn_blocking(move || {
        let nc = nats::connect(&url).map_err(|err| err.to_string())?;
        nc.publish(&subject, bytes).map_err(|err| err.to_string())
    })
    .await
    .map_err(|err| err.to_string())?
    .map_err(|err| format!("NATS publish başarısız ({subject_err}): {err}"))?;

    Ok(GrokTestResult {
        task_id: task.id,
        subject: TASK_REQUESTED.into(),
        target_agent: GROK_BOT_WORKER.into(),
        chain_label: chain,
    })
}

#[tauri::command]
async fn list_projects(
    services: tauri::State<'_, SharedServices>,
    store: tauri::State<'_, ExperienceStore>,
) -> Result<Vec<ProjectSummary>, String> {
    let indexed = store
        .list_indexed_projects()
        .await
        .map_err(|err| err.to_string())?;
    let discovered = {
        let bridge = {
            let manager = services.lock().await;
            manager.memory().clone()
        };
        bridge.list_projects().await.unwrap_or_default()
    };
    Ok(merge_project_summaries(indexed, discovered))
}

#[tauri::command]
async fn list_quotas(
    services: tauri::State<'_, SharedServices>,
    store: tauri::State<'_, ExperienceStore>,
) -> Result<Vec<ToolQuota>, String> {
    Ok(get_quota_state(services, store).await?.quotas)
}

#[tauri::command]
async fn get_quota_state(
    services: tauri::State<'_, SharedServices>,
    store: tauri::State<'_, ExperienceStore>,
) -> Result<QuotaState, String> {
    let (ollama, nats_monitor, memory) = {
        let manager = services.lock().await;
        (
            manager.ollama_endpoint(),
            manager.nats_monitor_url(),
            manager.memory().clone(),
        )
    };
    let keys = api_keys_from_store(&store).await;
    Ok(collect_quota_state_with_keys(&ollama, &nats_monitor, &memory, &keys).await)
}

#[tauri::command]
async fn get_routing_policy(state: tauri::State<'_, Dispatcher>) -> Result<RoutingPolicy, String> {
    state.routing_policy().await.map_err(|err| err.to_string())
}

#[tauri::command]
async fn set_routing_policy(
    state: tauri::State<'_, Dispatcher>,
    policy: RoutingPolicy,
) -> Result<RoutingPolicy, String> {
    state
        .set_routing_policy(policy)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn resolve_routing(
    state: tauri::State<'_, Dispatcher>,
    task_id: String,
    vote: RoutingVote,
) -> Result<(), String> {
    // Senkron komut: async runtime'daki await_approval ile tokio::Mutex üzerinden
    // kilitlenmeden oneshot'a hemen ulaşır (Grand Test hata #8).
    log::info!("resolve_routing task_id={task_id} vote={vote:?}");
    state
        .resolve_vote(task_id, vote)
        .map_err(|err| err.to_string())
}

/// Bekleyen onayları UI'ye verir (listener yarışı / sayfa dönüşü rehydrate — Madde 8 hipotez a).
#[tauri::command]
fn pending_approvals(
    state: tauri::State<'_, Dispatcher>,
) -> Result<Vec<crate::models::ApprovalRequest>, String> {
    Ok(state.pending_approvals())
}

/// Context Whisper tecrübesini kullanıcı "faydalı" bulduğunda Feedback Loop kaydı.
#[tauri::command]
async fn record_whisper_feedback(
    store: tauri::State<'_, ExperienceStore>,
    experience_id: String,
    project_id: Option<String>,
    task_id: Option<String>,
) -> Result<(), String> {
    if experience_id.trim().is_empty() {
        return Err("experience_id gerekli".into());
    }
    let store = store.inner().clone();
    tokio::task::spawn_blocking(move || {
        db::feedback::with_conn_record_whisper(
            &store,
            &experience_id,
            project_id.as_deref(),
            task_id.as_deref(),
        )
    })
    .await
    .map_err(|err| err.to_string())?
    .map_err(|err| err.to_string())?;
    Ok(())
}

#[tauri::command]
async fn probe_bus(state: tauri::State<'_, BusManager>) -> Result<LoungeMessage, String> {
    state.probe().await.map_err(|err| err.to_string())
}

#[tauri::command]
async fn discover_system(
    state: tauri::State<'_, SharedServices>,
) -> Result<DiscoveryReport, String> {
    get_discovery_report(state).await
}

#[tauri::command]
async fn get_discovery_report(
    state: tauri::State<'_, SharedServices>,
) -> Result<DiscoveryReport, String> {
    let (lounge_endpoint, system_endpoint) = {
        let mut manager = state.lock().await;
        let report = manager.ensure_all().await;
        if !report.ollama.running {
            log::warn!(
                "keşif öncesi LMR ayakta değil: {}",
                report.ollama.error.as_deref().unwrap_or("yanıt yok")
            );
        }
        (
            manager.ollama_endpoint(),
            services::system_ollama_endpoint(),
        )
    };
    discovery_report(workspace_root(), lounge_endpoint, system_endpoint)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn save_connected_tools(
    state: tauri::State<'_, ExperienceStore>,
    tools: Vec<DiscoveredTool>,
) -> Result<Vec<ConnectedTool>, String> {
    save_selected_tools(state, tools).await
}

#[tauri::command]
async fn save_selected_tools(
    state: tauri::State<'_, ExperienceStore>,
    tools: Vec<DiscoveredTool>,
) -> Result<Vec<ConnectedTool>, String> {
    state
        .save_selected_tools(tools)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn list_connected_tools(
    state: tauri::State<'_, ExperienceStore>,
) -> Result<Vec<ConnectedTool>, String> {
    state
        .list_connected_tools()
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn get_graph_ui_status(
    services: tauri::State<'_, SharedServices>,
    graph: tauri::State<'_, GraphUiState>,
    project_root: Option<String>,
) -> Result<GraphUiStatus, String> {
    let bridge = {
        let manager = services.lock().await;
        manager.memory().clone()
    };
    Ok(graph_ui_status(graph.inner(), &bridge, project_root.as_deref()).await)
}

#[tauri::command]
async fn open_graph_ui(
    app: tauri::AppHandle,
    services: tauri::State<'_, SharedServices>,
    graph: tauri::State<'_, GraphUiState>,
    project_root: Option<String>,
) -> Result<(), String> {
    let bridge = {
        let manager = services.lock().await;
        manager.memory().clone()
    };
    let status = graph_ui_status(graph.inner(), &bridge, project_root.as_deref()).await;
    if !status.binary_found {
        return Err("codebase-memory-mcp bulunamadı".into());
    }
    if !status.ui_available {
        return Err("Graph UI kapalı — önce Enable Graph UI".into());
    }
    let Some(name) = status.cbm_project_name else {
        return Err("Index workspace first".into());
    };
    open_or_focus_graph_window(&app, graph.inner(), status.port, &name)
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn enable_graph_ui_cmd(
    app: tauri::AppHandle,
    services: tauri::State<'_, SharedServices>,
    graph: tauri::State<'_, GraphUiState>,
    project_root: Option<String>,
) -> Result<(), String> {
    let bridge = {
        let manager = services.lock().await;
        manager.memory().clone()
    };
    let status = graph_ui_status(graph.inner(), &bridge, project_root.as_deref()).await;
    if !status.binary_found {
        return Err("codebase-memory-mcp bulunamadı".into());
    }
    if status.port_conflict {
        return Err(status
            .conflict_message
            .unwrap_or_else(|| format!("Port {} meşgul", status.port)));
    }
    enable_graph_ui(
        &app,
        graph.inner(),
        &bridge,
        status.cbm_project_name.as_deref(),
    )
    .await
    .map_err(|err| err.to_string())
}

#[tauri::command]
async fn get_graph_ui_port(
    store: tauri::State<'_, ExperienceStore>,
    services: tauri::State<'_, SharedServices>,
) -> Result<u16, String> {
    let from_store = load_port_from_store(store.inner()).await;
    let manager = services.lock().await;
    let live = manager.memory().http_port();
    Ok(if live > 0 { live } else { from_store })
}

#[tauri::command]
async fn set_graph_ui_port(
    app: tauri::AppHandle,
    store: tauri::State<'_, ExperienceStore>,
    services: tauri::State<'_, SharedServices>,
    graph: tauri::State<'_, GraphUiState>,
    port: u16,
) -> Result<u16, String> {
    if port == 0 {
        return Err("port 0 geçersiz".into());
    }
    let bridge = {
        let manager = services.lock().await;
        manager.memory().clone()
    };
    persist_port(store.inner(), &bridge, &app, graph.inner(), port)
        .await
        .map_err(|err| err.to_string())?;
    Ok(port)
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
