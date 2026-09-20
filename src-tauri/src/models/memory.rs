use std::collections::{HashMap, HashSet};

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
    pub fn unique_file_count(&self) -> u64 {
        let mut files = HashSet::new();
        for node in &self.nodes {
            if let Some(file) = node.file.as_deref().filter(|path| !path.is_empty()) {
                files.insert(file);
            }
        }
        for edge in &self.references {
            if let Some(file) = edge.file.as_deref().filter(|path| !path.is_empty()) {
                files.insert(file);
            }
        }
        for symbol in &self.dead {
            if let Some(file) = symbol.file.as_deref().filter(|path| !path.is_empty()) {
                files.insert(file);
            }
        }
        files.len() as u64
    }

    pub fn snapshot(&self) -> IndexSnapshot {
        let nodes = self.node_count.max(self.nodes.len() as u64);
        let edges = self.edge_count.max(self.references.len() as u64);
        IndexSnapshot {
            project: self.project.clone(),
            status: self.status.clone(),
            nodes,
            edges,
            files: Some(self.files.unwrap_or(0).max(self.unique_file_count())),
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
    #[serde(default)]
    pub files: Option<u64>,
}

impl ProjectSummary {
    pub fn merge_key(&self) -> String {
        self.root_path
            .as_deref()
            .filter(|path| !path.is_empty())
            .unwrap_or(self.name.as_str())
            .to_string()
    }
}

/// SQLite indeks kaydı CBM `list_projects` üzerine yazılır; aynı repo tek satır kalır.
pub fn merge_project_summaries(
    indexed: Vec<ProjectSummary>,
    discovered: Vec<ProjectSummary>,
) -> Vec<ProjectSummary> {
    let mut by_key: HashMap<String, ProjectSummary> = HashMap::new();
    for row in discovered {
        by_key.insert(row.merge_key(), row);
    }
    for row in indexed {
        by_key.insert(row.merge_key(), row);
    }
    let mut rows: Vec<_> = by_key.into_values().collect();
    rows.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.root_path.cmp(&right.root_path))
    });
    rows
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProjectList {
    #[serde(default)]
    pub projects: Vec<ProjectSummary>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_prefers_larger_file_count() {
        let graph = IndexGraph {
            project: "lounge".into(),
            files: Some(2),
            nodes: vec![
                AstNode {
                    file: Some("a.rs".into()),
                    ..AstNode::default()
                },
                AstNode {
                    file: Some("b.rs".into()),
                    ..AstNode::default()
                },
            ],
            references: vec![CodeReference {
                file: Some("c.rs".into()),
                ..CodeReference::default()
            }],
            ..IndexGraph::default()
        };
        assert_eq!(graph.unique_file_count(), 3);
        assert_eq!(graph.snapshot().files, Some(3));
    }

    #[test]
    fn merge_project_summaries_indexes_overwrite_discovered() {
        let indexed = vec![ProjectSummary {
            name: "lounge".into(),
            root_path: Some("/tmp/lounge".into()),
            nodes: 12,
            edges: 4,
            files: Some(8),
        }];
        let discovered = vec![
            ProjectSummary {
                name: "lounge".into(),
                root_path: Some("/tmp/lounge".into()),
                nodes: 1,
                edges: 0,
                files: None,
            },
            ProjectSummary {
                name: "other".into(),
                root_path: Some("/tmp/other".into()),
                nodes: 3,
                edges: 1,
                files: Some(2),
            },
        ];
        let merged = merge_project_summaries(indexed, discovered);
        assert_eq!(merged.len(), 2);
        let lounge = merged.iter().find(|row| row.name == "lounge").unwrap();
        assert_eq!(lounge.nodes, 12);
        assert_eq!(lounge.files, Some(8));
    }
}
