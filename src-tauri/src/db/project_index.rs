use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use super::ExperienceStore;
use crate::models::{now_rfc3339, DeadSymbol, IndexGraph, IndexSnapshot};

pub(crate) fn migrate_project_index(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
            CREATE TABLE IF NOT EXISTS project_index (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL,
              repo_path TEXT NOT NULL,
              kind TEXT NOT NULL,
              name TEXT NOT NULL,
              file_path TEXT,
              line INTEGER,
              target TEXT,
              ref_count INTEGER NOT NULL DEFAULT 0,
              detail TEXT,
              payload_json TEXT NOT NULL DEFAULT '{}',
              indexed_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_project_index_project_kind
              ON project_index(project_id, kind);
            "#,
    )?;
    Ok(())
}

impl ExperienceStore {
    pub async fn save_project_index(&self, graph: IndexGraph) -> Result<IndexSnapshot> {
        let snapshot = graph.snapshot();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            save_project_index_blocking(&conn, &graph)
        })
        .await
        .context("project_index save join")??;
        Ok(snapshot)
    }

    pub async fn list_dead_symbols(&self, project_id: Option<String>) -> Result<Vec<DeadSymbol>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            list_dead_symbols_blocking(&conn, project_id.as_deref())
        })
        .await
        .context("project_index dead list join")?
    }

    pub async fn project_index_snapshot(
        &self,
        project_id: Option<String>,
    ) -> Result<IndexSnapshot> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            project_index_snapshot_blocking(&conn, project_id.as_deref())
        })
        .await
        .context("project_index snapshot join")?
    }
}

fn save_project_index_blocking(conn: &Connection, graph: &IndexGraph) -> Result<()> {
    let now = now_rfc3339();
    let project = if graph.project.is_empty() {
        "unknown"
    } else {
        graph.project.as_str()
    };
    conn.execute(
        "DELETE FROM project_index WHERE project_id = ?1",
        params![project],
    )?;

    for node in &graph.nodes {
        insert_row(
            conn,
            IndexRow {
                project_id: project,
                repo_path: &graph.repo_path,
                kind: "node",
                name: if node.name.is_empty() {
                    &node.id
                } else {
                    &node.name
                },
                file_path: node.file.as_deref(),
                line: node.line,
                target: Some(node.id.as_str()).filter(|id| !id.is_empty()),
                ref_count: node.ref_count as i64,
                detail: Some(node.kind.as_str()).filter(|kind| !kind.is_empty()),
                indexed_at: &now,
            },
        )?;
    }

    for edge in &graph.references {
        insert_row(
            conn,
            IndexRow {
                project_id: project,
                repo_path: &graph.repo_path,
                kind: "reference",
                name: &edge.from_id,
                file_path: edge.file.as_deref(),
                line: edge.line,
                target: Some(edge.to_id.as_str()),
                ref_count: 1,
                detail: None,
                indexed_at: &now,
            },
        )?;
    }

    for symbol in &graph.dead {
        let kind = if symbol.kind == "broken" {
            "broken"
        } else {
            "dead"
        };
        insert_row(
            conn,
            IndexRow {
                project_id: project,
                repo_path: &graph.repo_path,
                kind,
                name: &symbol.name,
                file_path: symbol.file.as_deref(),
                line: symbol.line,
                target: None,
                ref_count: 0,
                detail: symbol.detail.as_deref(),
                indexed_at: &now,
            },
        )?;
    }

    Ok(())
}

struct IndexRow<'a> {
    project_id: &'a str,
    repo_path: &'a str,
    kind: &'a str,
    name: &'a str,
    file_path: Option<&'a str>,
    line: Option<i64>,
    target: Option<&'a str>,
    ref_count: i64,
    detail: Option<&'a str>,
    indexed_at: &'a str,
}

fn insert_row(conn: &Connection, row: IndexRow<'_>) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO project_index (
            id, project_id, repo_path, kind, name, file_path, line, target,
            ref_count, detail, payload_json, indexed_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, '{}', ?11)
        "#,
        params![
            Uuid::new_v4().to_string(),
            row.project_id,
            row.repo_path,
            row.kind,
            row.name,
            row.file_path,
            row.line,
            row.target,
            row.ref_count,
            row.detail,
            row.indexed_at,
        ],
    )?;
    Ok(())
}

fn list_dead_symbols_blocking(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Vec<DeadSymbol>> {
    let sql = r#"
        SELECT project_id, name, kind, file_path, line, detail
        FROM project_index
        WHERE kind IN ('dead', 'broken')
          AND (?1 IS NULL OR project_id = ?1)
        ORDER BY kind, name
        "#;
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![project_id], |row| {
        let table_kind: String = row.get(2)?;
        let kind = if table_kind == "broken" {
            "broken"
        } else {
            "unused"
        };
        Ok(DeadSymbol {
            project_id: Some(row.get(0)?),
            name: row.get(1)?,
            kind: kind.into(),
            file: row.get(3)?,
            line: row.get(4)?,
            detail: row.get(5)?,
        })
    })?;
    let mut symbols = Vec::new();
    for row in rows {
        symbols.push(row?);
    }
    Ok(symbols)
}

fn project_index_snapshot_blocking(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<IndexSnapshot> {
    let project = match project_id {
        Some(id) => id.to_string(),
        None => conn
            .query_row(
                "SELECT project_id FROM project_index ORDER BY indexed_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or_default(),
    };
    if project.is_empty() {
        return Ok(IndexSnapshot::default());
    }

    let nodes: i64 = conn.query_row(
        "SELECT COUNT(*) FROM project_index WHERE project_id = ?1 AND kind = 'node'",
        params![project],
        |row| row.get(0),
    )?;
    let edges: i64 = conn.query_row(
        "SELECT COUNT(*) FROM project_index WHERE project_id = ?1 AND kind = 'reference'",
        params![project],
        |row| row.get(0),
    )?;
    let dead: i64 = conn.query_row(
        "SELECT COUNT(*) FROM project_index WHERE project_id = ?1 AND kind IN ('dead', 'broken')",
        params![project],
        |row| row.get(0),
    )?;
    let indexed_status: Option<String> = conn
        .query_row(
            "SELECT indexed_at FROM project_index WHERE project_id = ?1 LIMIT 1",
            params![project],
            |row| row.get(0),
        )
        .optional()?;

    Ok(IndexSnapshot {
        project,
        status: indexed_status.map(|_| "indexed".into()),
        nodes: nodes as u64,
        edges: edges as u64,
        files: None,
        dead: dead as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AstNode, CodeReference};

    fn sample_graph() -> IndexGraph {
        IndexGraph {
            project: "lounge".into(),
            repo_path: "/tmp/lounge".into(),
            status: Some("indexed".into()),
            node_count: 2,
            edge_count: 1,
            files: Some(2),
            nodes: vec![
                AstNode {
                    id: "foo".into(),
                    name: "foo".into(),
                    kind: "fn".into(),
                    file: Some("src/lib.rs".into()),
                    line: Some(10),
                    ref_count: 1,
                },
                AstNode {
                    id: "bar".into(),
                    name: "bar".into(),
                    kind: "fn".into(),
                    file: Some("src/dead.rs".into()),
                    line: Some(4),
                    ref_count: 0,
                },
            ],
            references: vec![CodeReference {
                from_id: "main".into(),
                to_id: "foo".into(),
                file: Some("src/main.rs".into()),
                line: Some(1),
            }],
            dead: vec![
                DeadSymbol {
                    name: "bar".into(),
                    kind: "unused".into(),
                    file: Some("src/dead.rs".into()),
                    line: Some(4),
                    detail: Some("gelen referans yok".into()),
                    project_id: Some("lounge".into()),
                },
                DeadSymbol {
                    name: "ghost".into(),
                    kind: "broken".into(),
                    file: Some("src/lib.rs".into()),
                    line: Some(12),
                    detail: Some("foo → ghost hedefi yok".into()),
                    project_id: Some("lounge".into()),
                },
            ],
        }
    }

    #[tokio::test]
    async fn save_and_list_dead_symbols_roundtrip() {
        let store = ExperienceStore::memory().expect("memory db");
        let snapshot = store
            .save_project_index(sample_graph())
            .await
            .expect("save");
        assert_eq!(snapshot.project, "lounge");
        assert_eq!(snapshot.nodes, 2);
        assert_eq!(snapshot.dead, 2);

        let dead = store
            .list_dead_symbols(Some("lounge".into()))
            .await
            .expect("list");
        assert_eq!(dead.len(), 2);
        assert!(dead
            .iter()
            .any(|row| row.kind == "unused" && row.name == "bar"));
        assert!(dead
            .iter()
            .any(|row| row.kind == "broken" && row.name == "ghost"));

        let listed = store
            .project_index_snapshot(Some("lounge".into()))
            .await
            .expect("snapshot");
        assert_eq!(listed.dead, 2);
        assert_eq!(listed.nodes, 2);
        assert_eq!(listed.edges, 1);

        store
            .save_project_index(IndexGraph {
                project: "lounge".into(),
                repo_path: "/tmp/lounge".into(),
                dead: vec![DeadSymbol {
                    name: "only".into(),
                    kind: "unused".into(),
                    ..DeadSymbol::default()
                }],
                ..IndexGraph::default()
            })
            .await
            .expect("replace");
        let again = store
            .list_dead_symbols(Some("lounge".into()))
            .await
            .unwrap();
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].name, "only");
    }
}
