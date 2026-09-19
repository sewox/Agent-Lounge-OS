use anyhow::{Context, Result};
use rusqlite::{params, Connection};

use super::ExperienceStore;
use crate::models::{now_rfc3339, sqlite_tool_type, ConnectedTool, DiscoveredTool};

pub(crate) fn migrate_connected_tools(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
            CREATE TABLE IF NOT EXISTS connected_tools (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              kind TEXT NOT NULL,
              source TEXT NOT NULL,
              origin_path TEXT,
              command TEXT,
              args_json TEXT NOT NULL DEFAULT '[]',
              endpoint TEXT,
              enabled INTEGER NOT NULL DEFAULT 1,
              connected_at TEXT NOT NULL,
              payload_json TEXT NOT NULL DEFAULT '{}',
              "type" TEXT NOT NULL DEFAULT 'mcp',
              config_path TEXT,
              is_active INTEGER NOT NULL DEFAULT 1,
              last_synced TEXT NOT NULL DEFAULT ''
            );
            "#,
    )?;
    ensure_column(conn, "type", r#"TEXT NOT NULL DEFAULT 'mcp'"#)?;
    ensure_column(conn, "config_path", "TEXT")?;
    ensure_column(conn, "is_active", "INTEGER NOT NULL DEFAULT 1")?;
    ensure_column(conn, "last_synced", "TEXT NOT NULL DEFAULT ''")?;
    conn.execute_batch(
        r#"
            UPDATE connected_tools SET
              "type" = CASE kind WHEN 'model' THEN 'model' WHEN 'mcp' THEN 'mcp' ELSE 'cli' END,
              config_path = COALESCE(NULLIF(config_path, ''), origin_path),
              is_active = enabled,
              last_synced = CASE
                WHEN last_synced IS NULL OR last_synced = '' THEN connected_at
                ELSE last_synced
              END;
            "#,
    )?;
    Ok(())
}

fn ensure_column(conn: &Connection, name: &str, decl: &str) -> Result<()> {
    if column_names(conn)?.iter().any(|col| col == name) {
        return Ok(());
    }
    conn.execute(
        &format!(r#"ALTER TABLE connected_tools ADD COLUMN "{name}" {decl}"#),
        [],
    )?;
    Ok(())
}

fn column_names(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("PRAGMA table_info(connected_tools)")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row?);
    }
    Ok(names)
}

impl ExperienceStore {
    pub async fn save_connected_tools(
        &self,
        selected: Vec<DiscoveredTool>,
    ) -> Result<Vec<ConnectedTool>> {
        self.save_selected_tools(selected).await
    }

    pub async fn save_selected_tools(
        &self,
        selected: Vec<DiscoveredTool>,
    ) -> Result<Vec<ConnectedTool>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            save_selected_tools_blocking(&conn, &selected)
        })
        .await
        .context("connected_tools save join")?
    }

    pub async fn list_connected_tools(&self) -> Result<Vec<ConnectedTool>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().expect("experience db lock");
            list_connected_tools_blocking(&conn)
        })
        .await
        .context("connected_tools list join")?
    }
}

fn save_selected_tools_blocking(
    conn: &Connection,
    selected: &[DiscoveredTool],
) -> Result<Vec<ConnectedTool>> {
    let now = now_rfc3339();
    let selected_ids: Vec<String> = selected.iter().map(|tool| tool.id.clone()).collect();

    for tool in selected {
        upsert_tool(conn, tool, &now)?;
    }

    let existing = list_ids(conn)?;
    for id in existing {
        if !selected_ids.iter().any(|selected| selected == &id) {
            conn.execute(
                r#"UPDATE connected_tools SET enabled = 0, is_active = 0, last_synced = ?2 WHERE id = ?1"#,
                params![id, now],
            )?;
        }
    }

    list_connected_tools_blocking(conn)
}

fn upsert_tool(conn: &Connection, tool: &DiscoveredTool, now: &str) -> Result<()> {
    let row = ConnectedTool::from_discovered(tool, now.to_string());
    let args_json = serde_json::to_string(&row.args).unwrap_or_else(|_| "[]".into());
    let payload_json = serde_json::to_string(&row.payload).unwrap_or_else(|_| "{}".into());
    let tool_type = sqlite_tool_type(&row.kind);
    conn.execute(
        r#"
        INSERT INTO connected_tools (
            id, name, kind, source, origin_path, command, args_json, endpoint,
            enabled, connected_at, payload_json,
            "type", config_path, is_active, last_synced
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?10, ?11, ?12, 1, ?13)
        ON CONFLICT(id) DO UPDATE SET
            name = excluded.name,
            kind = excluded.kind,
            source = excluded.source,
            origin_path = excluded.origin_path,
            command = excluded.command,
            args_json = excluded.args_json,
            endpoint = excluded.endpoint,
            enabled = 1,
            payload_json = excluded.payload_json,
            "type" = excluded."type",
            config_path = excluded.config_path,
            is_active = 1,
            last_synced = excluded.last_synced
        "#,
        params![
            row.id,
            row.name,
            row.kind,
            row.source,
            row.origin_path,
            row.command,
            args_json,
            row.endpoint,
            row.connected_at,
            payload_json,
            tool_type,
            row.config_path,
            row.last_synced,
        ],
    )?;
    Ok(())
}

fn list_ids(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT id FROM connected_tools")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    let mut ids = Vec::new();
    for id in rows {
        ids.push(id?);
    }
    Ok(ids)
}

fn map_connected_tool(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConnectedTool> {
    let args_json: String = row.get(6)?;
    let payload_json: String = row.get(10)?;
    let enabled: i64 = row.get(8)?;
    let is_active: i64 = row.get(13)?;
    Ok(ConnectedTool {
        id: row.get(0)?,
        name: row.get(1)?,
        kind: row.get(2)?,
        source: row.get(3)?,
        origin_path: row.get(4)?,
        command: row.get(5)?,
        args: serde_json::from_str(&args_json).unwrap_or_default(),
        endpoint: row.get(7)?,
        enabled: enabled != 0,
        connected_at: row.get(9)?,
        payload: serde_json::from_str(&payload_json).unwrap_or(serde_json::json!({})),
        tool_type: row.get(11)?,
        config_path: row.get(12)?,
        is_active: is_active != 0,
        last_synced: row.get(14)?,
    })
}

fn list_connected_tools_blocking(conn: &Connection) -> Result<Vec<ConnectedTool>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, name, kind, source, origin_path, command, args_json, endpoint,
               enabled, connected_at, payload_json,
               "type", config_path, is_active, last_synced
        FROM connected_tools
        ORDER BY "type", name
        "#,
    )?;
    let rows = stmt.query_map([], map_connected_tool)?;
    let mut tools = Vec::new();
    for row in rows {
        tools.push(row?);
    }
    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DiscoveredTool;

    fn sample_tool(id_name: &str) -> DiscoveredTool {
        let mut tool = DiscoveredTool::new("cursor", id_name, "mcp");
        tool.command = Some("npx".into());
        tool.args = vec!["-y".into(), "demo".into()];
        tool.detail = Some("env keys: TOKEN".into());
        tool.origin_path = Some("~/.cursor/mcp.json".into());
        tool
    }

    fn sample_model() -> DiscoveredTool {
        let mut tool = DiscoveredTool::new("ollama", "llama3.1:8b", "model");
        tool.origin_path = Some("http://127.0.0.1:11434/api/tags".into());
        tool.endpoint = Some("http://127.0.0.1:11434".into());
        tool
    }

    fn sample_cli() -> DiscoveredTool {
        let mut tool = DiscoveredTool::new("system", "git", "system");
        tool.origin_path = Some("/usr/bin/git".into());
        tool.command = Some("/usr/bin/git".into());
        tool
    }

    #[tokio::test]
    async fn upsert_and_list_roundtrip() {
        let store = ExperienceStore::memory().expect("memory db");
        let first = sample_tool("notion");
        let listed = store.save_connected_tools(vec![first]).await.expect("save");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "cursor:notion");
        assert!(listed[0].enabled);
        assert_eq!(listed[0].args, vec!["-y", "demo"]);

        let again = store.list_connected_tools().await.expect("list");
        assert_eq!(again.len(), 1);
        assert_eq!(again[0].command.as_deref(), Some("npx"));

        let second = sample_tool("github");
        let listed = store
            .save_connected_tools(vec![second])
            .await
            .expect("save subset");
        assert_eq!(listed.len(), 2);
        let notion = listed.iter().find(|row| row.id == "cursor:notion").unwrap();
        let github = listed.iter().find(|row| row.id == "cursor:github").unwrap();
        assert!(!notion.enabled);
        assert!(!notion.is_active);
        assert!(github.enabled);
        assert!(github.is_active);
    }

    #[tokio::test]
    async fn payload_does_not_store_env_values() {
        let store = ExperienceStore::memory().expect("memory db");
        let mut tool = sample_tool("secretive");
        tool.detail = Some("env keys: API_KEY".into());
        let listed = store.save_connected_tools(vec![tool]).await.expect("save");
        let blob = serde_json::to_string(&listed[0]).expect("json");
        assert!(!blob.contains("sk-"));
        assert!(blob.contains("env keys: API_KEY"));
    }

    #[tokio::test]
    async fn save_selected_tools_bulk_writes_type_and_sync() {
        let store = ExperienceStore::memory().expect("memory db");
        let listed = store
            .save_selected_tools(vec![sample_tool("notion"), sample_model(), sample_cli()])
            .await
            .expect("bulk save");
        assert_eq!(listed.len(), 3);

        let mcp = listed.iter().find(|row| row.id == "cursor:notion").unwrap();
        assert_eq!(mcp.tool_type, "mcp");
        assert_eq!(mcp.config_path.as_deref(), Some("~/.cursor/mcp.json"));
        assert!(mcp.is_active);
        assert!(!mcp.last_synced.is_empty());

        let model = listed
            .iter()
            .find(|row| row.id == "ollama:llama3.1:8b")
            .unwrap();
        assert_eq!(model.tool_type, "model");

        let cli = listed.iter().find(|row| row.id == "system:git").unwrap();
        assert_eq!(cli.tool_type, "cli");
        assert_eq!(cli.config_path.as_deref(), Some("/usr/bin/git"));

        let blob = serde_json::to_string(&mcp).expect("json");
        assert!(blob.contains("\"type\":\"mcp\""));
    }
}
