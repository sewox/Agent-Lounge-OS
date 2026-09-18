pub mod db;
pub mod infra;
pub mod kernel;
pub mod models;
pub mod services;

use std::path::PathBuf;

use db::ExperienceStore;
use infra::{listen_lounge_wildcard, probe_quotas};
use kernel::{default_model_lock, Dispatcher};
use models::{
    IndexSnapshot, LoungeExperience, ProjectSummary, RoutingPolicy, RoutingVote, ServiceReport,
    ToolQuota,
};
use services::{MemoryBridge, ServiceManager, SharedServices};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
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
            let model = default_model_lock();
            let services = ServiceManager::shared();
            let memory = MemoryBridge::discover().unwrap_or_else(|err| {
                log::warn!("{err}");
                MemoryBridge::from_binary("codebase-memory-mcp")
            });
            let dispatcher = Dispatcher::new(
                "nats://127.0.0.1:4222",
                "http://127.0.0.1:11434",
                model,
                memory,
                store.clone(),
                workspace,
            );
            dispatcher.attach_app(app.handle().clone());

            app.manage(services.clone());
            app.manage(dispatcher.clone());
            app.manage(store);

            let hub_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                {
                    let mut manager = services.lock().await;
                    let report = manager.ensure_all().await;
                    log::info!(
                        "bootstrap ollama={} nats={} memory={}",
                        report.ollama.running,
                        report.nats.running,
                        report.memory.running
                    );
                }
                tauri::async_runtime::spawn(listen_lounge_wildcard(
                    hub_app,
                    "nats://127.0.0.1:4222".into(),
                ));
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
            get_kernel_model,
            set_kernel_model,
            list_ollama_models,
            list_experiences,
            list_projects,
            list_quotas,
            get_routing_policy,
            set_routing_policy,
            resolve_routing
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
    state: tauri::State<'_, SharedServices>,
    repo_path: Option<String>,
) -> Result<IndexSnapshot, String> {
    let bridge = {
        let manager = state.lock().await;
        manager.memory().clone()
    };

    let path = repo_path.map(PathBuf::from).unwrap_or_else(workspace_root);
    bridge
        .index_repository(path)
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
    let manager = state.lock().await;
    manager.ollama_models().await.map_err(|err| err.to_string())
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
    state: tauri::State<'_, SharedServices>,
) -> Result<Vec<ProjectSummary>, String> {
    let bridge = {
        let manager = state.lock().await;
        manager.memory().clone()
    };
    bridge.list_projects().await.map_err(|err| err.to_string())
}

#[tauri::command]
async fn list_quotas(state: tauri::State<'_, SharedServices>) -> Result<Vec<ToolQuota>, String> {
    let (ollama, nats_monitor, memory) = {
        let manager = state.lock().await;
        (
            manager.ollama_endpoint(),
            "http://127.0.0.1:8222".to_string(),
            manager.memory().clone(),
        )
    };
    Ok(probe_quotas(&ollama, &nats_monitor, &memory).await)
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

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
