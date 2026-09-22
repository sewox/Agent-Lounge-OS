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
    Dispatcher, InferMeter, LoungeTelemetry, DECISION_GATE_EVENT,
};
use lounge_protocol::LoungeMessage;
use models::{
    merge_project_summaries, ConnectedTool, DeadSymbol, DeviceProfile, DiscoveredTool,
    DiscoveryReport, IndexSnapshot, LoungeExperience, ProjectSummary, QuotaState,
    RecommendedModels, RoutingPolicy, RoutingVote, SemanticMap, ServiceReport, ToolQuota,
};
use services::autodiscover::discovery_report;
use services::{
    api_keys_from_store, collect_quota_state_with_keys, spawn_event_pump, spawn_quota_pump,
    spawn_supervisor, LayaEngineStatus, MemoryBridge, ModelManager, ServiceManager, SharedServices,
};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

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
            let services = ServiceManager::shared();
            let memory = MemoryBridge::discover().unwrap_or_else(|err| {
                log::warn!("{err}");
                MemoryBridge::from_binary("codebase-memory-mcp")
            });
            let (decision_tx, mut decision_rx) = tokio::sync::mpsc::channel(64);
            let gate = DecisionGate::new(decision_tx);
            let dispatcher = Dispatcher::new(
                "nats://127.0.0.1:4222",
                services::lounge_ollama_endpoint(),
                model,
                memory,
                store.clone(),
                workspace,
            )
            .with_decision_cache(gate.cache());
            dispatcher.attach_app(app.handle().clone());
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

            app.manage(services.clone());
            app.manage(dispatcher.clone());
            app.manage(gate);
            app.manage(models);
            app.manage(store);
            app.manage(bus.clone());

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
                    if let Err(err) =
                        inject_knowledge_hit(&retrieve_store, &retrieve_bus, &result).await
                    {
                        log::warn!("Cross-Project Memory: {err}");
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
            list_projects,
            list_quotas,
            get_quota_state,
            get_routing_policy,
            set_routing_policy,
            resolve_routing,
            probe_bus,
            discover_system,
            get_discovery_report,
            save_connected_tools,
            save_selected_tools,
            list_connected_tools
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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
    store
        .save_project_index(graph.clone())
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn get_dead_symbols(
    store: tauri::State<'_, ExperienceStore>,
    project_id: Option<String>,
) -> Result<Vec<DeadSymbol>, String> {
    store
        .list_dead_symbols(project_id)
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
        emit_decision_gate(app, &gate.status());
        return;
    }

    let models = models.clone();
    let app_for_files = app.clone();
    let engine = tokio::task::spawn_blocking(move || models.ensure(Some(&app_for_files)))
        .await
        .unwrap_or_else(|err| {
            LayaEngineStatus::failed(&kernel::decision_engine::laya_dir(), err.to_string())
        });
    emit_laya_engine(app, &engine);
    if !engine.is_ready() {
        let detail = engine
            .error
            .clone()
            .unwrap_or_else(|| "Laya ağırlıkları yok".into());
        gate.fail_load(&detail);
        emit_decision_gate(app, &gate.status());
        return;
    }
    let dir = PathBuf::from(&engine.path);
    match tokio::task::spawn_blocking(move || kernel::decision_engine::load_session(&dir)).await {
        Ok(Ok(session)) => gate.install(session),
        Ok(Err(err)) => gate.fail_load(&err.to_string()),
        Err(err) => gate.fail_load(&err.to_string()),
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
async fn resolve_routing(
    state: tauri::State<'_, Dispatcher>,
    task_id: String,
    vote: RoutingVote,
) -> Result<(), String> {
    state
        .resolve_vote(task_id, vote)
        .await
        .map_err(|err| err.to_string())
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

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
