use serde::{Deserialize, Serialize};

/// `codebase-memory-mcp cli index_repository` stdout yükü.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IndexSnapshot {
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub nodes: u64,
    #[serde(default)]
    pub edges: u64,
    #[serde(default)]
    pub files: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProjectSummary {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub root_path: Option<String>,
    #[serde(default)]
    pub nodes: u64,
    #[serde(default)]
    pub edges: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProjectList {
    #[serde(default)]
    pub projects: Vec<ProjectSummary>,
}
