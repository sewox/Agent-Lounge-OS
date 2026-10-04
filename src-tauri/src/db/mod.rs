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
    admit_task_atomic, configured_agent_silence, configured_max_hops,
    configured_silence_scan_interval, mark_silent_tasks_needs_human, migrate_a2a, AdmitError,
    AdmitOutcome, A2A_SCHEMA_VERSION, ABSOLUTE_MAX_HOPS, DEFAULT_AGENT_SILENCE,
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
