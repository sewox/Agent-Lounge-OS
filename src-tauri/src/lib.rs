pub mod db;
pub mod infra;
pub mod kernel;
pub mod models;
pub mod services;

use std::path::PathBuf;

use db::ExperienceStore;
use infra::BusManager;
use kernel::{default_model_lock, Dispatcher};
use lounge_protocol::LoungeMessage;
use models::{
    merge_project_summaries, ConnectedTool, DeadSymbol, DeviceProfile, DiscoveredTool,
    DiscoveryReport, IndexSnapshot, LoungeExperience, ProjectSummary, QuotaState,
    RecommendedModels, RoutingPolicy, RoutingVote, SemanticMap, ServiceReport, ToolQuota,
};
use services::autodiscover::discovery_report;
use services::{
    api_keys_from_store, collect_quota_state_with_keys, spawn_event_pump, spawn_quota_pump,
    spawn_supervisor, MemoryBridge, ServiceManager, SharedServices,
};
use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

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
            let dispatcher = Dispatcher::new(
                "nats://127.0.0.1:4222",
                services::lounge_ollama_endpoint(),
                model,
                memory,
                store.clone(),
                workspace,
            );
            dispatcher.attach_app(app.handle().clone());
            let bus = BusManager::new("nats://127.0.0.1:4222");
            let handle = app.handle().clone();
            let supervisor_handle = app.handle().clone();
            let supervisor_services = services.clone();
            let quota_handle = app.handle().clone();
            let quota_services = services.clone();
            let quota_store = store.clone();

            app.manage(services.clone());
            app.manage(dispatcher.clone());
            app.manage(store);
            app.manage(bus.clone());

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
