//! Canonical project identity (Project-ID-Core).
//!
//! **Canonical ID** = dunce-style canonicalized `repo_path` (symlinks resolved;
//! Windows `\\?\` stripped for local drives; Unicode case-folded on macOS/Windows;
//! trailing slashes removed; UNC kept).
//!
//! **Aliases** map experience/CBM slugs onto that canonical id. Case handling
//! matches the platform (case-sensitive on Linux; Unicode-folded keys on
//! macOS/Windows). Conflicting aliases become **ambiguous** — writes using them
//! go to `unclassified` with a warning naming the candidate canonical ids.
//!
//! **Explicit vs path conflict (B1):** when an explicit `project` / `project_id`
//! resolves to a different indexed canonical project than `active_file` /
//! `workspace_root` / `repo_path` / session `workspace_path`, the write is stored
//! as **`unclassified`** with a clear warning (not silently path-wins). Reason:
//! path-wins would override agent intent without visibility; unclassified forces
//! a visible warning and prevents silent misfiling under the wrong indexed slug
//! (the f5d084e2 class of bug).
//!
//! **Sentinels**
//! - `unclassified` — unknown / ambiguous / conflicting identity
//! - `unindexed_placeholder` — path hint given but no indexed repo matched

#[cfg(test)]
use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Component, PathBuf};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::project_index::accepts_cross_platform_path;

/// Free-form slug that matched nothing known / ambiguous / conflicted.
pub const PROJECT_ID_UNCLASSIFIED: &str = "unclassified";
/// Path hint present but no `project_index` / alias repo match.
pub const PROJECT_ID_UNINDEXED_PLACEHOLDER: &str = "unindexed_placeholder";

/// Settings: one-shot remap of experiences + project_index → canonical ids.
pub const MIGRATION_PROJECT_ID_CANONICAL_V1: &str = "migrations.project_id_canonical_v1";
/// JSON backup for reverse notes (also see remap tables).
pub const MIGRATION_PROJECT_ID_CANONICAL_V1_BACKUP: &str =
    "migrations.project_id_canonical_v1.backup";

const REMAP_TABLE: &str = "project_id_canonical_v1_remap";
const ROW_BACKUP_TABLE: &str = "project_id_canonical_v1_row_backup";

#[cfg(test)]
thread_local! {
    /// Test-only: when set to a label, migration fails at that point (after DML).
    static MIGRATION_FAIL_POINT: Cell<Option<&'static str>> = const { Cell::new(None) };
}

#[cfg(test)]
pub fn set_migration_fail_point(point: Option<&'static str>) {
    MIGRATION_FAIL_POINT.with(|c| c.set(point));
}

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectBind {
    pub canonical_id: String,
    pub kind: ProjectBindKind,
    pub display_name: String,
    /// Human-readable warning for tool / Vault UI (dynamic text).
    pub warning: Option<String>,
}

impl ProjectBind {
    fn canonical(conn: &Connection, canonical_id: &str) -> Result<Self> {
        if canonical_id.eq_ignore_ascii_case(PROJECT_ID_UNCLASSIFIED) {
            return Ok(Self::unclassified(None));
        }
        if canonical_id.eq_ignore_ascii_case(PROJECT_ID_UNINDEXED_PLACEHOLDER) {
            return Ok(Self::unindexed(None));
        }
        let display =
            lookup_display_name(conn, canonical_id)?.unwrap_or_else(|| path_basename(canonical_id));
        Ok(Self {
            canonical_id: canonical_id.to_string(),
            kind: ProjectBindKind::Canonical,
            display_name: display,
            warning: None,
        })
    }

    fn unclassified(warning: Option<String>) -> Self {
        Self {
            canonical_id: PROJECT_ID_UNCLASSIFIED.into(),
            kind: ProjectBindKind::Unclassified,
            display_name: "Unclassified".into(),
            warning: Some(warning.unwrap_or_else(|| {
                "project_id could not be matched to a known canonical repo path; stored as unclassified"
                    .into()
            })),
        }
    }

    fn unindexed(warning: Option<String>) -> Self {
        Self {
            canonical_id: PROJECT_ID_UNINDEXED_PLACEHOLDER.into(),
            kind: ProjectBindKind::UnindexedPlaceholder,
            display_name: "Unindexed".into(),
            warning: Some(warning.unwrap_or_else(|| {
                "no indexed repo_path matched; stored under unindexed_placeholder".into()
            })),
        }
    }
}

/// Create identity tables (idempotent).
///
/// Legacy `project_aliases` (single-column `alias` PK) is **not** rebuilt here —
/// that upgrade runs inside [`migrate_project_ids_to_canonical`]'s transaction
/// so a failed copy cannot DROP the old table after swallowing a SELECT error.
pub fn migrate_project_identity(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS projects (
          canonical_id TEXT PRIMARY KEY,
          display_name TEXT NOT NULL,
          repo_path TEXT,
          kind TEXT NOT NULL DEFAULT 'indexed'
        );
        "#,
    )?;
    ensure_alias_table_present(conn)?;
    if !alias_table_is_legacy(conn)? {
        ensure_sentinel_projects(conn)?;
    }
    Ok(())
}

fn alias_table_sql(conn: &Connection) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='project_aliases'",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

fn alias_table_is_legacy(conn: &Connection) -> Result<bool> {
    match alias_table_sql(conn)? {
        None => Ok(false),
        Some(sql) => Ok(!sql.to_ascii_lowercase().contains("alias_key")),
    }
}

/// Create modern `project_aliases` when missing; leave legacy tables for the
/// transactional upgrade in [`upgrade_legacy_alias_table`].
fn ensure_alias_table_present(conn: &Connection) -> Result<()> {
    if alias_table_sql(conn)?.is_some() {
        return Ok(());
    }
    conn.execute_batch(
        r#"
        CREATE TABLE project_aliases (
          alias_key TEXT NOT NULL,
          canonical_id TEXT NOT NULL,
          alias_display TEXT NOT NULL,
          PRIMARY KEY (alias_key, canonical_id)
        );
        CREATE INDEX IF NOT EXISTS idx_project_aliases_canonical
          ON project_aliases(canonical_id);
        "#,
    )?;
    Ok(())
}

/// Rebuild legacy `PRIMARY KEY (alias)` → composite `(alias_key, canonical_id)`.
/// Must run inside an open transaction (caller owns BEGIN/COMMIT/ROLLBACK).
/// SELECT/copy errors propagate — the old table is not dropped on failure.
fn upgrade_legacy_alias_table(conn: &Connection) -> Result<()> {
    if !alias_table_is_legacy(conn)? {
        return Ok(());
    }
    conn.execute_batch(
        r#"
        CREATE TABLE project_aliases_new (
          alias_key TEXT NOT NULL,
          canonical_id TEXT NOT NULL,
          alias_display TEXT NOT NULL,
          PRIMARY KEY (alias_key, canonical_id)
        );
        "#,
    )
    .context("create project_aliases_new for legacy upgrade")?;
    let mut stmt = conn
        .prepare("SELECT alias, canonical_id FROM project_aliases")
        .context("prepare legacy project_aliases SELECT")?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("query legacy project_aliases rows")?;
    for row in rows {
        let (alias, canonical) = row.context("read legacy project_aliases row")?;
        let key = alias_key(&alias);
        conn.execute(
            r#"
            INSERT OR IGNORE INTO project_aliases_new(alias_key, canonical_id, alias_display)
            VALUES (?1, ?2, ?3)
            "#,
            params![key, canonical, alias],
        )
        .context("insert into project_aliases_new")?;
    }
    conn.execute_batch(
        r#"
        DROP TABLE project_aliases;
        ALTER TABLE project_aliases_new RENAME TO project_aliases;
        CREATE INDEX IF NOT EXISTS idx_project_aliases_canonical
          ON project_aliases(canonical_id);
        "#,
    )
    .context("swap legacy project_aliases to modern schema")?;
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
        insert_alias(conn, id, id)?;
        insert_alias(conn, name, id)?;
    }
    Ok(())
}

/// Strip Windows extended-length prefix (`\\?\` / `//?/`) for local drives only.
/// Plain UNC (`\\server\share`) and extended UNC (`\\?\UNC\…`) are left intact.
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

fn is_unc_path(raw: &str) -> bool {
    let t = raw.trim();
    if t.starts_with(r"\\?\UNC\") || t.starts_with(r"\\?\unc\") || t.starts_with("//?/UNC/") {
        return true;
    }
    // Plain UNC: \\server\share… (not \\?\…)
    if t.starts_with(r"\\") && !t.starts_with(r"\\?\") {
        return true;
    }
    if t.starts_with("//") && !t.starts_with("//?/") {
        return true;
    }
    false
}

/// Canonical repo path identity string (Project-ID-Core).
pub fn canonicalize_repo_path(raw: &str) -> String {
    let stripped = strip_windows_extended_prefix(raw);
    let trimmed = stripped.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let unc = is_unc_path(trimmed);
    // Keep UNC server/share structure; still unify separators for identity.
    let unified = trimmed.replace('\\', "/");
    if !unc && super::project_index::path_has_windows_drive(trimmed) {
        // Windows drive paths: full Windows identity pipeline (slash unify + fold).
        // Applies on every OS so `C:\Foo` and `c:/foo` share a key in tests/CI.
        return windows_identity_key(trimmed);
    }
    let resolved = if unc {
        // Do not call std canonicalize on UNC in tests / mixed hosts — lexical only.
        trim_trailing_slashes(&lexical_normalize_unc(&unified))
    } else {
        let path = PathBuf::from(&unified);
        let body = match path.canonicalize() {
            Ok(canon) => {
                let s = canon.to_string_lossy().into_owned();
                strip_windows_extended_prefix(&s).replace('\\', "/")
            }
            Err(_) => lexical_normalize(&unified),
        };
        trim_trailing_slashes(&body)
    };
    // UNC is case-insensitive on Windows; fold when building Win/mac identity.
    if unc {
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            unicode_case_fold_key(&resolved)
        } else {
            resolved
        }
    } else {
        fold_identity_case(&resolved)
    }
}

fn trim_trailing_slashes(path: &str) -> String {
    let mut out = path.to_string();
    while out.len() > 1 && (out.ends_with('/') || out.ends_with('\\')) {
        // Keep Windows drive root `C:/` as `C:` → normalize to `C:/` style without trailing junk.
        if out.len() == 3 && out.as_bytes()[1] == b':' {
            break;
        }
        out.pop();
    }
    out
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

fn lexical_normalize_unc(unified: &str) -> String {
    // `//server/share/a/../b` → `//server/share/b`
    let mut parts: Vec<&str> = unified.split('/').filter(|p| !p.is_empty()).collect();
    // Leading `//` means UNC — keep empty markers via prefix.
    let mut stack: Vec<&str> = Vec::new();
    for part in parts.drain(..) {
        if part == "." {
            continue;
        }
        if part == ".." {
            if stack.len() > 2 {
                // keep server + share
                stack.pop();
            }
            continue;
        }
        stack.push(part);
    }
    format!("//{}", stack.join("/"))
}

/// Unicode case fold for identity keys (macOS/Windows).
///
/// `str::to_lowercase` is **not** case folding: U+0130 `İ` becomes `i` + U+0307
/// (combining dot), so `İstanbul` would not equal `istanbul`. We map U+0130 → `i`
/// and drop U+0307 so folded forms compare equal.
pub fn unicode_case_fold_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\u{0130}' => out.push('i'), // LATIN CAPITAL LETTER I WITH DOT ABOVE
            '\u{0307}' => {}             // COMBINING DOT ABOVE (from İ case maps)
            c => {
                for lower in c.to_lowercase() {
                    if lower != '\u{0307}' {
                        out.push(lower);
                    }
                }
            }
        }
    }
    out
}

/// Case fold on macOS/Windows; preserve case on Linux.
pub fn fold_identity_case(path: &str) -> String {
    // `cfg!` keeps `unicode_case_fold_key` referenced on every OS (no dead_code).
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        unicode_case_fold_key(path)
    } else {
        path.to_string()
    }
}

/// Alias lookup key — platform case policy.
pub fn alias_key(alias: &str) -> String {
    let t = alias.trim();
    if cfg!(any(target_os = "macos", target_os = "windows")) {
        unicode_case_fold_key(t)
    } else {
        t.to_string()
    }
}

/// Windows identity key pipeline (slash unify + lexical + Unicode fold).
/// Used by tests and as the case-insensitive half of path identity on Win/macOS.
pub fn windows_identity_key(raw: &str) -> String {
    let stripped = strip_windows_extended_prefix(raw);
    let unified = stripped.trim().replace('\\', "/");
    let lex = trim_trailing_slashes(&lexical_normalize(&unified));
    unicode_case_fold_key(&lex)
}

fn path_basename(path: &str) -> String {
    trim_trailing_slashes(&path.replace('\\', "/"))
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

/// Insert alias mapping. Same canonical → idempotent. Different canonical →
/// keep **both** rows so the alias becomes ambiguous (never silent overwrite).
pub fn upsert_alias(conn: &Connection, alias: &str, canonical_id: &str) -> Result<()> {
    insert_alias(conn, alias, canonical_id)
}

fn insert_alias(conn: &Connection, alias: &str, canonical_id: &str) -> Result<()> {
    let alias = alias.trim();
    let canonical_id = canonical_id.trim();
    if alias.is_empty() || canonical_id.is_empty() {
        return Ok(());
    }
    let key = alias_key(alias);
    conn.execute(
        r#"
        INSERT OR IGNORE INTO project_aliases(alias_key, canonical_id, alias_display)
        VALUES (?1, ?2, ?3)
        "#,
        params![key, canonical_id, alias],
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
        bail!("repo_path empty after canonicalize");
    }
    let display = choose_display_name(Some(lounge_name), Some(repo_path), Some(lounge_name));
    upsert_project(conn, &canonical, &display, Some(&canonical), "indexed")?;
    insert_alias(conn, &canonical, &canonical)?;
    insert_alias(conn, &display, &canonical)?;
    if !lounge_name.trim().is_empty() {
        insert_alias(conn, lounge_name, &canonical)?;
    }
    let base = path_basename(&canonical);
    if !base.is_empty() {
        insert_alias(conn, &base, &canonical)?;
    }
    let raw_base = path_basename(repo_path);
    if !raw_base.is_empty() {
        insert_alias(conn, &raw_base, &canonical)?;
    }
    for alias in extra_aliases {
        insert_alias(conn, alias, &canonical)?;
    }
    Ok(canonical)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AliasLookup {
    None,
    Unique(String),
    Ambiguous(Vec<String>),
}

fn lookup_alias_status(conn: &Connection, alias: &str) -> Result<AliasLookup> {
    let alias = alias.trim();
    if alias.is_empty() {
        return Ok(AliasLookup::None);
    }
    let key = alias_key(alias);
    let mut stmt = conn.prepare(
        "SELECT canonical_id FROM project_aliases WHERE alias_key = ?1 ORDER BY canonical_id",
    )?;
    let rows = stmt.query_map(params![key], |r| r.get::<_, String>(0))?;
    let mut ids = Vec::new();
    for row in rows {
        let id = row?;
        if !ids.iter().any(|x| x == &id) {
            ids.push(id);
        }
    }
    Ok(match ids.len() {
        0 => AliasLookup::None,
        1 => AliasLookup::Unique(ids.remove(0)),
        _ => AliasLookup::Ambiguous(ids),
    })
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
/// Conflict policy (B1): explicit id that resolves to a *different* indexed
/// canonical than path hints → `unclassified` + warning (not path-wins).
pub fn bind_write_project_id(
    conn: &Connection,
    explicit: Option<&str>,
    path_hints: &[&str],
    resolve_path: impl Fn(&str) -> Result<Option<String>>,
) -> Result<Option<ProjectBind>> {
    migrate_project_identity(conn)?;

    let path_resolution = resolve_path_hints(conn, path_hints, &resolve_path)?;

    if let Some(raw) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        let explicit_resolution = resolve_known(conn, raw)?;
        return Ok(Some(reconcile_explicit_and_path(
            conn,
            raw,
            explicit_resolution,
            path_resolution,
        )?));
    }

    match path_resolution {
        PathResolution::Hit(id) => Ok(Some(ProjectBind::canonical(conn, &id)?)),
        PathResolution::Unindexed => Ok(Some(ProjectBind::unindexed(None))),
        PathResolution::None => Ok(None),
    }
}

#[derive(Debug, Clone)]
enum PathResolution {
    None,
    Hit(String),
    Unindexed,
}

#[derive(Debug, Clone)]
enum ExplicitResolution {
    Unknown,
    Hit(String),
    Ambiguous(Vec<String>),
}

fn resolve_path_hints(
    conn: &Connection,
    path_hints: &[&str],
    resolve_path: &impl Fn(&str) -> Result<Option<String>>,
) -> Result<PathResolution> {
    let mut saw_path_shaped = false;
    for hint in path_hints {
        let hint = hint.trim();
        if hint.is_empty() {
            continue;
        }
        if let Some(matched) = resolve_path(hint)? {
            let id = prefer_canonical(conn, &matched)?;
            return Ok(PathResolution::Hit(id));
        }
        if accepts_cross_platform_path(hint) {
            saw_path_shaped = true;
        }
    }
    if saw_path_shaped {
        Ok(PathResolution::Unindexed)
    } else {
        Ok(PathResolution::None)
    }
}

fn reconcile_explicit_and_path(
    conn: &Connection,
    explicit_raw: &str,
    explicit: ExplicitResolution,
    path: PathResolution,
) -> Result<ProjectBind> {
    match (&explicit, &path) {
        (ExplicitResolution::Hit(a), PathResolution::Hit(b)) if a != b => {
            Ok(ProjectBind::unclassified(Some(format!(
                "explicit project_id={explicit_raw:?} resolves to {a}, but active_file/workspace path resolves to {b}; stored as unclassified to prevent misfiling"
            ))))
        }
        (ExplicitResolution::Ambiguous(cands), _) => Ok(ProjectBind::unclassified(Some(format!(
            "alias {explicit_raw:?} is ambiguous across projects [{}]; stored as unclassified",
            cands.join(", ")
        )))),
        (ExplicitResolution::Hit(a), _) => ProjectBind::canonical(conn, a),
        (ExplicitResolution::Unknown, PathResolution::Hit(b)) => {
            // Explicit slug is not a known alias — path context wins when available.
            ProjectBind::canonical(conn, b)
        }
        (ExplicitResolution::Unknown, PathResolution::Unindexed) => {
            // Explicit free-form + path-shaped hint with no index → unclassified
            // (free-form dominates the unknown-slug rule) unless path-only.
            if accepts_cross_platform_path(explicit_raw) {
                Ok(ProjectBind::unindexed(None))
            } else {
                Ok(ProjectBind::unclassified(None))
            }
        }
        (ExplicitResolution::Unknown, PathResolution::None) => {
            if accepts_cross_platform_path(explicit_raw) {
                Ok(ProjectBind::unindexed(None))
            } else {
                Ok(ProjectBind::unclassified(None))
            }
        }
    }
}

fn resolve_known(conn: &Connection, raw: &str) -> Result<ExplicitResolution> {
    match lookup_alias_status(conn, raw)? {
        AliasLookup::Unique(id) => return Ok(ExplicitResolution::Hit(id)),
        AliasLookup::Ambiguous(cands) => return Ok(ExplicitResolution::Ambiguous(cands)),
        AliasLookup::None => {}
    }
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM projects WHERE canonical_id = ?1 LIMIT 1",
            params![raw],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if exists {
        return Ok(ExplicitResolution::Hit(raw.to_string()));
    }
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
                return Ok(ExplicitResolution::Hit(id));
            }
        }
    }
    Ok(ExplicitResolution::Unknown)
}

/// One-shot atomic remap. Done-flag set only on COMMIT. Errors roll back fully.
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

    conn.execute_batch("BEGIN IMMEDIATE")
        .context("project_id canonical migration BEGIN")?;
    match migrate_project_ids_to_canonical_inner(conn) {
        Ok(n) => {
            conn.execute_batch("COMMIT")
                .context("project_id canonical migration COMMIT")?;
            Ok(n)
        }
        Err(err) => {
            if let Err(rb) = conn.execute_batch("ROLLBACK") {
                log::error!("project_id canonical migration ROLLBACK failed: {rb:#}");
                eprintln!("[lounge] CRITICAL: project_id migration ROLLBACK failed: {rb:#}");
            }
            log::error!("project_id canonical migration failed; DB unchanged: {err:#}");
            eprintln!(
                "[lounge] CRITICAL: project_id canonical migration failed; DB unchanged: {err:#}"
            );
            Err(err)
        }
    }
}

fn migrate_project_ids_to_canonical_inner(conn: &Connection) -> Result<u64> {
    // Legacy alias rebuild must run inside this transaction (propagate errors;
    // never DROP the old table after a swallowed SELECT failure).
    upgrade_legacy_alias_table(conn)?;
    ensure_sentinel_projects(conn)?;
    ensure_backup_tables(conn)?;

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
            upsert_project(
                conn,
                &old_id,
                &choose_display_name(Some(&old_id), None, Some(&old_id)),
                None,
                "unindexed_placeholder",
            )?;
            insert_alias(conn, &old_id, &old_id)?;
            continue;
        }
        let canonical = register_path_project(conn, &old_id, repo_trim, &[])?;
        if canonical != old_id && seen_from.insert(old_id.clone()) {
            remaps.push((old_id, canonical));
        }
    }

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
            if !matches!(lookup_alias_status(conn, &old_id)?, AliasLookup::None) {
                continue;
            }
            upsert_project(
                conn,
                &old_id,
                &choose_display_name(Some(&old_id), None, Some(&old_id)),
                None,
                "unindexed_placeholder",
            )?;
            insert_alias(conn, &old_id, &old_id)?;
        }
    }

    // Full row backup before rewrite (reversible).
    for (from, to) in &remaps {
        backup_rows_for_remap(conn, from, to)?;
    }

    let mut touched = 0u64;
    for (from, to) in &remaps {
        touched += rewrite_project_id(conn, from, to)?;
        maybe_fail_point("after_rewrite")?;
    }

    let backup = serde_json::json!({
        "remaps": remaps.iter().map(|(f, t)| json_remap(f, t)).collect::<Vec<_>>(),
        "tables": [REMAP_TABLE, ROW_BACKUP_TABLE],
        "note": "Reverse: for each remap restore project_id from project_id_canonical_v1_row_backup / remap table; DELETE done-flag settings keys."
    });
    conn.execute(
        "INSERT INTO settings(key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![MIGRATION_PROJECT_ID_CANONICAL_V1_BACKUP, backup.to_string()],
    )?;
    maybe_fail_point("before_done_flag")?;
    conn.execute(
        "INSERT INTO settings(key, value_json) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json",
        params![MIGRATION_PROJECT_ID_CANONICAL_V1, "1"],
    )?;
    Ok(touched)
}

fn ensure_backup_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(&format!(
        r#"
        CREATE TABLE IF NOT EXISTS {REMAP_TABLE} (
          old_project_id TEXT PRIMARY KEY,
          new_project_id TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS {ROW_BACKUP_TABLE} (
          entity TEXT NOT NULL,
          entity_id TEXT NOT NULL,
          project_id_before TEXT NOT NULL,
          PRIMARY KEY (entity, entity_id)
        );
        "#
    ))?;
    Ok(())
}

fn backup_rows_for_remap(conn: &Connection, from: &str, to: &str) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT OR REPLACE INTO {REMAP_TABLE}(old_project_id, new_project_id) VALUES (?1, ?2)"
        ),
        params![from, to],
    )?;
    if table_exists(conn, "experiences")? {
        let mut stmt = conn.prepare("SELECT id FROM experiences WHERE project_id = ?1")?;
        let ids = stmt.query_map(params![from], |r| r.get::<_, String>(0))?;
        for id in ids {
            let id = id?;
            conn.execute(
                &format!(
                    "INSERT OR REPLACE INTO {ROW_BACKUP_TABLE}(entity, entity_id, project_id_before) VALUES ('experience', ?1, ?2)"
                ),
                params![id, from],
            )?;
        }
    }
    if table_exists(conn, "project_flags")? {
        let hidden: Option<i64> = conn
            .query_row(
                "SELECT hidden FROM project_flags WHERE project_id = ?1",
                params![from],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(h) = hidden {
            conn.execute(
                &format!(
                    "INSERT OR REPLACE INTO {ROW_BACKUP_TABLE}(entity, entity_id, project_id_before) VALUES ('project_flag', ?1, ?2)"
                ),
                params![format!("{from}:{h}"), from],
            )?;
        }
    }
    if table_exists(conn, "ignored_symbols")? {
        let mut stmt = conn.prepare("SELECT id FROM ignored_symbols WHERE project_id = ?1")?;
        let ids = stmt.query_map(params![from], |r| r.get::<_, String>(0))?;
        for id in ids {
            let id = id?;
            conn.execute(
                &format!(
                    "INSERT OR REPLACE INTO {ROW_BACKUP_TABLE}(entity, entity_id, project_id_before) VALUES ('ignored_symbol', ?1, ?2)"
                ),
                params![id, from],
            )?;
        }
    }
    Ok(())
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
    if table_exists(conn, "project_flags")? {
        conn.execute(
            r#"
            INSERT INTO project_flags(project_id, hidden)
            SELECT ?1, hidden FROM project_flags WHERE project_id = ?2
            ON CONFLICT(project_id) DO UPDATE SET
              hidden = CASE
                WHEN excluded.hidden > project_flags.hidden THEN excluded.hidden
                ELSE project_flags.hidden
              END
            "#,
            params![to, from],
        )
        .context("project_flags merge during project_id remap")?;
        conn.execute(
            "DELETE FROM project_flags WHERE project_id = ?1",
            params![from],
        )
        .context("project_flags delete old id during project_id remap")?;
    }
    if table_exists(conn, "ignored_symbols")? {
        n += conn.execute(
            "UPDATE ignored_symbols SET project_id = ?1 WHERE project_id = ?2",
            params![to, from],
        )? as u64;
    }
    Ok(n)
}

fn maybe_fail_point(label: &'static str) -> Result<()> {
    #[cfg(test)]
    {
        let hit = MIGRATION_FAIL_POINT.with(|c| c.get() == Some(label));
        if hit {
            bail!("injected migration failure at {label}");
        }
    }
    #[cfg(not(test))]
    {
        let _ = label;
    }
    Ok(())
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
        params![name],
        |row| row.get(0),
    )?;
    Ok(n > 0)
}

/// Map a path-resolve hit through aliases when the alias is unambiguous.
/// Ambiguous aliases leave the index project_id unchanged.
pub fn prefer_canonical(conn: &Connection, resolved: &str) -> Result<String> {
    match lookup_alias_status(conn, resolved)? {
        AliasLookup::Unique(c) => Ok(c),
        AliasLookup::None | AliasLookup::Ambiguous(_) => Ok(resolved.to_string()),
    }
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
        let unc = r"\\?\UNC\server\share\repo";
        assert_eq!(strip_windows_extended_prefix(unc), unc);
    }

    #[test]
    fn plain_unc_and_trailing_slashes_normalized() {
        let plain = canonicalize_repo_path(r"\\server\share\Agent-Lounge-OS\");
        assert!(
            plain.starts_with("//server/share/"),
            "plain UNC must stay UNC-shaped: {plain}"
        );
        assert!(!plain.ends_with('/'), "trailing slash stripped: {plain}");
        assert!(!plain.contains('\\'));

        let with_slash = canonicalize_repo_path("/tmp/Agent-Lounge-OS///");
        assert!(
            !with_slash.ends_with('/'),
            "POSIX trailing slashes stripped: {with_slash}"
        );
    }

    #[test]
    fn drive_letter_windows_rules_exact_keys() {
        // Windows identity rules: slash unify + case fold — exact equality.
        assert_eq!(windows_identity_key(r"C:\Foo"), "c:/foo");
        assert_eq!(windows_identity_key(r"c:/foo"), "c:/foo");
        assert_eq!(
            windows_identity_key(r"C:\Foo"),
            windows_identity_key(r"c:/foo")
        );
        assert_ne!(
            windows_identity_key(r"C:\Foo"),
            windows_identity_key(r"D:\Foo"),
            "different drives must not collide"
        );
        assert_eq!(windows_identity_key(r"D:\Foo"), "d:/foo");
        // Trailing slash / dots under Windows rules.
        assert_eq!(
            windows_identity_key(r"C:\Users\dev\..\Foo\"),
            "c:/users/foo"
        );
    }

    #[test]
    fn unicode_case_fold_istanbul_matches_istanbul() {
        // Real case folding: İ (U+0130) must match ascii i — to_lowercase alone fails.
        assert_eq!(
            unicode_case_fold_key("İstanbul"),
            unicode_case_fold_key("istanbul")
        );
        assert_eq!(unicode_case_fold_key("İstanbul"), "istanbul");
        assert_ne!(
            "İstanbul".to_lowercase(),
            "istanbul",
            "sanity: to_lowercase is NOT sufficient case folding"
        );
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            assert_eq!(
                fold_identity_case("İstanbul"),
                fold_identity_case("istanbul")
            );
            assert_eq!(fold_identity_case("İstanbul"), "istanbul");
            assert_eq!(alias_key("İstanbul"), alias_key("istanbul"));
        }
        #[cfg(target_os = "linux")]
        {
            assert_eq!(fold_identity_case("İstanbul"), "İstanbul");
            assert_ne!(alias_key("İstanbul"), alias_key("istanbul"));
        }
    }

    #[test]
    fn linux_case_sensitive_alias_keys() {
        let a = alias_key("Photo-Culling");
        let b = alias_key("photo-culling");
        #[cfg(target_os = "linux")]
        {
            assert_ne!(a, b, "Linux alias keys are case-sensitive");
        }
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            assert_eq!(a, b, "macOS/Windows alias keys fold case");
        }
    }

    #[test]
    fn canonicalize_case_policy_matches_os() {
        let mixed = canonicalize_repo_path("/tmp/Agent-Lounge-OS-Case");
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            assert_eq!(mixed, unicode_case_fold_key("/tmp/Agent-Lounge-OS-Case"));
        }
        #[cfg(target_os = "linux")]
        {
            assert!(
                mixed.contains("Agent-Lounge-OS-Case") || mixed.ends_with("Agent-Lounge-OS-Case")
            );
        }
    }

    #[tokio::test]
    async fn legacy_alias_table_upgrades_inside_migration_transaction() {
        let store = ExperienceStore::memory().unwrap();
        {
            let conn = store.conn.lock().unwrap();
            // Force legacy schema.
            conn.execute_batch(
                r#"
                DROP TABLE IF EXISTS project_aliases;
                CREATE TABLE project_aliases (
                  alias TEXT PRIMARY KEY COLLATE NOCASE,
                  canonical_id TEXT NOT NULL
                );
                INSERT INTO project_aliases(alias, canonical_id)
                  VALUES ('Photo-Culling', '/tmp/photo');
                DELETE FROM settings WHERE key LIKE 'migrations.project_id_canonical_v1%';
                "#,
            )
            .unwrap();
            assert!(alias_table_is_legacy(&conn).unwrap());
            migrate_project_ids_to_canonical(&conn).unwrap();
            assert!(!alias_table_is_legacy(&conn).unwrap());
            let key = alias_key("Photo-Culling");
            let canon: String = conn
                .query_row(
                    "SELECT canonical_id FROM project_aliases WHERE alias_key = ?1",
                    params![key],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(canon, "/tmp/photo");
        }
    }

    #[tokio::test]
    async fn legacy_alias_upgrade_select_error_rolls_back() {
        let store = ExperienceStore::memory().unwrap();
        {
            let conn = store.conn.lock().unwrap();
            // Legacy-shaped name but missing `alias` column → SELECT fails.
            conn.execute_batch(
                r#"
                DROP TABLE IF EXISTS project_aliases;
                CREATE TABLE project_aliases (
                  id TEXT PRIMARY KEY,
                  canonical_id TEXT NOT NULL
                );
                INSERT INTO project_aliases(id, canonical_id) VALUES ('x', '/tmp/x');
                DELETE FROM settings WHERE key LIKE 'migrations.project_id_canonical_v1%';
                "#,
            )
            .unwrap();
            assert!(alias_table_is_legacy(&conn).unwrap());
            let err = migrate_project_ids_to_canonical(&conn).unwrap_err();
            assert!(
                err.to_string().contains("legacy")
                    || err
                        .chain()
                        .any(|e| e.to_string().to_ascii_lowercase().contains("alias")),
                "expected propagated SELECT error, got: {err:#}"
            );
            // Rolled back: legacy table still present, modern swap did not stick.
            assert!(alias_table_is_legacy(&conn).unwrap());
            let n: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name='project_aliases_new'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(n, 0, "project_aliases_new must not survive rollback");
            let done: Option<String> = conn
                .query_row(
                    "SELECT value_json FROM settings WHERE key = ?1",
                    params![MIGRATION_PROJECT_ID_CANONICAL_V1],
                    |r| r.get(0),
                )
                .optional()
                .unwrap();
            assert!(done.is_none());
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
            conn.execute(
                "DELETE FROM settings WHERE key = ?1 OR key = ?2",
                params![
                    MIGRATION_PROJECT_ID_CANONICAL_V1,
                    MIGRATION_PROJECT_ID_CANONICAL_V1_BACKUP
                ],
            )
            .unwrap();
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
            assert_eq!(
                lookup_alias_status(&conn, "photo-culling").unwrap(),
                AliasLookup::Unique(canonical.clone())
            );
            let remap_to: String = conn
                .query_row(
                    &format!(
                        "SELECT new_project_id FROM {REMAP_TABLE} WHERE old_project_id='photo-culling'"
                    ),
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(remap_to, canonical);
            assert_eq!(migrate_project_ids_to_canonical(&conn).unwrap(), 0);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn migration_failure_rolls_back_and_preserves_flags() {
        let store = ExperienceStore::memory().unwrap();
        let root = std::env::temp_dir().join(format!("pid-core-fail-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute("DELETE FROM project_index", []).unwrap();
            conn.execute("DELETE FROM project_aliases", []).ok();
            conn.execute("DELETE FROM projects", []).ok();
            migrate_project_identity(&conn).unwrap();
            crate::db::experience_governance::ensure_project_flags_table(&conn).unwrap();
            conn.execute(
                r#"
                INSERT INTO project_index (
                  id, project_id, repo_path, kind, name, file_path, line,
                  target, ref_count, detail, payload_json, indexed_at
                ) VALUES ('1','photo-culling',?1,'meta','__registered__',NULL,NULL,NULL,0,NULL,'{}','2026-01-01T00:00:00Z')
                "#,
                params![root.to_string_lossy().as_ref()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO project_flags(project_id, hidden) VALUES ('photo-culling', 1)",
                [],
            )
            .unwrap();
            conn.execute(
                "DELETE FROM settings WHERE key = ?1 OR key = ?2",
                params![
                    MIGRATION_PROJECT_ID_CANONICAL_V1,
                    MIGRATION_PROJECT_ID_CANONICAL_V1_BACKUP
                ],
            )
            .unwrap();
            set_migration_fail_point(Some("before_done_flag"));
            let err = migrate_project_ids_to_canonical(&conn).unwrap_err();
            set_migration_fail_point(None);
            assert!(
                err.to_string().contains("injected migration failure"),
                "{err}"
            );
            // Done flag must not be set.
            let done: Option<String> = conn
                .query_row(
                    "SELECT value_json FROM settings WHERE key = ?1",
                    params![MIGRATION_PROJECT_ID_CANONICAL_V1],
                    |r| r.get(0),
                )
                .optional()
                .unwrap();
            assert!(done.is_none(), "done flag must not stick after rollback");
            // Hidden flag on original id preserved (rollback).
            let hidden: i64 = conn
                .query_row(
                    "SELECT hidden FROM project_flags WHERE project_id = 'photo-culling'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(hidden, 1);
            let idx_pid: String = conn
                .query_row(
                    "SELECT project_id FROM project_index WHERE id='1'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(idx_pid, "photo-culling");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn free_form_unknown_slug_binds_unclassified() {
        let store = ExperienceStore::memory().unwrap();
        let conn = store.conn.lock().unwrap();
        migrate_project_identity(&conn).unwrap();
        let bind = bind_write_project_id(&conn, Some("no-such-slug-xyz"), &[], |_| Ok(None))
            .unwrap()
            .expect("bind");
        assert_eq!(bind.canonical_id, PROJECT_ID_UNCLASSIFIED);
        assert_eq!(bind.kind, ProjectBindKind::Unclassified);
        assert!(bind.warning.is_some());
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

    #[tokio::test]
    async fn explicit_photo_culling_conflicts_with_lounge_active_file() {
        let store = ExperienceStore::memory().unwrap();
        let photo = std::env::temp_dir().join(format!("pid-photo-{}", Uuid::new_v4()));
        let lounge = std::env::temp_dir().join(format!("pid-lounge-{}", Uuid::new_v4()));
        std::fs::create_dir_all(photo.join("src")).unwrap();
        std::fs::create_dir_all(lounge.join("src")).unwrap();
        let lounge_file = lounge.join("src/main.rs");
        std::fs::write(&lounge_file, "fn main() {}").unwrap();

        store
            .save_project_index(IndexGraph {
                project: "photo-culling".into(),
                repo_path: photo.to_string_lossy().into_owned(),
                status: None,
                node_count: 0,
                edge_count: 0,
                files: Some(0),
                nodes: vec![],
                references: vec![],
                dead: vec![],
            })
            .await
            .unwrap();
        store
            .save_project_index(IndexGraph {
                project: "Agent-Lounge-OS".into(),
                repo_path: lounge.to_string_lossy().into_owned(),
                status: None,
                node_count: 1,
                edge_count: 0,
                files: Some(1),
                nodes: vec![AstNode {
                    id: "main".into(),
                    name: "main".into(),
                    kind: "fn".into(),
                    file: Some(lounge_file.to_string_lossy().into_owned()),
                    line: Some(1),
                    ref_count: 0,
                }],
                references: vec![],
                dead: vec![],
            })
            .await
            .unwrap();

        let photo_canon = canonicalize_repo_path(&photo.to_string_lossy());
        let lounge_canon = canonicalize_repo_path(&lounge.to_string_lossy());
        assert_ne!(photo_canon, lounge_canon);

        let bind = {
            let conn = store.conn.lock().unwrap();
            let file = lounge_file.to_string_lossy().into_owned();
            bind_write_project_id(&conn, Some("photo-culling"), &[file.as_str()], |path| {
                // Mimic longest-prefix: lounge file → lounge canonical.
                if path.starts_with(&lounge.to_string_lossy().to_string())
                    || canonicalize_repo_path(path).starts_with(&lounge_canon)
                {
                    Ok(Some(lounge_canon.clone()))
                } else if path.starts_with(&photo.to_string_lossy().to_string()) {
                    Ok(Some(photo_canon.clone()))
                } else {
                    Ok(None)
                }
            })
            .unwrap()
            .expect("bind")
        };
        assert_eq!(bind.canonical_id, PROJECT_ID_UNCLASSIFIED);
        assert_eq!(bind.kind, ProjectBindKind::Unclassified);
        let w = bind.warning.unwrap_or_default();
        assert!(w.contains("photo-culling"), "{w}");
        assert!(w.contains(&photo_canon) || w.contains("resolves to"), "{w}");
        assert!(
            w.contains(&lounge_canon) || w.contains("active_file"),
            "{w}"
        );

        let _ = std::fs::remove_dir_all(&photo);
        let _ = std::fs::remove_dir_all(&lounge);
    }

    #[tokio::test]
    async fn conflicting_folder_basename_alias_is_ambiguous() {
        let store = ExperienceStore::memory().unwrap();
        let a = std::env::temp_dir().join(format!("pid-app-a-{}", Uuid::new_v4()));
        let b = std::env::temp_dir().join(format!("pid-app-b-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let (ca, cb) = {
            let conn = store.conn.lock().unwrap();
            migrate_project_identity(&conn).unwrap();
            let ca = register_path_project(&conn, "app", &a.to_string_lossy(), &[]).unwrap();
            let cb = register_path_project(&conn, "app", &b.to_string_lossy(), &[]).unwrap();
            // Force the shared slug alias onto both (basename may already collide).
            insert_alias(&conn, "app", &ca).unwrap();
            insert_alias(&conn, "app", &cb).unwrap();
            (ca, cb)
        };
        assert_ne!(ca, cb);
        let bind = {
            let conn = store.conn.lock().unwrap();
            bind_write_project_id(&conn, Some("app"), &[], |_| Ok(None))
                .unwrap()
                .expect("bind")
        };
        assert_eq!(bind.canonical_id, PROJECT_ID_UNCLASSIFIED);
        let w = bind.warning.unwrap_or_default();
        assert!(w.contains("ambiguous"), "{w}");
        assert!(w.contains(&ca) && w.contains(&cb), "{w}");
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    #[test]
    fn normalize_path_str_still_cross_platform() {
        let n = normalize_path_str(r"C:\Users\x\file.rs");
        assert!(!n.contains('\\'));
    }
}
