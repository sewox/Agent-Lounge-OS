//! PR-0 scaffolding for experience / dead-symbol command integration tests (S3).
//! These tests are intentionally incomplete stubs until PR-1 wires the commands.
//! They compile and document the acceptance surface from docs/qa/qa-plan.md.

#![allow(dead_code)]

/// Placeholder: `get_experience` / `update_experience` / soft-delete archive.
/// PR-1 will inject an ephemeral SQLite + MemoryBridgeConfig and assert roundtrips.
#[test]
fn experience_crud_commands_scaffolding() {
    // Expected commands (not yet registered on main):
    // - get_experience(id)
    // - update_experience(id, patch)
    // - archive_experience(id)  // soft delete via archived_at
    // - pin_experience(id, pinned)
    // - approve_experience(id)
    let planned = [
        "get_experience",
        "update_experience",
        "archive_experience",
        "pin_experience",
        "approve_experience",
    ];
    assert!(!planned.is_empty(), "PR-1 must implement experience commands");
}

/// Placeholder: dead-symbol ignore + FIX_WITH_AGENT NATS publish.
#[test]
fn dead_symbol_actions_scaffolding() {
    let planned = [
        "ignore_dead_symbol",
        "list_ignored_symbols",
        "unignore_dead_symbol",
        "open_in_editor",
        "create_task_from_dead_symbol", // NATS lounge.task.requested
    ];
    assert_eq!(planned.len(), 5);
}

/// Documents migration expectation: existing rows → Approved; count preserved.
#[test]
fn experience_status_migration_contract() {
    let statuses = ["draft", "approved", "deprecated"];
    assert!(statuses.contains(&"approved"));
}
