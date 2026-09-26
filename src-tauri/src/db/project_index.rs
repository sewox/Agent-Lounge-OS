use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

use super::ExperienceStore;
use crate::models::{
    now_rfc3339, AstNode, CodeReference, DeadSymbol, IndexGraph, IndexSnapshot, ProjectSummary,
    SemanticMap, SemanticProject,
};

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
            CREATE TABLE IF NOT EXISTS ignored_symbols (
              id TEXT PRIMARY KEY,
              project_id TEXT,
              name TEXT NOT NULL,
              kind TEXT,
              file_path TEXT,
              line INTEGER,
              ignored_at TEXT NOT NULL,
              UNIQUE(project_id, name, file_path, line)
            );
            "#,
    )?;
    Ok(())
}

/// Normalize raw path strings for cross-platform comparison (PATH-01).
pub fn normalize_path_str(raw: &str) -> String {
    normalize_path_hint(raw).to_string_lossy().replace('\\', "/")
}

/// True when `path` starts with a Windows drive letter (`C:` / `D:/` …).
pub fn path_has_windows_drive(path: &str) -> bool {
    let mut chars = path.trim().chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(letter), Some(':')) if letter.is_ascii_alphabetic()
    )
}

/// True when the string looks like a filesystem path (`/`, `\`, or drive letter).
pub fn accepts_cross_platform_path(path: &str) -> bool {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return false;
    }
    trimmed.contains('/') || trimmed.contains('\\') || path_has_windows_drive(trimmed)
}

const PLACEHOLDER_PROJECTS: &[&str] = &["backend-legacy", "frontend-new"];

fn is_placeholder_project(name: &str) -> bool {
    PLACEHOLDER_PROJECTS
        .iter()
        .any(|p| p.eq_ignore_ascii_case(name.trim()))
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

    pub async fn list_indexed_projects(&self) -> Result<Vec<ProjectSummary>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            list_indexed_projects_blocking(&conn)
        })
        .await
        .context("project_index list join")?
    }

    /// `active_file` / `workspace_root` / açık path → `project_index.repo_path` eşlemesi.
    /// En uzun eşleşen kök kazanır; yoksa `None`.
    pub async fn resolve_project_id(&self, path_hint: impl Into<String>) -> Result<Option<String>> {
        let path_hint = path_hint.into();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            resolve_project_id_blocking(&conn, &path_hint)
        })
        .await
        .context("project_index resolve join")?
    }

    pub async fn load_semantic_map(&self, project_id: Option<String>) -> Result<SemanticMap> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            load_semantic_map_blocking(&conn, project_id.as_deref())
        })
        .await
        .context("project_index semantic map join")?
    }

    /// Command Palette: proje indeksinde ucuz isim/dosya araması (memory_bridge SQLite).
    pub async fn search_index_nodes(
        &self,
        query: String,
        limit: Option<usize>,
    ) -> Result<Vec<AstNode>> {
        let conn = self.conn.clone();
        let lim = limit.unwrap_or(8).max(1);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            search_index_nodes_blocking(&conn, &query, lim)
        })
        .await
        .context("project_index search join")?
    }

    pub async fn ignore_symbol(&self, symbol: DeadSymbol) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            ignore_symbol_blocking(&conn, &symbol)
        })
        .await
        .context("ignore_symbol join")?
    }

    pub async fn unignore_symbol(&self, symbol: DeadSymbol) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            unignore_symbol_blocking(&conn, &symbol)
        })
        .await
        .context("unignore_symbol join")?
    }

    pub async fn list_ignored_symbols(
        &self,
        project_id: Option<String>,
    ) -> Result<Vec<DeadSymbol>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            list_ignored_symbols_blocking(&conn, project_id.as_deref())
        })
        .await
        .context("list_ignored_symbols join")?
    }
}

fn resolve_project_id_blocking(conn: &Connection, path_hint: &str) -> Result<Option<String>> {
    let needle = normalize_path_hint(path_hint);
    if needle.as_os_str().is_empty() {
        return Ok(None);
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT DISTINCT project_id, repo_path, file_path
        FROM project_index
        WHERE TRIM(repo_path) != '' OR (file_path IS NOT NULL AND TRIM(file_path) != '')
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;

    let mut best: Option<(usize, String)> = None;
    for row in rows {
        let (project_id, repo_path, file_path) = row?;
        let candidates = [
            Some(repo_path.as_str()),
            file_path.as_deref().filter(|s| !s.trim().is_empty()),
        ];
        for candidate in candidates.into_iter().flatten() {
            let root = normalize_path_hint(candidate);
            if root.as_os_str().is_empty() {
                continue;
            }
            if path_is_within(&needle, &root) {
                let score = root.as_os_str().len();
                if best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
                    best = Some((score, project_id.clone()));
                }
            }
        }
    }
    Ok(best.map(|(_, id)| id))
}

fn normalize_path_hint(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return PathBuf::new();
    }
    // PATH-01: unify Windows `\` and drive-letter paths before lexical normalize.
    let unified = trimmed.replace('\\', "/");
    let path = PathBuf::from(&unified);
    if let Ok(canon) = path.canonicalize() {
        return canon;
    }
    // Henüz var olmayan / test path'leri için lexical normalize.
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                out.push(prefix.as_os_str());
            }
            Component::RootDir => {
                out.push(Component::RootDir.as_os_str());
            }
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn path_is_within(path: &Path, root: &Path) -> bool {
    if path == root {
        return true;
    }
    path.starts_with(root)
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
          AND NOT EXISTS (
            SELECT 1 FROM ignored_symbols i
            WHERE i.name = project_index.name
              AND COALESCE(i.project_id, '') = COALESCE(project_index.project_id, '')
              AND COALESCE(i.file_path, '') = COALESCE(project_index.file_path, '')
              AND COALESCE(i.line, -1) = COALESCE(project_index.line, -1)
          )
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

fn ignore_symbol_blocking(conn: &Connection, symbol: &DeadSymbol) -> Result<()> {
    // SQLite UNIQUE treats NULLs as distinct — delete then insert for stable upsert.
    let _ = unignore_symbol_blocking(conn, symbol);
    let id = Uuid::new_v4().to_string();
    let now = now_rfc3339();
    let kind = if symbol.kind.trim().is_empty() {
        None
    } else {
        Some(symbol.kind.as_str())
    };
    conn.execute(
        r#"
        INSERT INTO ignored_symbols (id, project_id, name, kind, file_path, line, ignored_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        "#,
        params![
            id,
            symbol.project_id,
            symbol.name,
            kind,
            symbol.file,
            symbol.line,
            now,
        ],
    )?;
    Ok(())
}

fn unignore_symbol_blocking(conn: &Connection, symbol: &DeadSymbol) -> Result<()> {
    conn.execute(
        r#"
        DELETE FROM ignored_symbols
        WHERE name = ?1
          AND COALESCE(project_id, '') = COALESCE(?2, '')
          AND COALESCE(file_path, '') = COALESCE(?3, '')
          AND COALESCE(line, -1) = COALESCE(?4, -1)
        "#,
        params![
            symbol.name,
            symbol.project_id,
            symbol.file,
            symbol.line,
        ],
    )?;
    Ok(())
}

fn list_ignored_symbols_blocking(
    conn: &Connection,
    project_id: Option<&str>,
) -> Result<Vec<DeadSymbol>> {
    let sql = r#"
        SELECT project_id, name, kind, file_path, line
        FROM ignored_symbols
        WHERE ?1 IS NULL OR project_id = ?1
        ORDER BY ignored_at DESC, name
        "#;
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![project_id], |row| {
        Ok(DeadSymbol {
            project_id: row.get(0)?,
            name: row.get(1)?,
            kind: row
                .get::<_, Option<String>>(2)?
                .unwrap_or_else(|| "unused".into()),
            file: row.get(3)?,
            line: row.get(4)?,
            detail: Some("ignored".into()),
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
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
    let files: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT NULLIF(file_path, '')) FROM project_index WHERE project_id = ?1",
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
        files: Some(files as u64),
        dead: dead as u64,
    })
}

fn list_indexed_projects_blocking(conn: &Connection) -> Result<Vec<ProjectSummary>> {
    let sql = r#"
        SELECT
            project_id,
            MAX(repo_path),
            SUM(CASE WHEN kind = 'node' THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind = 'reference' THEN 1 ELSE 0 END),
            COUNT(DISTINCT NULLIF(file_path, ''))
        FROM project_index
        GROUP BY project_id
        ORDER BY MAX(indexed_at) DESC, project_id
        "#;
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(ProjectSummary {
            name: row.get(0)?,
            root_path: row
                .get::<_, Option<String>>(1)?
                .filter(|path| !path.is_empty()),
            nodes: row.get::<_, i64>(2)? as u64,
            edges: row.get::<_, i64>(3)? as u64,
            files: Some(row.get::<_, i64>(4)? as u64),
        })
    })?;
    let mut projects = Vec::new();
    for row in rows {
        let project = row?;
        if is_placeholder_project(&project.name) {
            continue;
        }
        projects.push(project);
    }
    Ok(projects)
}

fn search_index_nodes_blocking(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> Result<Vec<AstNode>> {
    let needle = query.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let like = format!("%{needle}%");
    let sql = r#"
        SELECT name, kind, file_path, line, ref_count
        FROM project_index
        WHERE kind = 'node'
          AND (
            lower(name) LIKE ?1
            OR lower(COALESCE(file_path, '')) LIKE ?1
            OR lower(COALESCE(detail, '')) LIKE ?1
          )
        ORDER BY ref_count DESC, name
        LIMIT ?2
        "#;
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(params![like, limit as i64], |row| {
        Ok(AstNode {
            id: format!(
                "{}:{}",
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(2)?.unwrap_or_default()
            ),
            name: row.get(0)?,
            kind: row.get(1)?,
            file: row.get(2)?,
            line: row.get(3)?,
            ref_count: row.get::<_, i64>(4)? as u64,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn load_semantic_map_blocking(conn: &Connection, project_id: Option<&str>) -> Result<SemanticMap> {
    let mut by_project: BTreeMap<String, SemanticProject> = BTreeMap::new();
    {
        let sql = r#"
            SELECT project_id, repo_path, kind, name, file_path, line, target, ref_count, detail
            FROM project_index
            WHERE ?1 IS NULL OR project_id = ?1
            ORDER BY project_id, kind, name
            "#;
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![project_id], |row| {
            Ok(MapRow {
                project_id: row.get(0)?,
                repo_path: row.get(1)?,
                kind: row.get(2)?,
                name: row.get(3)?,
                file_path: row.get(4)?,
                line: row.get(5)?,
                target: row.get(6)?,
                ref_count: row.get(7)?,
                detail: row.get(8)?,
            })
        })?;

        for row in rows {
            let row = row?;
            let project = by_project
                .entry(row.project_id.clone())
                .or_insert_with(|| SemanticProject {
                    name: row.project_id.clone(),
                    repo_path: row.repo_path.clone(),
                    ..SemanticProject::default()
                });
            if project.repo_path.is_empty() && !row.repo_path.is_empty() {
                project.repo_path = row.repo_path.clone();
            }
            match row.kind.as_str() {
                "node" => {
                    let id = row
                        .target
                        .clone()
                        .filter(|id| !id.is_empty())
                        .unwrap_or_else(|| row.name.clone());
                    project.nodes.push(AstNode {
                        id,
                        name: row.name,
                        kind: row.detail.unwrap_or_default(),
                        file: row.file_path,
                        line: row.line,
                        ref_count: row.ref_count.max(0) as u64,
                    });
                }
                "reference" => {
                    project.references.push(CodeReference {
                        from_id: row.name,
                        to_id: row.target.unwrap_or_default(),
                        file: row.file_path,
                        line: row.line,
                    });
                }
                "dead" | "broken" => {
                    project.dead.push(DeadSymbol {
                        name: row.name,
                        kind: if row.kind == "broken" {
                            "broken".into()
                        } else {
                            "unused".into()
                        },
                        file: row.file_path,
                        line: row.line,
                        detail: row.detail,
                        project_id: Some(row.project_id),
                    });
                }
                _ => {}
            }
        }
    }

    let projects = by_project
        .into_values()
        .filter(|project| !is_placeholder_project(&project.name))
        .map(|mut project| {
            // Prefer DB totals so later LIMIT truncations cannot rewrite counts.
            let node_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM project_index WHERE project_id = ?1 AND kind = 'node'",
                    params![project.name],
                    |row| row.get(0),
                )
                .unwrap_or(project.nodes.len() as i64);
            let edge_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM project_index WHERE project_id = ?1 AND kind = 'reference'",
                    params![project.name],
                    |row| row.get(0),
                )
                .unwrap_or(project.references.len() as i64);
            project.node_count = node_count.max(0) as u64;
            project.edge_count = edge_count.max(0) as u64;
            let mut files = HashSet::new();
            for node in &project.nodes {
                if let Some(file) = node.file.as_deref().filter(|path| !path.is_empty()) {
                    files.insert(file.to_string());
                }
            }
            for edge in &project.references {
                if let Some(file) = edge.file.as_deref().filter(|path| !path.is_empty()) {
                    files.insert(file.to_string());
                }
            }
            project.files = files.len() as u64;
            project
        })
        .collect();
    Ok(SemanticMap::from_projects(projects))
}

struct MapRow {
    project_id: String,
    repo_path: String,
    kind: String,
    name: String,
    file_path: Option<String>,
    line: Option<i64>,
    target: Option<String>,
    ref_count: i64,
    detail: Option<String>,
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
        assert_eq!(listed.files, Some(3));

        let projects = store.list_indexed_projects().await.expect("list indexed");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].name, "lounge");
        assert_eq!(projects[0].root_path.as_deref(), Some("/tmp/lounge"));
        assert_eq!(projects[0].nodes, 2);
        assert_eq!(projects[0].edges, 1);
        assert_eq!(projects[0].files, Some(3));

        let map = store
            .load_semantic_map(Some("lounge".into()))
            .await
            .expect("map");
        assert_eq!(map.projects.len(), 1);
        assert_eq!(map.projects[0].nodes.len(), 2);
        assert_eq!(map.projects[0].references.len(), 1);
        assert_eq!(map.projects[0].dead.len(), 2);
        assert_eq!(map.projects[0].files, 3);
        assert!(map.projects[0]
            .nodes
            .iter()
            .any(|node| node.name == "foo" && node.ref_count == 1));

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

    #[tokio::test]
    async fn ignore_symbol_filters_dead_list() {
        let store = ExperienceStore::memory().expect("memory db");
        store
            .save_project_index(sample_graph())
            .await
            .expect("save");
        let dead = store
            .list_dead_symbols(Some("lounge".into()))
            .await
            .expect("list");
        assert_eq!(dead.len(), 2);
        let target = dead
            .iter()
            .find(|row| row.name == "bar")
            .cloned()
            .expect("bar");
        store.ignore_symbol(target.clone()).await.expect("ignore");
        let after = store
            .list_dead_symbols(Some("lounge".into()))
            .await
            .expect("list after");
        assert_eq!(after.len(), 1);
        assert!(!after.iter().any(|row| row.name == "bar"));
        let ignored = store
            .list_ignored_symbols(Some("lounge".into()))
            .await
            .expect("ignored");
        assert_eq!(ignored.len(), 1);
        assert_eq!(ignored[0].name, "bar");
        store.unignore_symbol(target).await.expect("unignore");
        assert_eq!(
            store
                .list_dead_symbols(Some("lounge".into()))
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn path_helpers_accept_windows_and_posix() {
        assert!(path_has_windows_drive(r"C:\Users\x\file.rs"));
        assert!(path_has_windows_drive("D:/work/repo"));
        assert!(!path_has_windows_drive("/home/x/file.rs"));
        assert!(accepts_cross_platform_path(
            r"C:\Users\sercan\dev\Agent-Lounge-OS\src\main.rs"
        ));
        assert!(accepts_cross_platform_path(
            "/home/sercan/dev/Agent-Lounge-OS/src/main.rs"
        ));
        assert!(accepts_cross_platform_path(r"mixed/path\with\both"));
        assert!(!accepts_cross_platform_path("no-separators"));
        let normalized = normalize_path_str(r"C:\Users\sercan\dev\..\Agent-Lounge-OS\src\main.rs");
        assert!(normalized.contains("Agent-Lounge-OS"));
        assert!(!normalized.contains('\\'));
    }
}
