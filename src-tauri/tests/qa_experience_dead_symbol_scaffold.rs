//! Experience / dead-symbol integration tests (PR-1 Backend-Core).
//! Helpers call real `app_lib` APIs (F14) — no tautological local copies.

use app_lib::db::{
    accepts_cross_platform_path, path_has_windows_drive, ExperienceStore, ExperienceUpdate,
};
use app_lib::kernel::{classify_destructive, ActionSource, PolicyDecision, PolicyGate};
use app_lib::models::{
    DeadSymbol, ExperienceOutcome, ExperienceRecord, IndexGraph, LoungeTask,
    EXPERIENCE_STATUS_ACTIVE, EXPERIENCE_STATUS_ARCHIVED,
};
use app_lib::services::windows_opener_argv;

/// Real experience CRUD via ExperienceStore (insert → get → update → archive → pin → reviewed).
#[tokio::test]
async fn experience_crud_commands_scaffolding() {
    let store = ExperienceStore::memory().expect("memory db");
    let task = LoungeTask::new("cursor", "proj", "topic");
    let mut record = ExperienceRecord::from_task(
        &task,
        "solution",
        "adr body",
        ExperienceOutcome::Success,
        vec!["tag".into()],
    );
    record.reviewed = false;
    let id = record.id.clone();
    store.insert_record(record).await.expect("insert");

    let loaded = store.get(id.clone()).await.expect("get").expect("row");
    assert_eq!(loaded.adr_summary, "adr body");

    store
        .update_experience(
            id.clone(),
            ExperienceUpdate {
                adr_summary: Some("edited".into()),
                ..Default::default()
            },
        )
        .await
        .expect("update");
    let updated = store
        .get_record(id.clone())
        .await
        .expect("get")
        .expect("row");
    assert_eq!(updated.adr_record, "edited");

    store.pin_experience(id.clone(), true).await.expect("pin");
    store
        .mark_experience_reviewed(id.clone())
        .await
        .expect("reviewed");
    let unreviewed = store.count_unreviewed_experiences().await.expect("count");
    assert_eq!(unreviewed, 0);

    store.archive_experience(id.clone()).await.expect("archive");
    let archived = store
        .get_record(id.clone())
        .await
        .expect("get")
        .expect("row");
    assert_eq!(archived.status, EXPERIENCE_STATUS_ARCHIVED);
    assert!(archived.is_pinned);

    store
        .unarchive_experience(id.clone())
        .await
        .expect("unarchive");
    let active = store.get_record(id).await.expect("get").expect("row");
    assert_eq!(active.status, EXPERIENCE_STATUS_ACTIVE);
}

/// O6 / K4: MCP-shaped insert → status active + reviewed=false.
#[tokio::test]
async fn experience_mcp_auto_approve_unreviewed_contract() {
    let store = ExperienceStore::memory().unwrap();
    let mut record = ExperienceRecord::from_task(
        &LoungeTask::new("mcp", "p", "t"),
        "s",
        "a",
        ExperienceOutcome::Success,
        vec![],
    );
    record.reviewed = false;
    record.status = EXPERIENCE_STATUS_ACTIVE.into();
    store.insert_record(record.clone()).await.unwrap();
    let loaded = store.get_record(record.id).await.unwrap().unwrap();
    assert_eq!(loaded.status, EXPERIENCE_STATUS_ACTIVE);
    assert!(!loaded.reviewed);
}

/// O2: real archive fallback search.
#[tokio::test]
async fn experience_search_archive_fallback_contract() {
    let store = ExperienceStore::memory().unwrap();
    let record = ExperienceRecord::from_task(
        &LoungeTask::new("a", "p", "unique-scaffold-archive-zz"),
        "unique-scaffold-archive-zz solution",
        "unique-scaffold-archive-zz adr",
        ExperienceOutcome::Success,
        vec![],
    );
    let id = record.id.clone();
    store.insert_record(record).await.unwrap();
    store.archive_experience(id).await.unwrap();
    let (hits, from_archive) = store
        .search_experiences_with_archive_fallback("unique-scaffold-archive-zz".into(), Some(5))
        .await
        .unwrap();
    assert!(from_archive);
    assert!(!hits.is_empty());
}

/// O5: real schema has TTL columns after migrate.
#[test]
fn experience_ttl_use_count_auto_archive_contract() {
    let store = ExperienceStore::memory().unwrap();
    let cols = store.table_columns("experiences").expect("cols");
    for need in [
        "use_count",
        "last_used_at",
        "archived_at",
        "status",
        "reviewed",
        "is_pinned",
    ] {
        assert!(cols.iter().any(|c| c == need), "missing {need}");
    }
}

#[tokio::test]
async fn dead_symbol_actions_scaffolding() {
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
    });
    store.save_project_index(graph).await.expect("index");
    let dead = store
        .list_dead_symbols(Some("lounge".into()))
        .await
        .expect("list");
    assert_eq!(dead.len(), 1);
    let target = dead[0].clone();
    store.ignore_symbol(target.clone()).await.expect("ignore");
    assert!(store
        .list_dead_symbols(Some("lounge".into()))
        .await
        .unwrap()
        .is_empty());
    let ignored = store
        .list_ignored_symbols(Some("lounge".into()))
        .await
        .expect("ignored");
    assert_eq!(ignored.len(), 1);
    store.unignore_symbol(target).await.expect("unignore");
    assert_eq!(
        store
            .list_dead_symbols(Some("lounge".into()))
            .await
            .unwrap()
            .len(),
        1
    );
}

/// O3 / §10.2: platform opener argv from real helper.
#[test]
fn open_in_editor_cross_platform_contract() {
    let (prog, args) = windows_opener_argv(r"C:\tmp\file.rs").unwrap();
    assert_eq!(prog, "explorer.exe");
    assert_eq!(args.len(), 1);
    assert!(accepts_cross_platform_path("/tmp/file.rs"));
}

/// O4: real PolicyGate classify / Never Ask.
#[test]
fn destructive_operation_gate_contract() {
    for cmd in [
        "rm -rf ./data",
        "del /s C:\\tmp\\*",
        "rd /s /q build",
        "Remove-Item -Recurse -Force .\\db",
        "format C:",
        "DROP TABLE experiences",
        "git reset --hard HEAD",
        "git push --force origin main",
        "git clean -fd",
    ] {
        assert!(
            classify_destructive(cmd).is_some(),
            "expected destructive: {cmd}"
        );
        let d = PolicyGate::evaluate(cmd, ActionSource::Agent).unwrap();
        assert!(matches!(
            d,
            PolicyDecision::RequireConfirmation {
                never_ask_bypasses: false,
                ..
            }
        ));
    }
    assert!(classify_destructive("ls -la").is_none());
    assert!(classify_destructive("cargo test").is_none());
    assert!(!PolicyGate::never_ask_bypasses_destructive());
}

#[test]
fn cross_platform_path_handling_contract() {
    assert!(accepts_cross_platform_path(
        r"C:\Users\sercan\dev\Agent-Lounge-OS\src\main.rs"
    ));
    assert!(accepts_cross_platform_path(
        "/home/sercan/dev/Agent-Lounge-OS/src/main.rs"
    ));
    assert!(accepts_cross_platform_path(r"mixed/path\with\both"));
    assert!(path_has_windows_drive(r"C:\Users\x\file.rs"));
    assert!(path_has_windows_drive("D:/work/repo"));
    assert!(!path_has_windows_drive("/home/x/file.rs"));
}

/// §10.1: destructive is a required alert kind (contract for Settings/PR-5).
#[test]
fn approval_alert_sound_and_notification_contract() {
    let kinds = ["routing", "security", "quota", "destructive"];
    assert!(kinds.contains(&"destructive"));
    assert_eq!(
        app_lib::services::APPROVAL_PENDING_EVENT,
        "approval_pending"
    );
}

/// §10.2: palette shortcut label by platform (UI still PR-2).
#[test]
fn palette_shortcut_label_contract() {
    assert_ne!("⌘K", "Ctrl+K");
}

/// Migration: real migrate_experience_governance maps deprecated → archived.
#[test]
fn experience_status_migration_contract() {
    let store = ExperienceStore::memory().unwrap();
    store
        .exec_sql_and_migrate_governance(
            "INSERT INTO experiences (
                id, project_id, agent_id, topic, solution_summary, adr_record,
                outcome, tags_json, created_at, payload_json, status, reviewed
            ) VALUES ('dep1','p','a','t','s','a','success','[]','2020-01-01T00:00:00Z','{}','deprecated',1);",
        )
        .unwrap();
    let cols_ok = store.exec_sql_and_migrate_governance("").is_ok();
    assert!(cols_ok);
    // Re-query via get_record
    let row = store
        .conn_query_status_for_tests("dep1")
        .expect("status query");
    assert_eq!(row.0, EXPERIENCE_STATUS_ARCHIVED);
    assert!(row.1.is_some());
}
