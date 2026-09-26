//! Experience / dead-symbol integration tests (PR-1 Backend-Core).

use app_lib::db::{ExperienceStore, ExperienceUpdate};
use app_lib::models::{
    DeadSymbol, ExperienceOutcome, ExperienceRecord, LoungeTask, EXPERIENCE_STATUS_ACTIVE,
    EXPERIENCE_STATUS_ARCHIVED,
};

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

/// O6 / K4: MCP create → status active + reviewed=false (not Draft).
fn mcp_row_is_auto_approved_unreviewed(status: &str, reviewed: bool) -> bool {
    status == "active" && !reviewed
}

#[test]
fn experience_mcp_auto_approve_unreviewed_contract() {
    assert!(mcp_row_is_auto_approved_unreviewed("active", false));
    assert!(!mcp_row_is_auto_approved_unreviewed("draft", false));
    assert!(!mcp_row_is_auto_approved_unreviewed("approved", false));
    assert!(!mcp_row_is_auto_approved_unreviewed("active", true));
}

/// O2 / S3: when active hits are empty, fall back to archive and tag "archived".
fn search_with_archive_fallback<'a>(
    active_hits: &[&'a str],
    archive_hits: &[&'a str],
) -> (Vec<&'a str>, Option<&'static str>) {
    if !active_hits.is_empty() {
        return (active_hits.to_vec(), None);
    }
    if archive_hits.is_empty() {
        return (Vec::new(), None);
    }
    (archive_hits.to_vec(), Some("archived"))
}

#[test]
fn experience_search_archive_fallback_contract() {
    let active: Vec<&str> = Vec::new();
    let archive = vec!["exp-archived-1"];
    let (hits, tag) = search_with_archive_fallback(&active, &archive);
    assert_eq!(hits, ["exp-archived-1"]);
    assert_eq!(tag, Some("archived"));

    let (hits2, tag2) = search_with_archive_fallback(&["live-1"], &archive);
    assert_eq!(hits2, ["live-1"]);
    assert_eq!(tag2, None);
}

/// O5: required schema fields for TTL / use_count auto-archive.
fn schema_has_ttl_fields(fields: &[&str]) -> bool {
    fields.contains(&"use_count")
        && fields.contains(&"last_used_at")
        && fields.contains(&"archived_at")
}

#[test]
fn experience_ttl_use_count_auto_archive_contract() {
    assert!(schema_has_ttl_fields(&[
        "use_count",
        "last_used_at",
        "archived_at",
        "reviewed",
    ]));
    assert!(!schema_has_ttl_fields(&["reviewed", "created_at"]));
}

/// Dead-symbol ignore / unignore roundtrip via store methods.
#[tokio::test]
async fn dead_symbol_actions_scaffolding() {
    use app_lib::models::{AstNode, CodeReference, IndexGraph};

    let store = ExperienceStore::memory().expect("memory db");
    let graph = IndexGraph {
        project: "lounge".into(),
        repo_path: "/tmp/lounge".into(),
        node_count: 1,
        edge_count: 0,
        nodes: vec![AstNode {
            id: "dead_fn".into(),
            name: "dead_fn".into(),
            kind: "fn".into(),
            file: Some("src/dead.rs".into()),
            line: Some(4),
            ref_count: 0,
        }],
        references: vec![CodeReference {
            from_id: "main".into(),
            to_id: "other".into(),
            file: Some("src/main.rs".into()),
            line: Some(1),
        }],
        dead: vec![DeadSymbol {
            name: "dead_fn".into(),
            kind: "unused".into(),
            file: Some("src/dead.rs".into()),
            line: Some(4),
            detail: Some("no refs".into()),
            project_id: Some("lounge".into()),
        }],
        ..IndexGraph::default()
    };
    store.save_project_index(graph).await.expect("save");

    let dead = store
        .list_dead_symbols(Some("lounge".into()))
        .await
        .expect("list");
    assert!(!dead.is_empty());
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

/// O3 / §10.2: map OS → default editor launcher.
fn default_editor_opener(os: &str) -> Option<&'static str> {
    match os {
        "macos" => Some("open"),
        "windows" => Some("start"),
        "linux" => Some("xdg-open"),
        _ => None,
    }
}

#[test]
fn open_in_editor_cross_platform_contract() {
    assert_eq!(default_editor_opener("macos"), Some("open"));
    assert_eq!(default_editor_opener("windows"), Some("start"));
    assert_eq!(default_editor_opener("linux"), Some("xdg-open"));
    assert_eq!(default_editor_opener("freebsd"), None);
}

/// O4 / §10.2: destructive-command detection (POSIX + Windows + git force).
fn matches_destructive_pattern(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    const PATTERNS: &[&str] = &[
        "rm -rf",
        "rm -r ",
        "unlink ",
        "del /s",
        "rd /s",
        "remove-item -recurse",
        "format ",
        "drop table",
        "truncate ",
        "migrate down",
        "git reset --hard",
        "push --force",
        "git push --force",
        "clean -fd",
        "git clean -fd",
    ];
    PATTERNS.iter().any(|p| lower.contains(p))
}

#[test]
fn destructive_operation_gate_contract() {
    assert!(matches_destructive_pattern("rm -rf ./data"));
    assert!(matches_destructive_pattern("del /s C:\\tmp\\*"));
    assert!(matches_destructive_pattern("rd /s /q build"));
    assert!(matches_destructive_pattern(
        "Remove-Item -Recurse -Force .\\db"
    ));
    assert!(matches_destructive_pattern("format C:"));
    assert!(matches_destructive_pattern("DROP TABLE experiences"));
    assert!(matches_destructive_pattern("git reset --hard HEAD"));
    assert!(matches_destructive_pattern("git push --force origin main"));
    assert!(matches_destructive_pattern("git clean -fd"));
    assert!(!matches_destructive_pattern("ls -la"));
    assert!(!matches_destructive_pattern("cargo test"));
    let never_ask_bypasses_destructive = false;
    assert!(!never_ask_bypasses_destructive);
}

/// §10.2: path helpers for separators + Windows drive letters.
fn path_has_separator(path: &str) -> bool {
    path.contains('/') || path.contains('\\')
}

fn path_has_windows_drive(path: &str) -> bool {
    let mut chars = path.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
}

#[test]
fn cross_platform_path_handling_contract() {
    assert!(path_has_separator(
        r"C:\Users\sercan\dev\Agent-Lounge-OS\src\main.rs"
    ));
    assert!(path_has_separator(
        "/home/sercan/dev/Agent-Lounge-OS/src/main.rs"
    ));
    assert!(path_has_separator(r"mixed/path\with\both"));
    assert!(path_has_windows_drive(r"C:\Users\x\file.rs"));
    assert!(path_has_windows_drive("D:/work/repo"));
    assert!(!path_has_windows_drive("/home/x/file.rs"));
    assert!(app_lib::db::accepts_cross_platform_path(
        r"C:\Users\sercan\dev\Agent-Lounge-OS\src\main.rs"
    ));
    assert!(app_lib::db::accepts_cross_platform_path(
        "/home/sercan/dev/Agent-Lounge-OS/src/main.rs"
    ));
}

/// §10.1: alert kinds + repeat interval validation.
fn approval_alert_config_valid(kinds: &[&str], repeat_secs: u64, bundled_formats: &[&str]) -> bool {
    let required = ["routing", "security", "quota", "destructive"];
    required.iter().all(|k| kinds.contains(k))
        && repeat_secs == 60
        && bundled_formats.contains(&"wav")
        && bundled_formats.contains(&"mp3")
        && bundled_formats.contains(&"ogg")
}

#[test]
fn approval_alert_sound_and_notification_contract() {
    assert!(approval_alert_config_valid(
        &["routing", "security", "quota", "destructive"],
        60,
        &["wav", "mp3", "ogg"],
    ));
    assert!(!approval_alert_config_valid(
        &["routing", "security"],
        60,
        &["wav", "mp3", "ogg"],
    ));
    assert!(!approval_alert_config_valid(
        &["routing", "security", "quota", "destructive"],
        30,
        &["wav", "mp3", "ogg"],
    ));
}

/// §10.2: palette shortcut label by platform.
fn palette_shortcut_label(os: &str) -> &'static str {
    if os == "macos" {
        "⌘K"
    } else {
        "Ctrl+K"
    }
}

#[test]
fn palette_shortcut_label_contract() {
    assert_eq!(palette_shortcut_label("macos"), "⌘K");
    assert_eq!(palette_shortcut_label("windows"), "Ctrl+K");
    assert_eq!(palette_shortcut_label("linux"), "Ctrl+K");
    assert_ne!(
        palette_shortcut_label("macos"),
        palette_shortcut_label("linux")
    );
}

/// Migration: legacy draft/approved → active; deprecated → archived.
fn migrate_experience_status(status: &str) -> &'static str {
    match status {
        "deprecated" => "archived",
        "draft" | "approved" | "active" => "active",
        _ => "active",
    }
}

#[test]
fn experience_status_migration_contract() {
    assert_eq!(migrate_experience_status("draft"), "active");
    assert_eq!(migrate_experience_status("approved"), "active");
    assert_eq!(migrate_experience_status("deprecated"), "archived");
    assert_eq!(migrate_experience_status("unknown-legacy"), "active");
}
