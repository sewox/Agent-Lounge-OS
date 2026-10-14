//! Canonical project identity (Project-ID-Core).
//!
//! **Canonical ID** = dunce-style canonicalized `repo_path` (symlinks resolved;
//! Windows `\\?\` stripped; case-folded on macOS/Windows).
//!
//! **Aliases** map experience/CBM slugs (e.g. `agent-lounge-os`, `photo-culling`)
//! onto that canonical id.
//!
//! **Sentinels**
//! - `unclassified` — MCP write supplied a free-form slug that matches no alias
//!   and no known canonical path.
//! - `unindexed_placeholder` — path hint given but no indexed/discovered repo
//!   matched (project has no usable repo path in the index yet).
//!
//! Display name priority: Lounge project name → repo_path basename → slug.

use std::collections::HashSet;
use std::path::{Component, PathBuf};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use super::project_index::accepts_cross_platform_path;

/// Free-form slug that matched nothing known.
pub const PROJECT_ID_UNCLASSIFIED: &str = "unclassified";
/// Path hint present but no `project_index` / alias repo match.
pub const PROJECT_ID_UNINDEXED_PLACEHOLDER: &str = "unindexed_placeholder";

/// Settings: one-shot remap of experiences + project_index → canonical ids.
pub const MIGRATION_PROJECT_ID_CANONICAL_V1: &str = "migrations.project_id_canonical_v1";
/// JSON backup of `{ "remaps": [ {"from","to"}… ] }` for reverse notes.
pub const MIGRATION_PROJECT_ID_CANONICAL_V1_BACKUP: &str =
    "migrations.project_id_canonical_v1.backup";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectBindKind {
    Canonical,
    Unclassified,
    UnindexedPlaceholder,
}

impl ProjectBindKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Canonical => "canonical",
            Self::Unclassified => "unclassified",
            Self::UnindexedPlaceholder => "unindexed_placeholder",
        }
    }

    pub fn warning(self) -> Option<&'static str> {
        match self {
            Self::Canonical => None,
            Self::Unclassified => Some(
                "project_id could not be matched to a known canonical repo path; stored as unclassified",
            ),
            Self::UnindexedPlaceholder => Some(
                "no indexed repo_path matched; stored under unindexed_placeholder",
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectBind {
    pub canonical_id: String,
    pub kind: ProjectBindKind,
    pub display_name: String,
}

/// Create identity tables (idempotent).
pub fn migrate_project_identity(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS projects (
          canonical_id TEXT PRIMARY KEY,
          display_name TEXT NOT NULL,
          repo_path TEXT,
          kind TEXT NOT NULL DEFAULT 'indexed'
        );
        CREATE TABLE IF NOT EXISTS project_aliases (
          alias TEXT PRIMARY KEY COLLATE NOCASE,
          canonical_id TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_project_aliases_canonical
          ON project_aliases(canonical_id);
        "#,
    )?;
    ensure_sentinel_projects(conn)?;
    Ok(())
}

fn ensure_sentinel_projects(conn: &Connection) -> Result<()> {
    for (id, name, kind) in [
        (PROJECT_ID_UNCLASSIFIED, "Unclassified", "unclassified"),
        (
            PROJECT_ID_UNINDEXED_PLACEHOLDER,
            "Unindexed",
            "unindexed_placeholder",
        ),
    ] {
        conn.execute(
            r#"
            INSERT INTO projects(canonical_id, display_name, repo_path, kind)
            VALUES (?1, ?2, NULL, ?3)
            ON CONFLICT(canonical_id) DO NOTHING
            "#,
            params![id, name, kind],
        )?;
        upsert_alias(conn, id, id)?;
        upsert_alias(conn, name, id)?;
    }
    Ok(())
}

/// Strip Windows extended-length prefix (`\\?\` / `//?/`) for local drives only.
pub fn strip_windows_extended_prefix(raw: &str) -> String {
    let t = raw.trim();
    if let Some(rest) = t.strip_prefix(r"\\?\").or_else(|| t.strip_prefix("//?/")) {
        let unc = rest.len() >= 4
            && (rest[..4].eq_ignore_ascii_case(r"UNC\") || rest[..4].eq_ignore_ascii_case("UNC/"));
        if unc {
            return t.to_string();
        }
        return rest.to_string();
    }
    t.to_string()
}

/// Canonical repo path identity string (Project-ID-Core).
///
/// - Resolves symlinks when the path exists
/// - Dunce-style: no `\\?\` prefix on Windows local paths
/// - Forward-slash separators
/// - Case-folded on macOS and Windows (case-insensitive volumes)
pub fn canonicalize_repo_path(raw: &str) -> String {
    let stripped = strip_windows_extended_prefix(raw);
    let trimmed = stripped.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let unified = trimmed.replace('\\', "/");
    let path = PathBuf::from(&unified);
    let resolved = match path.canonicalize() {
        Ok(canon) => {
            let s = canon.to_string_lossy().into_owned();
            strip_windows_extended_prefix(&s).replace('\\', "/")
        }
        Err(_) => lexical_normalize(&unified),
    };
    fold_identity_case(&resolved)
}

fn lexical_normalize(unified: &str) -> String {
    let path = PathBuf::from(unified);
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(Component::RootDir.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out.to_string_lossy().replace('\\', "/")
}

fn fold_identity_case(path: &str) -> String {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        path.to_ascii_lowercase()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        path.to_string()
    }
}

fn path_basename(path: &str) -> String {
    path.replace('\\', "/")
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_string()
}

/// displayName = Lounge project name > repo_path basename > slug
pub fn choose_display_name(
    lounge_name: Option<&str>,
    repo_path: Option<&str>,
    slug: Option<&str>,
) -> String {
    if let Some(n) = lounge_name.map(str::trim).filter(|s| !s.is_empty()) {
        return n.to_string();
    }
    if let Some(p) = repo_path.map(str::trim).filter(|s| !s.is_empty()) {
        let base = path_basename(p);
        if !base.is_empty() {
            return base;
        }
    }
    if let Some(s) = slug.map(str::trim).filter(|s| !s.is_empty()) {
        return s.to_string();
    }
    "unknown".into()
}

pub fn upsert_alias(conn: &Connection, alias: &str, canonical_id: &str) -> Result<()> {
    let alias = alias.trim();
    if alias.is_empty() || canonical_id.trim().is_empty() {
        return Ok(());
    }
    conn.execute(
        r#"
        INSERT INTO project_aliases(alias, canonical_id) VALUES (?1, ?2)
        ON CONFLICT(alias) DO UPDATE SET canonical_id = excluded.canonical_id
        "#,
        params![alias, canonical_id],
    )?;
    Ok(())
}

pub fn upsert_project(
    conn: &Connection,
    canonical_id: &str,
    display_name: &str,
    repo_path: Option<&str>,
    kind: &str,
) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO projects(canonical_id, display_name, repo_path, kind)
        VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(canonical_id) DO UPDATE SET
          display_name = excluded.display_name,
          repo_path = COALESCE(excluded.repo_path, projects.repo_path),
          kind = excluded.kind
        "#,
        params![canonical_id, display_name, repo_path, kind],
    )?;
    Ok(())
}

/// Register a path-backed project and slug/CBM aliases → canonical id.
pub fn register_path_project(
    conn: &Connection,
    lounge_name: &str,
    repo_path: &str,
    extra_aliases: &[&str],
) -> Result<String> {
    migrate_project_identity(conn)?;
    let canonical = canonicalize_repo_path(repo_path);
    if canonical.is_empty() {
        anyhow::bail!("repo_path empty after canonicalize");
    }
    let display = choose_display_name(Some(lounge_name), Some(repo_path), Some(lounge_name));
    upsert_project(conn, &canonical, &display, Some(&canonical), "indexed")?;
    // Self-alias + display + lounge name + basename + extras (CBM slugs, legacy folder names).
    upsert_alias(conn, &canonical, &canonical)?;
    upsert_alias(conn, &display, &canonical)?;
    if !lounge_name.trim().is_empty() {
        upsert_alias(conn, lounge_name, &canonical)?;
    }
    let base = path_basename(&canonical);
    if !base.is_empty() {
        upsert_alias(conn, &base, &canonical)?;
    }
    let raw_base = path_basename(repo_path);
    if !raw_base.is_empty() {
        upsert_alias(conn, &raw_base, &canonical)?;
    }
    for alias in extra_aliases {
        upsert_alias(conn, alias, &canonical)?;
    }
    Ok(canonical)
}

pub fn lookup_alias(conn: &Connection, alias: &str) -> Result<Option<String>> {
    let alias = alias.trim();
    if alias.is_empty() {
        return Ok(None);
    }
    let row: Option<String> = conn
        .query_row(
            "SELECT canonical_id FROM project_aliases WHERE alias = ?1 COLLATE NOCASE",
            params![alias],
            |r| r.get(0),
        )
        .optional()?;
    Ok(row)
}

pub fn lookup_display_name(conn: &Connection, canonical_id: &str) -> Result<Option<String>> {
    let row: Option<String> = conn
        .query_row(
            "SELECT display_name FROM projects WHERE canonical_id = ?1",
            params![canonical_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(row)
}

/// Bind an MCP write candidate to a canonical project id.
///
/// Rules:
/// 1. Alias / known canonical hit → that id
/// 2. Path-shaped → longest `project_index` root match → that project's canonical id
/// 3. Path-shaped, no match → `unindexed_placeholder`
/// 4. Free-form slug, no match → `unclassified`
pub fn bind_write_project_id(
    conn: &Connection,
    explicit: Option<&str>,
    path_hints: &[&str],
    resolve_path: impl Fn(&str) -> Result<Option<String>>,
) -> Result<Option<ProjectBind>> {
    migrate_project_identity(conn)?;

    if let Some(raw) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(bind) = resolve_known(conn, raw)? {
            return Ok(Some(bind));
        }
        if accepts_cross_platform_path(raw) {
            if let Some(matched) = resolve_path(raw)? {
                return Ok(Some(bind_from_matched(conn, &matched)?));
            }
            return Ok(Some(sentinel_bind(
                PROJECT_ID_UNINDEXED_PLACEHOLDER,
                ProjectBindKind::UnindexedPlaceholder,
            )));
        }
        return Ok(Some(sentinel_bind(
            PROJECT_ID_UNCLASSIFIED,
            ProjectBindKind::Unclassified,
        )));
    }

    for hint in path_hints {
        let hint = hint.trim();
        if hint.is_empty() {
            continue;
        }
        if let Some(matched) = resolve_path(hint)? {
            return Ok(Some(bind_from_matched(conn, &matched)?));
        }
        if accepts_cross_platform_path(hint) {
            return Ok(Some(sentinel_bind(
                PROJECT_ID_UNINDEXED_PLACEHOLDER,
                ProjectBindKind::UnindexedPlaceholder,
            )));
        }
    }
    Ok(None)
}

fn resolve_known(conn: &Connection, raw: &str) -> Result<Option<ProjectBind>> {
    if let Some(canonical) = lookup_alias(conn, raw)? {
        return Ok(Some(bind_from_matched(conn, &canonical)?));
    }
    // Already a canonical id row?
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM projects WHERE canonical_id = ?1 LIMIT 1",
            params![raw],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if exists {
        return Ok(Some(bind_from_matched(conn, raw)?));
    }
    // Path-shaped explicit: try canonicalize equality against projects.repo_path
    if accepts_cross_platform_path(raw) {
        let needle = canonicalize_repo_path(raw);
        if !needle.is_empty() {
            let hit: Option<String> = conn
                .query_row(
                    "SELECT canonical_id FROM projects WHERE repo_path = ?1 LIMIT 1",
                    params![needle],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(id) = hit {
                return Ok(Some(bind_from_matched(conn, &id)?));
            }
        }
    }
    Ok(None)
}

fn bind_from_matched(conn: &Connection, canonical_id: &str) -> Result<ProjectBind> {
    if canonical_id.eq_ignore_ascii_case(PROJECT_ID_UNCLASSIFIED) {
        return Ok(sentinel_bind(
            PROJECT_ID_UNCLASSIFIED,
            ProjectBindKind::Unclassified,
        ));
    }
    if canonical_id.eq_ignore_ascii_case(PROJECT_ID_UNINDEXED_PLACEHOLDER) {
        return Ok(sentinel_bind(
            PROJECT_ID_UNINDEXED_PLACEHOLDER,
            ProjectBindKind::UnindexedPlaceholder,
        ));
    }
    let display =
        lookup_display_name(conn, canonical_id)?.unwrap_or_else(|| path_basename(canonical_id));
    Ok(ProjectBind {
        canonical_id: canonical_id.to_string(),
        kind: ProjectBindKind::Canonical,
        display_name: display,
    })
}

fn sentinel_bind(id: &str, kind: ProjectBindKind) -> ProjectBind {
    let display = match kind {
        ProjectBindKind::Unclassified => "Unclassified",
        ProjectBindKind::UnindexedPlaceholder => "Unindexed",
        ProjectBindKind::Canonical => id,
    };
    ProjectBind {
        canonical_id: id.to_string(),
        kind,
        display_name: display.into(),
    }
}

/// One-shot: rebuild aliases from `project_index`, rewrite experiences + index
/// rows to canonical ids. Idempotent via settings key. Does **not** re-file
/// content across projects (e.g. will not move photo-culling rows onto another
/// repo's canonical id). Backup JSON stored for reverse notes.
pub fn migrate_project_ids_to_canonical(conn: &Connection) -> Result<u64> {
    migrate_project_identity(conn)?;
    let done: Option<String> = conn
        .query_row(
            "SELECT value_json FROM settings WHERE key = ?1",
            params![MIGRATION_PROJECT_ID_CANONICAL_V1],
            |row| row.get(0),
        )
        .optional()?;
    if matches!(done.as_deref(), Some("1") | Some("true") | Some("\"1\"")) {
        return Ok(0);
    }

    // Distinct project_id → repo_path from index.
    let mut stmt = conn.prepare(
        r#"
        SELECT project_id, MAX(repo_path)
        FROM project_index
        GROUP BY project_id
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;

    let mut remaps: Vec<(String, String)> = Vec::new();
    let mut seen_from: HashSet<String> = HashSet::new();

    for row in rows {
        let (old_id, repo_path) = row?;
        let repo = repo_path.unwrap_or_default();
        let repo_trim = repo.trim();
        if repo_trim.is_empty() {
            // Pathless index row → keep id, mark unindexed, alias to self.
            upsert_project(
                conn,
                &old_id,
                &choose_display_name(Some(&old_id), None, Some(&old_id)),
                None,
                "unindexed_placeholder",
            )?;
            upsert_alias(conn, &old_id, &old_id)?;
            continue;
        }
        let canonical = register_path_project(conn, &old_id, repo_trim, &[])?;
        if canonical != old_id && seen_from.insert(old_id.clone()) {
            remaps.push((old_id, canonical));
        }
    }

    // Experiences-only project ids (no index): keep slug, register as unindexed.
    if table_exists(conn, "experiences")? {
        let mut exp_stmt = conn.prepare(
            r#"
            SELECT DISTINCT project_id FROM experiences
            WHERE TRIM(project_id) != ''
            "#,
        )?;
        let exp_rows = exp_stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in exp_rows {
            let old_id = row?;
            if lookup_alias(conn, &old_id)?.is_some() {
                continue;
            }
            upsert_project(
                conn,
                &old_id,
                &choose_display_name(Some(&old_id), None, Some(&old_id)),
                None,
                "unindexed_placeholder",
            )?;
            upsert_alias(conn, &old_id, &old_id)?;
        }
    }

    let mut touched = 0u64;
    for (from, to) in &remaps {
        touched += rewrite_project_id(conn, from, to)?;
    }

    let backup = serde_json::json!({
        "remaps": remaps.iter().map(|(f, t)| json_remap(f, t)).collect::<Vec<_>>(),
        "note": "Reverse: UPDATE experiences/project_index SET project_id=from WHERE project_id=to for each remap; restore aliases from backup if needed. Does not re-file cross-project content."
    });
    conn.execute(
        "INSERT INTO settings(key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![MIGRATION_PROJECT_ID_CANONICAL_V1_BACKUP, backup.to_string()],
    )?;
    conn.execute(
        "INSERT INTO settings(key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![MIGRATION_PROJECT_ID_CANONICAL_V1, "1"],
    )?;
    Ok(touched)
}

fn json_remap(from: &str, to: &str) -> serde_json::Value {
    serde_json::json!({ "from": from, "to": to })
}

fn rewrite_project_id(conn: &Connection, from: &str, to: &str) -> Result<u64> {
    let mut n = 0u64;
    if table_exists(conn, "experiences")? {
        n += conn.execute(
            "UPDATE experiences SET project_id = ?1 WHERE project_id = ?2",
            params![to, from],
        )? as u64;
    }
    n += conn.execute(
        "UPDATE project_index SET project_id = ?1 WHERE project_id = ?2",
        params![to, from],
    )? as u64;
    // project_flags / ignored_symbols follow the same id space.
    if table_exists(conn, "project_flags")? {
        // Merge hidden flags onto canonical when both exist.
        let _ = conn.execute(
            r#"
            INSERT INTO project_flags(project_id, hidden)
            SELECT ?1, hidden FROM project_flags WHERE project_id = ?2
            ON CONFLICT(project_id) DO UPDATE SET
              hidden = MAX(project_flags.hidden, excluded.hidden)
            "#,
            params![to, from],
        );
        let _ = conn.execute(
            "DELETE FROM project_flags WHERE project_id = ?1",
            params![from],
        );
    }
    if table_exists(conn, "ignored_symbols")? {
        n += conn.execute(
            "UPDATE ignored_symbols SET project_id = ?1 WHERE project_id = ?2",
            params![to, from],
        )? as u64;
    }
    Ok(n)
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        params![name],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

/// Map a path-resolve hit (possibly legacy slug) through aliases when present.
pub fn prefer_canonical(conn: &Connection, resolved: &str) -> Result<String> {
    if let Some(c) = lookup_alias(conn, resolved)? {
        return Ok(c);
    }
    Ok(resolved.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{normalize_path_str, ExperienceStore};
    use crate::models::{AstNode, IndexGraph};
    use uuid::Uuid;

    #[test]
    fn strip_extended_prefix_local_only() {
        assert_eq!(
            strip_windows_extended_prefix(r"\\?\C:\Users\sercan\dev\Agent-Lounge-OS"),
            r"C:\Users\sercan\dev\Agent-Lounge-OS"
        );
        assert_eq!(
            strip_windows_extended_prefix(r"//?/C:/Users/sercan/dev/Agent-Lounge-OS"),
            "C:/Users/sercan/dev/Agent-Lounge-OS"
        );
        // UNC extended must stay intact (not dunce-stripped into a fake local path).
        let unc = r"\\?\UNC\server\share\repo";
        assert_eq!(strip_windows_extended_prefix(unc), unc);
    }

    #[test]
    fn canonicalize_folds_separators_and_dots() {
        let n = canonicalize_repo_path(r"C:\Users\sercan\dev\..\Agent-Lounge-OS");
        assert!(n.contains("Agent-Lounge-OS") || n.contains("agent-lounge-os"));
        assert!(!n.contains('\\'));
        assert!(!n.starts_with(r"\\?\"));
    }

    #[test]
    fn canonicalize_case_policy_matches_os() {
        let mixed = canonicalize_repo_path("/tmp/Agent-Lounge-OS-Case");
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            assert_eq!(mixed, mixed.to_ascii_lowercase());
        }
        #[cfg(target_os = "linux")]
        {
            assert!(
                mixed.contains("Agent-Lounge-OS-Case") || mixed.ends_with("Agent-Lounge-OS-Case")
            );
        }
    }

    #[test]
    fn display_name_priority() {
        assert_eq!(
            choose_display_name(Some("Lounge"), Some("/a/b/repo"), Some("slug")),
            "Lounge"
        );
        assert_eq!(
            choose_display_name(None, Some("/a/b/repo"), Some("slug")),
            "repo"
        );
        assert_eq!(choose_display_name(None, None, Some("slug")), "slug");
    }

    #[tokio::test]
    async fn migration_remaps_slug_to_canonical_and_is_idempotent() {
        let store = ExperienceStore::memory().unwrap();
        let root = std::env::temp_dir().join(format!("pid-core-mig-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let graph = IndexGraph {
            project: "photo-culling".into(),
            repo_path: root.to_string_lossy().into_owned(),
            status: None,
            node_count: 1,
            edge_count: 0,
            files: Some(1),
            nodes: vec![AstNode {
                id: "n".into(),
                name: "n".into(),
                kind: "fn".into(),
                file: Some(root.join("a.rs").to_string_lossy().into_owned()),
                line: Some(1),
                ref_count: 0,
            }],
            references: vec![],
            dead: vec![],
        };
        // Insert legacy-shaped rows: save_project_index already registers identity
        // in the new path — simulate pre-migration by writing raw SQL.
        {
            let conn = store.conn.lock().unwrap();
            conn.execute("DELETE FROM project_index", []).unwrap();
            conn.execute("DELETE FROM project_aliases", []).ok();
            conn.execute("DELETE FROM projects", []).ok();
            migrate_project_identity(&conn).unwrap();
            conn.execute(
                r#"
                INSERT INTO project_index (
                  id, project_id, repo_path, kind, name, file_path, line,
                  target, ref_count, detail, payload_json, indexed_at
                ) VALUES ('1','photo-culling',?1,'node','n',?2,1,NULL,0,NULL,'{}','2026-01-01T00:00:00Z')
                "#,
                params![
                    root.to_string_lossy().as_ref(),
                    root.join("a.rs").to_string_lossy().as_ref()
                ],
            )
            .unwrap();
            conn.execute(
                r#"
                INSERT INTO experiences (
                  id, project_id, agent_id, topic, solution_summary, adr_record,
                  outcome, tags_json, created_at, payload_json
                ) VALUES (
                  'exp-legacy', 'photo-culling', 'cursor', 't', 's', 'adr',
                  'success', '[]', '2026-01-01T00:00:00Z', '{}'
                )
                "#,
                [],
            )
            .unwrap();
            // Clear migration flag if save path set it.
            let _ = conn.execute(
                "DELETE FROM settings WHERE key = ?1 OR key = ?2",
                params![
                    MIGRATION_PROJECT_ID_CANONICAL_V1,
                    MIGRATION_PROJECT_ID_CANONICAL_V1_BACKUP
                ],
            );
            let touched = migrate_project_ids_to_canonical(&conn).unwrap();
            assert!(touched >= 1);
            let canonical = canonicalize_repo_path(&root.to_string_lossy());
            let pid: String = conn
                .query_row(
                    "SELECT project_id FROM experiences WHERE id='exp-legacy'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(pid, canonical);
            let alias: String = conn
                .query_row(
                    "SELECT canonical_id FROM project_aliases WHERE alias='photo-culling' COLLATE NOCASE",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(alias, canonical);
            assert_eq!(migrate_project_ids_to_canonical(&conn).unwrap(), 0);
            let _ = graph; // silence if unused on some cfgs
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn free_form_unknown_slug_binds_unclassified() {
        let store = ExperienceStore::memory().unwrap();
        let conn = store.conn.lock().unwrap();
        migrate_project_identity(&conn).unwrap();
        let bind = bind_write_project_id(&conn, Some("photo-culling"), &[], |_| Ok(None))
            .unwrap()
            .expect("bind");
        assert_eq!(bind.canonical_id, PROJECT_ID_UNCLASSIFIED);
        assert_eq!(bind.kind, ProjectBindKind::Unclassified);
        assert!(bind.kind.warning().is_some());
    }

    #[tokio::test]
    async fn path_without_index_binds_unindexed_placeholder() {
        let store = ExperienceStore::memory().unwrap();
        let conn = store.conn.lock().unwrap();
        migrate_project_identity(&conn).unwrap();
        let bind = bind_write_project_id(
            &conn,
            None,
            &["/tmp/no-such-lounge-project-xyz/src/main.rs"],
            |_| Ok(None),
        )
        .unwrap()
        .expect("bind");
        assert_eq!(bind.canonical_id, PROJECT_ID_UNINDEXED_PLACEHOLDER);
        assert_eq!(bind.kind, ProjectBindKind::UnindexedPlaceholder);
    }

    #[tokio::test]
    async fn alias_slug_binds_canonical_path() {
        let store = ExperienceStore::memory().unwrap();
        let root = std::env::temp_dir().join(format!("pid-core-alias-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let canonical = {
            let conn = store.conn.lock().unwrap();
            migrate_project_identity(&conn).unwrap();
            register_path_project(
                &conn,
                "Agent Lounge OS",
                &root.to_string_lossy(),
                &["agent-lounge-os"],
            )
            .unwrap()
        };
        let bind = {
            let conn = store.conn.lock().unwrap();
            bind_write_project_id(&conn, Some("agent-lounge-os"), &[], |_| Ok(None))
                .unwrap()
                .expect("bind")
        };
        assert_eq!(bind.canonical_id, canonical);
        assert_eq!(bind.kind, ProjectBindKind::Canonical);
        assert_eq!(bind.display_name, "Agent Lounge OS");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn normalize_path_str_still_cross_platform() {
        let n = normalize_path_str(r"C:\Users\x\file.rs");
        assert!(!n.contains('\\'));
    }
}
