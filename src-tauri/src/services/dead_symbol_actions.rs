//! Dead-symbol actions: index-backed editor open + NATS fix task (DS-03 / DS-07).

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::db::ExperienceStore;
use crate::kernel::workflow_engine::{grok_bot_in_fleet, GROK_BOT_WORKER};
use crate::models::{DeadSymbol, LoungeTask, TaskKind, TASK_REQUESTED};

use super::open_editor::open_in_editor;

#[derive(Debug, Clone, Serialize)]
pub struct FixDeadSymbolResult {
    pub task_id: String,
    pub subject: String,
    pub target_agent: String,
    pub message: String,
}

/// Open a dead symbol in the editor using the path stored in the index (never webview path).
pub async fn open_dead_symbol_in_editor(
    store: &ExperienceStore,
    symbol: DeadSymbol,
    editor_command: Option<String>,
) -> Result<()> {
    let resolved = store.resolve_dead_symbol(symbol).await?;
    open_in_editor(store, resolved.file_path, resolved.line, editor_command).await
}

/// Publish `lounge.task.requested` to fix/remove a dead symbol when an agent is available.
pub async fn fix_dead_symbol_with_agent(
    store: &ExperienceStore,
    bus_url: &str,
    symbol: DeadSymbol,
) -> Result<FixDeadSymbolResult> {
    if !grok_bot_in_fleet() {
        bail!("No agent available — connect Grok Bot or another worker in Fleet");
    }

    let resolved = store.resolve_dead_symbol(symbol).await?;
    let project = resolved
        .symbol
        .project_id
        .clone()
        .unwrap_or_else(|| "agent-lounge-os".into());
    let file = resolved.file_path.clone();
    let line = resolved.line.unwrap_or(0);
    let name = resolved.symbol.name.clone();

    let summary = format!("Fix dead symbol {name} @ {file}:{line}");
    let mut task = LoungeTask::new("dead_symbols_ui", &project, summary);
    task.kind = TaskKind::CodeAnalysis;
    task.target_agent = Some(GROK_BOT_WORKER.into());
    task.repo_path = Some(resolved.repo_path);
    task.ast_refs = vec![name.clone()];
    task.workflow_chain = Some(format!(
        "{project} -> FIX_WITH_AGENT {name} ({file}:{line})"
    ));

    let payload = serde_json::json!({
        "action": "fix_dead_symbol",
        "symbol": name,
        "file": file,
        "line": line,
        "project": project,
        "kind": resolved.symbol.kind,
    });
    task.summary = format!("{}\n{}", task.summary, payload);

    let subject = TASK_REQUESTED.to_string();
    let bytes = serde_json::to_vec(&task).context("task serialize")?;
    let url = bus_url.to_string();
    let subject_err = subject.clone();
    #[allow(deprecated)]
    tokio::task::spawn_blocking(move || {
        let nc = nats::connect(&url).map_err(|err| err.to_string())?;
        nc.publish(&subject, bytes).map_err(|err| err.to_string())
    })
    .await
    .context("nats join")?
    .map_err(|err| anyhow::anyhow!("NATS publish failed ({subject_err}): {err}"))?;

    Ok(FixDeadSymbolResult {
        task_id: task.id,
        subject: TASK_REQUESTED.into(),
        target_agent: GROK_BOT_WORKER.into(),
        message: format!("Task queued for {name}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::IndexGraph;

    #[tokio::test]
    async fn resolve_rejects_webview_path_spoofing() {
        let store = ExperienceStore::memory().expect("memory db");
        let mut graph = IndexGraph {
            project: "lounge".into(),
            repo_path: "/tmp/lounge".into(),
            ..Default::default()
        };
        graph.dead.push(DeadSymbol {
            project_id: Some("lounge".into()),
            name: "unused_fn".into(),
            kind: "unused".into(),
            file: Some("src/x.rs".into()),
            line: Some(10),
            detail: None,
            last_ref: None,
        });
        store.save_project_index(graph).await.expect("save");

        let spoof = DeadSymbol {
            project_id: Some("lounge".into()),
            name: "unused_fn".into(),
            kind: "unused".into(),
            file: Some("/etc/passwd".into()),
            line: Some(1),
            detail: None,
            last_ref: None,
        };
        let resolved = store.resolve_dead_symbol(spoof).await.expect("resolve");
        assert!(
            resolved.file_path.ends_with("src/x.rs") || resolved.file_path == "src/x.rs",
            "got {}",
            resolved.file_path
        );
        assert!(!resolved.file_path.contains("passwd"));
        assert_eq!(resolved.line, Some(10));
    }
}
