pub mod bridge;
pub mod db;
pub mod infra;
pub mod kernel;
pub mod markdown_export;
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
    DiscoveryReport, IndexSnapshot, LoungeExperience, LoungeTask, ProjectPageList, ProjectSummary,
    QuotaState, RecommendedModels, RoutingPolicy, RoutingVote, SemanticMap, ServiceReport,
    TaskKind, ToolQuota, VaultProjectAggregate, TASK_REQUESTED,
};
use services::autodiscover::discovery_report;
use services::{
    api_keys_from_store, apply_placement_show, build_agent_efficiency_report,
    collect_quota_state_with_keys, data_root, enable_graph_ui,
    fix_dead_symbol_with_agent as publish_fix_dead_symbol, flush_session, graph_ui_status,
    handle_window_event, load_port_from_store, load_port_preference_from_store,
    on_main_window_closed, open_dead_symbol_in_editor as open_indexed_dead_symbol,
    open_or_focus_graph_window, open_path_in_editor, persist_port, persist_port_preference,
    placement_for_window, record_dead_snapshot, record_whisper_injection,
    resolve_data_root_for_app, spawn_auto_archive, spawn_event_pump, spawn_quota_pump,
    spawn_supervisor, AgentEfficiencyReport, EfficiencyReportQuery, FixDeadSymbolResult,
    GeometrySession, GraphUiPortMode, GraphUiState, GraphUiStatus, IndexJob, IndexProgress,
    IndexQueue, LayaEngineStatus, MemoryBridge, ModelManager, ServiceManager, SharedServices,
    WorkspaceScanResult, GRAPH_WINDOW_LABEL, MAIN_WINDOW_LABEL,
};
use tauri::{Emitter, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::DialogExt;

const ONBOARDING_ROUTE: &str = "/onboarding";
const DASHBOARD_ROUTE: &str = "/dashboard";

/// Kernel file-log rotation: larger than plugin defaults (~40 KB / KeepOne).
fn kernel_log_settings() -> (u128, tauri_plugin_log::RotationStrategy) {
    const MAX_FILE_SIZE: u128 = 5 * 1024 * 1024;
    const KEEP_COUNT: usize = 5;
    (
        MAX_FILE_SIZE,
        tauri_plugin_log::RotationStrategy::KeepSome(KEEP_COUNT),
    )
}

/// `connected_tools` boşsa onboarding, doluysa dashboard.
pub fn initial_window_route() -> &'static str {
    match ExperienceStore::open(db::default_db_path(data_root())) {
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

fn open_main_window(
    app: &tauri::App,
    start_route: &str,
    data_root: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut window_config = app
        .config()
        .app
        .windows
        .first()
        .cloned()
        .ok_or("main window config missing")?;
    window_config.url = WebviewUrl::App(PathBuf::from(start_route.trim_start_matches('/')));
    let placement = placement_for_window(app.handle(), MAIN_WINDOW_LABEL, data_root);
    window_config.width = placement.geometry.width;
    window_config.height = placement.geometry.height;
    window_config.min_width = Some(placement.min_width);
    window_config.min_height = Some(placement.min_height);
    window_config.maximized = false;
    window_config.visible = false;
    if placement.center {
        window_config.center = true;
        window_config.x = None;
        window_config.y = None;
    } else {
        // Physical position applied after build (mixed-DPI safe); skip logical x/y.
        window_config.center = false;
        window_config.x = None;
        window_config.y = None;
    }
    let window = WebviewWindowBuilder::from_config(app.handle(), &window_config)?.build()?;
    apply_placement_show(&window, &placement)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    run_with_start_route(initial_window_route());
}

pub fn run_with_start_route(start_route: &'static str) {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .setup(move |app| {
            let workspace = resolve_data_root_for_app(app.handle()).map_err(|err| {
                format!("uygulama veri dizini hazırlanamadı (LOUNGE_DATA_DIR veya app data): {err}")
            })?;
            let log_dir = workspace.join("logs");
            std::fs::create_dir_all(&log_dir).map_err(|err| {
                format!("log dizini oluşturulamadı ({}): {err}", log_dir.display())
            })?;
            // Always persist kernel/supervisor logs under the data dir so empty
            // redirected stdout/stderr still leaves a diagnosable file.
            // Plugin defaults are ~40 KB + KeepOne — too small for diagnostics.
            let (max_file_size, rotation_strategy) = kernel_log_settings();
            app.handle().plugin(
                tauri_plugin_log::Builder::new()
                    .level(log::LevelFilter::Info)
                    .max_file_size(max_file_size)
                    .rotation_strategy(rotation_strategy)
                    .targets([
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                        tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Folder {
                            path: log_dir.clone(),
                            file_name: Some("kernel".into()),
                        }),
                    ])
                    .build(),
            )?;
            log::info!("kernel log → {}", log_dir.join("kernel.log").display());

            let store = ExperienceStore::open(db::default_db_path(&workspace)).map_err(|err| {
                format!(
                    "experience veritabanı açılamadı ({}): {err}",
                    db::default_db_path(&workspace).display()
                )
            })?;
            let geometry_session = GeometrySession::new(workspace.clone());
            app.manage(geometry_session);
            open_main_window(app, start_route, &workspace)?;
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
            services::install_destructive_approval_emitter(
                app.handle().clone(),
                "nats://127.0.0.1:4222".into(),
            );
            let (trusted_tx, trusted_rx) = tokio::sync::mpsc::channel(64);
            let workflow = WorkflowEngine::new("nats://127.0.0.1:4222")
                .with_trusted_ingress(trusted_tx)
                .with_store(store.clone());
            let bus = BusManager::new("nats://127.0.0.1:4222");
            let models = ModelManager::with_nats_url(bus.nats_url());
            let handle = app.handle().clone();
            let supervisor_handle = app.handle().clone();
            let supervisor_services = services.clone();
            let quota_handle = app.handle().clone();
            let quota_services = services.clone();
            let quota_store = store.clone();
            let auto_archive_store = store.clone();
            let load_gate = gate.clone();
            let load_app = app.handle().clone();
            let load_models = models.clone();
            let watch_gate = gate.clone();
            let watch_app = app.handle().clone();
            let retrieve_store = store.clone();
            let retrieve_bus = bus.clone();
            let registry_listen = workers.clone();

            let index_queue = IndexQueue::new(memory.clone(), store.clone());
            index_queue.ensure_worker(app.handle().clone());

            app.manage(services.clone());
            app.manage(dispatcher.clone());
            app.manage(workers);
            app.manage(gate);
            app.manage(models);
            app.manage(store.clone());
            app.manage(bus.clone());
            app.manage(graph_ui);
            app.manage(memory.clone());
            app.manage(index_queue);

            // Graph UI port — settings'ten MemoryBridge + mode yükle (process-global yok).
            // Also run one-time shared CBM config.json pollution migration.
            let port_store = store.clone();
            let port_bridge = memory.clone();
            let port_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let pref = load_port_preference_from_store(&port_store).await;
                port_bridge.set_http_port(pref.port);
                if let Some(state) = port_app.try_state::<GraphUiState>() {
                    state.set_port_mode(pref.mode);
                }
                log::info!("graph UI port={} mode={}", pref.port, pref.mode.as_str());
                if let Err(err) =
                    services::cbm_ui_config::run_startup_cbm_config_migration(&port_store).await
                {
                    log::warn!("cbm ui config migration: {err}");
                }
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
                        "bootstrap lmr={}({}) nats={}({}) memory={} plugin={}",
                        report.ollama.running,
                        report
                            .ollama
                            .availability
                            .as_deref()
                            .unwrap_or(if report.ollama.running { "up" } else { "down" }),
                        report.nats.running,
                        report
                            .nats
                            .availability
                            .as_deref()
                            .unwrap_or(if report.nats.running { "up" } else { "down" }),
                        report.memory.running,
                        report.plugin.running
                    );
                }
                spawn_quota_pump(quota_handle, quota_services, quota_store);
                spawn_supervisor(supervisor_handle, supervisor_services);
                spawn_auto_archive(auto_archive_store);
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
                // A2A zombi tarama + idempotency GC; trusted workflow girişi; worker lifecycle.
                dispatcher.spawn_trusted_ingress(trusted_rx);
                dispatcher.spawn_lifecycle_listener();
                dispatcher.spawn_control_stop_listener();
                dispatcher.spawn_task_ack_listener();
                dispatcher.spawn_silence_watchdog();
                if let Err(err) = dispatcher.listen().await {
                    log::error!("dispatcher durdu: {err}");
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ensure_services,
            service_status,
            get_runtime_paths,
            index_workspace,
            scan_workspace,
            list_index_jobs,
            cancel_index_job,
            cancel_all_index_jobs,
            get_dead_symbols,
            get_semantic_map,
            list_vault_projects,
            list_project_pages,
            list_file_symbols,
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
            get_experience,
            update_experience,
            archive_experience,
            unarchive_experience,
            pin_experience,
            mark_experience_reviewed,
            mark_all_experiences_reviewed,
            count_experiences,
            count_unreviewed_experiences,
            ignore_symbol,
            unignore_symbol,
            list_ignored_symbols,
            open_in_editor,
            open_dead_symbol_in_editor,
            fix_dead_symbol_with_agent,
            focus_app_for_approval,
            services::approval_sound::pick_custom_approval_sound,
            services::approval_sound::load_custom_approval_sound_data_url,
            confirm_destructive,
            reject_destructive,
            list_pending_destructive,
            get_editor_settings,
            set_editor_settings,
            test_editor_settings,
            get_experience_ttl_days,
            set_experience_ttl_days,
            get_experience_use_count_threshold,
            set_experience_use_count_threshold,
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
            list_agent_sessions,
            list_a2a_background_tasks,
            get_remote_access,
            register_remote_tunnel,
            remove_remote_tunnel,
            rotate_remote_token,
            agent_efficiency_report,
            get_graph_ui_status,
            open_graph_ui,
            enable_graph_ui_cmd,
            get_graph_ui_port,
            set_graph_ui_port,
            get_graph_ui_port_mode,
            set_graph_ui_port_mode,
            markdown_export::save_markdown_report
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| match &event {
            RunEvent::WindowEvent { label, event, .. }
                if label == MAIN_WINDOW_LABEL || label == GRAPH_WINDOW_LABEL =>
            {
                handle_window_event(app_handle, label, event);
                if label == MAIN_WINDOW_LABEL {
                    match event {
                        WindowEvent::CloseRequested { .. } => {
                            // Geometry flush is handled in handle_window_event (main+graph).
                            if let Some(state) = app_handle.try_state::<GraphUiState>() {
                                let bridge = app_handle.try_state::<MemoryBridge>();
                                on_main_window_closed(
                                    app_handle,
                                    state.inner(),
                                    bridge.as_ref().map(|b| b.inner()),
                                );
                            } else if let Some(window) =
                                app_handle.get_webview_window(GRAPH_WINDOW_LABEL)
                            {
                                let _ = window.destroy();
                            }
                        }
                        WindowEvent::Destroyed => {
                            if let Some(state) = app_handle.try_state::<GraphUiState>() {
                                let bridge = app_handle.try_state::<MemoryBridge>();
                                on_main_window_closed(
                                    app_handle,
                                    state.inner(),
                                    bridge.as_ref().map(|b| b.inner()),
                                );
                            }
                        }
                        // Desktop notification plugins do not deliver onAction. When the OS
                        // activates/focuses the app (toast click, Alt-Tab, taskbar), raise the
                        // pending-approval banner via focus_app_for_approval.
                        WindowEvent::Focused(true) => {
                            services::on_app_activated_for_pending_approval(app_handle);
                        }
                        _ => {}
                    }
                }
            }
            #[cfg(target_os = "macos")]
            RunEvent::Reopen { .. } => {
                services::on_app_activated_for_pending_approval(app_handle);
            }
            RunEvent::Exit | RunEvent::ExitRequested { .. } => {
                // Cmd+Q / Dock Quit may skip CloseRequested — flush cached geometry.
                flush_session(app_handle);
                if let Some(state) = app_handle.try_state::<GraphUiState>() {
                    state.kill_spawned_child();
                    state.restore_cbm_config_best_effort();
                }
            }
            _ => {}
        });
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

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct RuntimePaths {
    data_root: String,
    kernel_log: String,
    lmr_log: String,
    nats_log: String,
    lmr_dir: String,
    lmr_binary: String,
}

#[tauri::command]
fn get_runtime_paths() -> RuntimePaths {
    let root = services::data_root();
    let lmr_dir = services::lounge_lmr_dir();
    let nats_dir = services::lounge_nats_dir();
    RuntimePaths {
        data_root: root.display().to_string(),
        kernel_log: root.join("logs").join("kernel.log").display().to_string(),
        lmr_log: lmr_dir.join("serve.log").display().to_string(),
        nats_log: nats_dir.join("nats-server.log").display().to_string(),
        lmr_dir: lmr_dir.display().to_string(),
        lmr_binary: services::lounge_lmr_binary_path().display().to_string(),
    }
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

/// Çalışma alanını tara → projeleri keşfet/import et → arka plan indeks kuyruğuna al.
/// `path` verilmezse native klasör diyaloğu açılır. İndeksleme istek yaşam döngüsüne bağlı değildir.
#[tauri::command]
async fn scan_workspace(
    app: tauri::AppHandle,
    queue: tauri::State<'_, std::sync::Arc<IndexQueue>>,
    path: Option<String>,
) -> Result<WorkspaceScanResult, String> {
    let selected = match path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
        Some(p) => p,
        None => {
            let app_for_dialog = app.clone();
            let picked = tauri::async_runtime::spawn_blocking(move || {
                app_for_dialog
                    .dialog()
                    .file()
                    .set_title("Index Workspace")
                    .blocking_pick_folder()
            })
            .await
            .map_err(|err| format!("dialog failed: {err}"))?;
            let Some(folder) = picked else {
                return Err("cancelled".into());
            };
            folder
                .into_path()
                .map_err(|err| format!("invalid folder path: {err}"))?
                .to_string_lossy()
                .into_owned()
        }
    };

    match queue.scan_and_enqueue(&app, selected).await {
        Ok(result) => Ok(result),
        Err(kind) => Err(format!("{}: {}", kind.code(), kind.message())),
    }
}

#[tauri::command]
async fn list_index_jobs(
    queue: tauri::State<'_, std::sync::Arc<IndexQueue>>,
) -> Result<Vec<IndexJob>, String> {
    Ok(queue.list_jobs().await)
}

#[tauri::command]
async fn cancel_index_job(
    app: tauri::AppHandle,
    queue: tauri::State<'_, std::sync::Arc<IndexQueue>>,
    job_id: String,
) -> Result<bool, String> {
    Ok(queue.cancel_job(&app, job_id.trim()).await)
}

#[tauri::command]
async fn cancel_all_index_jobs(
    app: tauri::AppHandle,
    queue: tauri::State<'_, std::sync::Arc<IndexQueue>>,
) -> Result<IndexProgress, String> {
    queue.cancel_all(&app).await;
    Ok(queue.progress().await)
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
async fn list_vault_projects(
    store: tauri::State<'_, ExperienceStore>,
) -> Result<Vec<VaultProjectAggregate>, String> {
    store
        .list_vault_project_aggregates()
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn list_project_pages(
    store: tauri::State<'_, ExperienceStore>,
    project_id: String,
    query: Option<String>,
    sort: Option<String>,
    offset: Option<u64>,
    limit: Option<u64>,
) -> Result<ProjectPageList, String> {
    let project_id = project_id.trim().to_string();
    if project_id.is_empty() {
        return Err("project_id gerekli".into());
    }
    store
        .list_project_pages(project_id, query, sort, offset, limit)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn list_file_symbols(
    store: tauri::State<'_, ExperienceStore>,
    project_id: String,
    file_path: String,
) -> Result<Vec<crate::models::FileSymbol>, String> {
    let project_id = project_id.trim().to_string();
    let file_path = file_path.trim().to_string();
    if project_id.is_empty() {
        return Err("project_id gerekli".into());
    }
    if file_path.is_empty() {
        return Err("file_path gerekli".into());
    }
    store
        .list_file_symbols(project_id, file_path)
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
    include_archived: Option<bool>,
    offset: Option<usize>,
) -> Result<Vec<LoungeExperience>, String> {
    state
        .list_experiences_page(
            limit.unwrap_or(20),
            include_archived.unwrap_or(false),
            offset.unwrap_or(0),
        )
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn count_experiences(
    state: tauri::State<'_, ExperienceStore>,
    include_archived: Option<bool>,
) -> Result<u64, String> {
    state
        .count_experiences(include_archived.unwrap_or(false))
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn get_experience(
    state: tauri::State<'_, ExperienceStore>,
    id: String,
) -> Result<Option<LoungeExperience>, String> {
    state.get(id).await.map_err(|err| err.to_string())
}

#[tauri::command]
async fn update_experience(
    state: tauri::State<'_, ExperienceStore>,
    id: String,
    patch: db::ExperienceUpdate,
) -> Result<(), String> {
    state
        .update_experience(id, patch)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn archive_experience(
    state: tauri::State<'_, ExperienceStore>,
    id: String,
) -> Result<(), String> {
    state
        .archive_experience(id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn unarchive_experience(
    state: tauri::State<'_, ExperienceStore>,
    id: String,
) -> Result<(), String> {
    state
        .unarchive_experience(id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn pin_experience(
    state: tauri::State<'_, ExperienceStore>,
    id: String,
    pinned: bool,
) -> Result<(), String> {
    state
        .pin_experience(id, pinned)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn mark_experience_reviewed(
    state: tauri::State<'_, ExperienceStore>,
    id: String,
) -> Result<(), String> {
    state
        .mark_experience_reviewed(id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn mark_all_experiences_reviewed(
    state: tauri::State<'_, ExperienceStore>,
) -> Result<u64, String> {
    state
        .mark_all_experiences_reviewed()
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn count_unreviewed_experiences(
    state: tauri::State<'_, ExperienceStore>,
) -> Result<u64, String> {
    state
        .count_unreviewed_experiences()
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn ignore_symbol(
    state: tauri::State<'_, ExperienceStore>,
    symbol: DeadSymbol,
) -> Result<(), String> {
    state
        .ignore_symbol(symbol)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn unignore_symbol(
    state: tauri::State<'_, ExperienceStore>,
    symbol: DeadSymbol,
) -> Result<(), String> {
    state
        .unignore_symbol(symbol)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn list_ignored_symbols(
    state: tauri::State<'_, ExperienceStore>,
    project_id: Option<String>,
) -> Result<Vec<DeadSymbol>, String> {
    state
        .list_ignored_symbols(project_id)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn open_in_editor(
    store: tauri::State<'_, ExperienceStore>,
    path: String,
    line: Option<i64>,
    editor_command: Option<String>,
) -> Result<(), String> {
    open_path_in_editor(&store, path, line, editor_command)
        .await
        .map_err(|err| err.to_string())
}

/// Open a dead symbol using index-backed path (symbol identity only from webview).
#[tauri::command]
async fn open_dead_symbol_in_editor(
    store: tauri::State<'_, ExperienceStore>,
    symbol: DeadSymbol,
    editor_command: Option<String>,
) -> Result<(), String> {
    open_indexed_dead_symbol(&store, symbol, editor_command)
        .await
        .map_err(|err| err.to_string())
}

/// Queue a NATS cleanup task for a dead symbol (DS-07).
#[tauri::command]
async fn fix_dead_symbol_with_agent(
    store: tauri::State<'_, ExperienceStore>,
    bus: tauri::State<'_, BusManager>,
    symbol: DeadSymbol,
) -> Result<FixDeadSymbolResult, String> {
    publish_fix_dead_symbol(&store, bus.nats_url(), symbol)
        .await
        .map_err(|err| err.to_string())
}

/// Notification click / tray activate → focus main window + open approval banner.
#[tauri::command]
async fn focus_app_for_approval(
    app: tauri::AppHandle,
    task_id: Option<String>,
) -> Result<(), String> {
    services::focus_app_for_approval(&app, task_id.as_deref());
    Ok(())
}

/// One-shot confirm for a PolicyGate-blocked destructive command (F3).
/// `command_hash` is required and must match the pending token (UI/QA binding).
#[tauri::command]
async fn confirm_destructive(id: String, command_hash: Option<String>) -> Result<(), String> {
    kernel::validate_destructive_ipc_hash(&id, command_hash.as_deref())
        .map_err(|err| err.to_string())?;
    kernel::confirm_destructive(&id).map_err(|err| err.to_string())
}

/// Reject / dismiss a pending destructive confirmation (clears notification slot).
/// `command_hash` is required and must match the pending token (UI/QA binding).
#[tauri::command]
async fn reject_destructive(id: String, command_hash: Option<String>) -> Result<(), String> {
    kernel::validate_destructive_ipc_hash(&id, command_hash.as_deref())
        .map_err(|err| err.to_string())?;
    kernel::reject_destructive(&id).map_err(|err| err.to_string())
}

/// FIFO list of pending destructive confirmations for the UI queue.
#[tauri::command]
async fn list_pending_destructive() -> Result<Vec<kernel::DestructivePendingEvent>, String> {
    Ok(kernel::list_pending_destructive())
}

#[tauri::command]
async fn get_editor_settings(
    state: tauri::State<'_, ExperienceStore>,
) -> Result<services::open_editor::EditorSettings, String> {
    services::open_editor::load_editor_settings(&state)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn set_editor_settings(
    state: tauri::State<'_, ExperienceStore>,
    settings: services::open_editor::EditorSettings,
) -> Result<services::open_editor::EditorSettings, String> {
    services::open_editor::save_editor_settings(&state, &settings)
        .await
        .map_err(|err| err.to_string())
}

/// Open a harmless Rust-chosen path (app data dir) with the selected editor.
#[tauri::command]
async fn test_editor_settings(
    app: tauri::AppHandle,
    state: tauri::State<'_, ExperienceStore>,
    settings: services::open_editor::EditorSettings,
) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|err| err.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let path = dir.to_string_lossy().into_owned();
    services::open_editor::test_editor_open(&state, &settings, &path)
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn get_experience_ttl_days(state: tauri::State<'_, ExperienceStore>) -> Result<u64, String> {
    state
        .experience_ttl_days()
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn set_experience_ttl_days(
    state: tauri::State<'_, ExperienceStore>,
    days: u64,
) -> Result<u64, String> {
    if !(1..=3650).contains(&days) {
        return Err("TTL must be between 1 and 3650 days".into());
    }
    state
        .set_experience_ttl_days(days)
        .await
        .map_err(|err| err.to_string())?;
    state
        .experience_ttl_days()
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn get_experience_use_count_threshold(
    state: tauri::State<'_, ExperienceStore>,
) -> Result<u64, String> {
    state
        .experience_use_count_threshold()
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn set_experience_use_count_threshold(
    state: tauri::State<'_, ExperienceStore>,
    threshold: u64,
) -> Result<u64, String> {
    if threshold > 1_000_000 {
        return Err("use-count threshold too large".into());
    }
    state
        .set_experience_use_count_threshold(threshold)
        .await
        .map_err(|err| err.to_string())?;
    state
        .experience_use_count_threshold()
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
        let nc = crate::services::nats_connect(&url).map_err(|err| err.to_string())?;
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
    discovery_report(data_root(), lounge_endpoint, system_endpoint)
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
async fn list_agent_sessions(
    state: tauri::State<'_, ExperienceStore>,
) -> Result<Vec<serde_json::Value>, String> {
    let rows = state.list_sessions(64).map_err(|err| err.to_string())?;
    Ok(rows
        .into_iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "agent_id": s.agent_id,
                "app_kind": s.app_kind,
                "workspace_path": s.workspace_path,
                "state": s.state,
                "owner": s.owner,
                "created_by": s.created_by,
                "last_seen": s.last_seen,
                "orchestrated": s.is_orchestrated(),
            })
        })
        .collect())
}

#[tauri::command]
async fn list_a2a_background_tasks(
    state: tauri::State<'_, ExperienceStore>,
) -> Result<Vec<serde_json::Value>, String> {
    state
        .list_background_a2a_tasks(50)
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn get_remote_access(tunnel_url: Option<String>) -> services::RemoteAccessInfo {
    let bind = bridge::mcp_server::default_mcp_http_bind();
    services::remote_access_info(&bind, tunnel_url.as_deref())
}

#[tauri::command]
fn register_remote_tunnel(tunnel_url: String) -> Result<services::RemoteAccessInfo, String> {
    let host = services::add_allowed_origin(&tunnel_url).map_err(|e| e.to_string())?;
    log::info!("remote allow-list += {host}");
    let bind = bridge::mcp_server::default_mcp_http_bind();
    Ok(services::remote_access_info(&bind, Some(tunnel_url.trim())))
}

#[tauri::command]
fn remove_remote_tunnel(host: String) -> services::RemoteAccessInfo {
    let _ = services::remove_allowed_origin(&host);
    let bind = bridge::mcp_server::default_mcp_http_bind();
    services::remote_access_info(&bind, None)
}

#[tauri::command]
fn rotate_remote_token() -> services::RemoteAccessInfo {
    let _ = services::rotate_lounge_token();
    let bind = bridge::mcp_server::default_mcp_http_bind();
    services::remote_access_info(&bind, None)
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
    store: tauri::State<'_, ExperienceStore>,
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
    // User modunda çakışma engeller; auto modda enable sonraki boş porta geçer.
    if status.port_conflict && status.port_mode == GraphUiPortMode::User {
        return Err(status
            .conflict_message
            .unwrap_or_else(|| format!("Port {} meşgul", status.port)));
    }
    enable_graph_ui(
        &app,
        graph.inner(),
        &bridge,
        Some(store.inner()),
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

#[tauri::command]
async fn get_graph_ui_port_mode(graph: tauri::State<'_, GraphUiState>) -> Result<String, String> {
    Ok(graph.port_mode().as_str().into())
}

#[tauri::command]
async fn set_graph_ui_port_mode(
    app: tauri::AppHandle,
    store: tauri::State<'_, ExperienceStore>,
    services: tauri::State<'_, SharedServices>,
    graph: tauri::State<'_, GraphUiState>,
    mode: String,
) -> Result<String, String> {
    let parsed = GraphUiPortMode::parse(&mode)
        .ok_or_else(|| "port mode must be 'auto' or 'user'".to_string())?;
    let bridge = {
        let manager = services.lock().await;
        manager.memory().clone()
    };
    let port = {
        let live = bridge.http_port();
        if live > 0 {
            live
        } else {
            load_port_from_store(store.inner()).await
        }
    };
    persist_port_preference(store.inner(), &bridge, &app, graph.inner(), parsed, port)
        .await
        .map_err(|err| err.to_string())?;
    Ok(parsed.as_str().into())
}

#[cfg(test)]
mod kernel_log_config_tests {
    #[test]
    fn kernel_log_settings_are_multi_mb_keep_some() {
        let (max_size, strategy) = super::kernel_log_settings();
        assert_eq!(max_size, 5 * 1024 * 1024);
        match strategy {
            tauri_plugin_log::RotationStrategy::KeepSome(n) => assert_eq!(n, 5),
            other => panic!("expected KeepSome(5), got {other:?}"),
        }
        // Plugin defaults (tauri-plugin-log 2.10.0): 40_000 bytes + KeepOne.
        assert!(max_size > 40_000);
    }
}
