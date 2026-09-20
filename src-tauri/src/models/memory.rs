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
    #[serde(default)]
    pub dead: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AstNode {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<i64>,
    #[serde(default)]
    pub ref_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CodeReference {
    #[serde(default, alias = "from", alias = "source")]
    pub from_id: String,
    #[serde(default, alias = "to", alias = "target")]
    pub to_id: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DeadSymbol {
    #[serde(default)]
    pub name: String,
    /// `unused` | `broken`
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<i64>,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct IndexGraph {
    #[serde(default)]
    pub project: String,
    #[serde(default)]
    pub repo_path: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub node_count: u64,
    #[serde(default)]
    pub edge_count: u64,
    #[serde(default)]
    pub files: Option<u64>,
    #[serde(default)]
    pub nodes: Vec<AstNode>,
    #[serde(default)]
    pub references: Vec<CodeReference>,
    #[serde(default)]
    pub dead: Vec<DeadSymbol>,
}

impl IndexGraph {
    pub fn snapshot(&self) -> IndexSnapshot {
        let nodes = self.node_count.max(self.nodes.len() as u64);
        let edges = self.edge_count.max(self.references.len() as u64);
        IndexSnapshot {
            project: self.project.clone(),
            status: self.status.clone(),
            nodes,
            edges,
            files: self.files,
            dead: self.dead.len() as u64,
        }
    }
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
