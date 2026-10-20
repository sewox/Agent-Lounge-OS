//! SQLite tecrübe deposu ve semantik (kosinüs) arama.

mod a2a;
mod connected_tools;
mod embedding;
mod experience_governance;
mod experience_store;
mod experiences;
pub mod feedback;
mod project_identity;
mod project_index;
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
    sort_experiences_for_search, strip_original_content_for_ui, strip_original_content_for_ui_vec,
    ArchiveClock, ExperienceUpdate, FakeClock, ReviewQueueFilter, ReviewStatFilter, ReviewStats,
    SearchRankKey, SystemClock, DEFAULT_EXPERIENCE_TTL_DAYS, MIGRATION_MCP_QUALITY_V1,
    MIGRATION_PLACEHOLDER_CLEANUP_V1, MIGRATION_TITLE_BACKFILL_ENQUEUE_V1, REVIEW_SCORE_BAND,
    SEARCH_SAME_PROJECT_BOOST, SETTING_EXPERIENCE_TTL_DAYS,
};
pub use experience_store::{
    fast_retrieve, get_relevant_context, knowledge_hit_triggers, FastRetrieveQuery,
};
pub use experiences::{column_names, default_db_path, ExperienceStore};
pub use project_identity::{
    bind_write_project_id, canonicalize_repo_path, choose_display_name, migrate_project_identity,
    migrate_project_ids_to_canonical, prefer_canonical, register_path_project,
    strip_windows_extended_prefix, upsert_alias, upsert_project, ProjectBind, ProjectBindKind,
    MIGRATION_PROJECT_ID_CANONICAL_V1, MIGRATION_PROJECT_ID_CANONICAL_V1_BACKUP,
    PROJECT_ID_UNCLASSIFIED, PROJECT_ID_UNINDEXED_PLACEHOLDER,
};
pub use project_index::{accepts_cross_platform_path, normalize_path_str, path_has_windows_drive};
