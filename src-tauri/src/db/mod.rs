//! SQLite tecrübe deposu ve semantik (kosinüs) arama.

mod a2a;
mod connected_tools;
mod embedding;
mod experience_governance;
mod experience_store;
mod experiences;
pub mod feedback;
mod journal;
mod project_identity;
mod project_index;
mod vault_fts;
mod vector_memory;

pub use a2a::{
    admit_task_atomic, configured_agent_silence, configured_incomplete_orphan_ttl,
    configured_max_hops, configured_must_deliver_ttl, configured_result_orphan_ttl,
    configured_silence_scan_interval, generate_task_token, hash_task_token,
    mark_silent_tasks_needs_human, migrate_a2a, next_poll_after_secs, AdmitError, AdmitOutcome,
    A2A_SCHEMA_VERSION, ABSOLUTE_MAX_HOPS, DEFAULT_AGENT_SILENCE, DEFAULT_IDEMPOTENCY_TTL,
    DEFAULT_INCOMPLETE_ORPHAN_TTL, DEFAULT_MUST_DELIVER_TTL, DEFAULT_RESULT_ORPHAN_TTL,
    MUST_DELIVER_MAX_PER_AGENT, MUST_DELIVER_MAX_PER_SESSION, NEXT_ACTION_WAIT_TASK,
};
pub use embedding::{cosine_similarity, lexical_embedding};
pub use experience_governance::{
    cmp_search_with_review_band, lexical_search_score, migrate_experience_governance,
    migrate_mcp_quality_v1, normalize_search_score, review_score_bucket,
    sort_scored_experiences_for_search, strip_original_content_for_ui,
    strip_original_content_for_ui_vec, ArchiveClock, ExperienceUpdate, FakeClock,
    ReviewQueueFilter, ReviewStatFilter, ReviewStats, SearchRankKey, SystemClock,
    DEFAULT_EXPERIENCE_TTL_DAYS, MIGRATION_MCP_QUALITY_V1, MIGRATION_PLACEHOLDER_CLEANUP_V1,
    MIGRATION_TITLE_BACKFILL_ENQUEUE_V1, REVIEW_SCORE_BAND, SETTING_EXPERIENCE_TTL_DAYS,
};
pub use experience_store::{
    fast_retrieve, get_relevant_context, knowledge_hit_triggers, FastRetrieveQuery,
};
pub use experiences::{column_names, default_db_path, ExperienceStore};
pub use journal::{
    append_journal_best_effort, append_journal_entry_conn, append_journal_entry_immediate,
    canonical_project_id, explain_why, journal_task_lifecycle, migrate_journal, query_journal,
    timestamp_at_offset_ms, verify_journal_chain, JournalAppendInput, JournalEntry,
    JournalQueryFilter, JournalVerificationReport, JournalWhyReport, ACTION_CONTROL_HALTED,
    ACTION_CONTROL_RESUMED, ACTION_EXPERIENCE_ARCHIVED, ACTION_EXPERIENCE_RECORDED,
    ACTION_EXPERIENCE_REJECTED, ACTION_EXPERIENCE_REVIEWED, ACTION_EXPERIENCE_UNARCHIVED,
    ACTION_EXPERIENCE_UNDO_REJECTED, ACTION_TASK_APPROVAL_APPROVED, ACTION_TASK_APPROVAL_PENDING,
    ACTION_TASK_APPROVAL_REJECTED, ACTION_TASK_CANCELLED, ACTION_TASK_CLAIMED,
    ACTION_TASK_COMPLETED, ACTION_TASK_EXPIRED_OR_ORPHAN, ACTION_TASK_REQUESTED,
    DEFAULT_JOURNAL_QUERY_LIMIT, GENESIS_PREV_HASH, MAX_JOURNAL_QUERY_LIMIT,
};
pub use project_identity::{
    bind_write_project_id, canonicalize_repo_path, choose_display_name, migrate_project_identity,
    migrate_project_ids_to_canonical, prefer_canonical, register_path_project,
    strip_windows_extended_prefix, upsert_alias, upsert_project, ProjectBind, ProjectBindKind,
    MIGRATION_PROJECT_ID_CANONICAL_V1, MIGRATION_PROJECT_ID_CANONICAL_V1_BACKUP,
    PROJECT_ID_UNCLASSIFIED, PROJECT_ID_UNINDEXED_PLACEHOLDER,
};
pub use project_index::{accepts_cross_platform_path, normalize_path_str, path_has_windows_drive};
pub use vault_fts::{
    downgrade_vault_fts, fts5_available, hybrid_search_score, like_pattern, migrate_vault_fts,
    normalize_bm25, turkish_fold, HYBRID_LEXICAL_WEIGHT, HYBRID_SEMANTIC_WEIGHT,
    VAULT_FTS_SCHEMA_VERSION, VAULT_FTS_SCHEMA_VERSION_KEY,
};
