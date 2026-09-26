//! Lounge MCP — Cursor / Claude Desktop stdio sunucusu.
//!
//! Kullanım:
//!   cargo run --manifest-path src-tauri/Cargo.toml --bin lounge-mcp
//!
//! Ortam:
//!   LOUNGE_DB_PATH / LOUNGE_EXPERIENCE_DB — SQLite yolu
//!   LOUNGE_NATS_URL — varsayılan nats://127.0.0.1:4222

use std::process::ExitCode;

use app_lib::bridge::mcp_server::{resolve_db_path, resolve_nats_url, run_stdio, workspace_root};
use app_lib::db::ExperienceStore;

fn main() -> ExitCode {
    // MCP: yalnızca stderr'e log (stdout protokole ayrılmış).
    eprintln!(
        "[lounge-mcp] Agent Lounge OS MCP {} — stdio",
        env!("CARGO_PKG_VERSION")
    );

    let db_path = resolve_db_path(workspace_root());
    let nats_url = resolve_nats_url();
    eprintln!("[lounge-mcp] db={} nats={}", db_path.display(), nats_url);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("[lounge-mcp] runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    let result = runtime.block_on(async {
        let store = ExperienceStore::open(&db_path)
            .map_err(|err| format!("experience store açılamadı ({}): {err}", db_path.display()))?;
        run_stdio(store, nats_url)
            .await
            .map_err(|err| err.to_string())
    });

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("[lounge-mcp] {err}");
            ExitCode::FAILURE
        }
    }
}
