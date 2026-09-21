//! SQLite tecrübe deposu ve semantik (kosinüs) arama.

mod connected_tools;
mod embedding;
mod experience_store;
mod experiences;
mod project_index;
mod vector_memory;

pub use embedding::{cosine_similarity, lexical_embedding};
pub use experience_store::{fast_retrieve, knowledge_hit_triggers, FastRetrieveQuery};
pub use experiences::{default_db_path, ExperienceStore};
