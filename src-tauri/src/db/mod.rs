//! SQLite tecrübe deposu ve semantik (kosinüs) arama.

mod connected_tools;
mod embedding;
mod experience_governance;
mod experience_store;
mod experiences;
pub mod feedback;
mod project_index;
mod vector_memory;

pub use embedding::{cosine_similarity, lexical_embedding};
pub use experience_governance::{
    ArchiveClock, ExperienceUpdate, FakeClock, SystemClock, DEFAULT_EXPERIENCE_TTL_DAYS,
    SETTING_EXPERIENCE_TTL_DAYS,
};
pub use experience_store::{
    fast_retrieve, get_relevant_context, knowledge_hit_triggers, FastRetrieveQuery,
};
pub use experiences::{default_db_path, ExperienceStore};
pub use project_index::{
    accepts_cross_platform_path, normalize_path_str, path_has_windows_drive,
};
