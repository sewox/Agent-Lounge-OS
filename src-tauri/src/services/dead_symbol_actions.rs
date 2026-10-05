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

/// Pure builder for FIX_WITH_AGENT NATS tasks (unit-testable; no bus I/O).
pub fn build_fix_dead_symbol_task(
    resolved_file: &str,
    resolved_line: Option<i64>,
    resolved_repo: &str,
    symbol: &DeadSymbol,
    target_agent: &str,
) -> LoungeTask {
    let project = symbol
        .project_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("agent-lounge-os");
    let name = symbol.name.as_str();
    let line = resolved_line.unwrap_or(0);
    let summary = format!("Fix dead symbol {name} @ {resolved_file}:{line}");
    let mut task = LoungeTask::new("dead_symbols_ui", project, summary);
    task.kind = TaskKind::CodeAnalysis;
    task.target_agent = Some(target_agent.to_string());
    task.repo_path = Some(resolved_repo.to_string());
    task.ast_refs = vec![name.to_string()];
    task.file = Some(resolved_file.to_string());
    task.line = resolved_line;
    task.symbol = Some(name.to_string());
    task.workflow_chain = Some(format!(
        "{project} -> FIX_WITH_AGENT {name} ({resolved_file}:{line})"
    ));
    task
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
    fix_dead_symbol_with_agent_gated(store, bus_url, symbol, grok_bot_in_fleet).await
}

pub async fn fix_dead_symbol_with_agent_gated<F>(
    store: &ExperienceStore,
    bus_url: &str,
    symbol: DeadSymbol,
    fleet_has_agent: F,
) -> Result<FixDeadSymbolResult>
where
    F: Fn() -> bool,
{
    if !fleet_has_agent() {
        bail!("No agent available — connect Grok Bot or another worker in Fleet");
    }

    let resolved = store.resolve_dead_symbol(symbol).await?;
    let name = resolved.symbol.name.clone();
    let task = build_fix_dead_symbol_task(
        &resolved.file_path,
        resolved.line,
        &resolved.repo_path,
        &resolved.symbol,
        GROK_BOT_WORKER,
    );

    let subject = TASK_REQUESTED.to_string();
    let bytes = serde_json::to_vec(&task).context("task serialize")?;
    let url = bus_url.to_string();
    let subject_err = subject.clone();
    #[allow(deprecated)]
    tokio::task::spawn_blocking(move || {
        let nc = crate::services::nats_connect(&url).map_err(|err| err.to_string())?;
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
    use crate::services::open_editor::{platform_opener_argv, windows_opener_argv};

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
        let err = store.resolve_dead_symbol(spoof).await;
        assert!(
            err.is_err(),
            "spoofed file must not resolve by name alone: {err:?}"
        );

        let ok = store
            .resolve_dead_symbol(DeadSymbol {
                project_id: Some("lounge".into()),
                name: "unused_fn".into(),
                kind: "unused".into(),
                file: Some("src/x.rs".into()),
                line: Some(10),
                ..Default::default()
            })
            .await
            .expect("real identity");
        assert!(ok.file_path.ends_with("src/x.rs"), "{}", ok.file_path);
        assert_eq!(ok.line, Some(10));
    }

    #[test]
    fn build_fix_task_has_structured_fields() {
        let symbol = DeadSymbol {
            project_id: Some("Agent-Lounge-OS".into()),
            name: "orphan_dispatch".into(),
            kind: "unused".into(),
            file: Some("src/kernel/dispatcher.rs".into()),
            line: Some(142),
            ..Default::default()
        };
        let task = build_fix_dead_symbol_task(
            "/repo/src/kernel/dispatcher.rs",
            Some(142),
            "/repo",
            &symbol,
            GROK_BOT_WORKER,
        );
        assert_eq!(task.project_id, "Agent-Lounge-OS");
        assert_eq!(task.file.as_deref(), Some("/repo/src/kernel/dispatcher.rs"));
        assert_eq!(task.line, Some(142));
        assert_eq!(task.symbol.as_deref(), Some("orphan_dispatch"));
        assert_eq!(task.repo_path.as_deref(), Some("/repo"));
        assert_eq!(task.target_agent.as_deref(), Some(GROK_BOT_WORKER));
        assert_eq!(task.ast_refs, vec!["orphan_dispatch".to_string()]);
        assert!(!task.summary.contains('{'), "no JSON glued onto summary");
        assert!(task.summary.contains("orphan_dispatch"));
        let wire = serde_json::to_value(&task).unwrap();
        assert_eq!(wire["file"], "/repo/src/kernel/dispatcher.rs");
        assert_eq!(wire["line"], 142);
        assert_eq!(wire["symbol"], "orphan_dispatch");
        assert_eq!(wire["project_id"], "Agent-Lounge-OS");
    }

    #[tokio::test]
    async fn fix_with_agent_no_agent_branch() {
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
            ..Default::default()
        });
        store.save_project_index(graph).await.expect("save");
        let err = fix_dead_symbol_with_agent_gated(
            &store,
            "nats://127.0.0.1:4222",
            DeadSymbol {
                project_id: Some("lounge".into()),
                name: "unused_fn".into(),
                kind: "unused".into(),
                file: Some("src/x.rs".into()),
                line: Some(10),
                ..Default::default()
            },
            || false,
        )
        .await;
        assert!(err.is_err());
        let msg = format!("{}", err.unwrap_err());
        assert!(msg.contains("No agent available"), "{msg}");
    }

    #[test]
    fn opener_argv_for_index_path_with_spaces_and_ampersand_is_one_element() {
        let path = r"C:\tmp\my project\a&b.rs";
        let (program, args) = windows_opener_argv(path).expect("argv");
        assert_eq!(program, "explorer.exe");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], path);

        let posix = "/tmp/my project/a&b.rs";
        let (prog, args) = platform_opener_argv(posix).expect("platform");
        assert_eq!(args.len(), 1);
        assert_eq!(args[0], posix);
        assert!(prog == "open" || prog == "xdg-open" || prog == "explorer.exe");
    }
}
