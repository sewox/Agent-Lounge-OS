//! SQLite tecrübe deposu ve semantik (kosinüs) arama.

mod a2a;
mod connected_tools;
mod embedding;
mod experience_governance;
mod experience_store;
mod experiences;
pub mod feedback;
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
    migrate_experience_governance, ArchiveClock, ExperienceUpdate, FakeClock, SystemClock,
    DEFAULT_EXPERIENCE_TTL_DAYS, MIGRATION_PLACEHOLDER_CLEANUP_V1, SETTING_EXPERIENCE_TTL_DAYS,
};
pub use experience_store::{
    fast_retrieve, get_relevant_context, knowledge_hit_triggers, FastRetrieveQuery,
};
pub use experiences::{column_names, default_db_path, ExperienceStore};
pub use project_index::{accepts_cross_platform_path, normalize_path_str, path_has_windows_drive};
