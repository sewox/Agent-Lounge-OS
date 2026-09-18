use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use tokio::process::Command;

use super::probe::{find_executable, first_existing, repo_root_from_crate};
use crate::models::{IndexSnapshot, ProjectList, ProjectSummary, ServiceHealth, ServiceId};

const CLI_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct MemoryBridge {
    binary: PathBuf,
}

impl MemoryBridge {
    pub fn discover() -> Result<Self> {
        Self::discover_from(repo_root_from_crate())
    }

    pub fn discover_from(repo_root: impl AsRef<Path>) -> Result<Self> {
        let binary = resolve_binary(repo_root.as_ref())?;
        Ok(Self { binary })
    }

    pub fn from_binary(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }

    pub fn binary_path(&self) -> &Path {
        &self.binary
    }

    pub fn diagnose(&self) -> ServiceHealth {
        let found = self.binary.is_file();
        ServiceHealth {
            id: ServiceId::MemoryBridge,
            name: "Memory Bridge".into(),
            running: found,
            started_by_us: false,
            endpoint: self.binary.display().to_string(),
            detail: found.then(|| "codebase-memory-mcp hazır".into()),
            error: (!found).then(|| format!("binary yok: {}", self.binary.display())),
        }
    }

    pub async fn index_repository(&self, repo_path: impl AsRef<Path>) -> Result<IndexSnapshot> {
        let repo_path = repo_path
            .as_ref()
            .canonicalize()
            .with_context(|| "repo_path çözümlenemedi")?;
        let stdout = self
            .run_cli(&[
                "index_repository",
                "--repo-path",
                repo_path.to_str().context("repo_path UTF-8 değil")?,
                "--mode",
                "fast",
                "--format",
                "json",
            ])
            .await?;
        parse_index_stdout(&stdout)
    }

    pub async fn list_projects(&self) -> Result<Vec<ProjectSummary>> {
        let stdout = self.run_cli(&["list_projects", "--format", "json"]).await?;
        let payload = parse_cli_json(&stdout)?;
        let list: ProjectList =
            serde_json::from_value(payload).context("project listesi çözülemedi")?;
        Ok(list.projects)
    }

    async fn run_cli(&self, tool_args: &[&str]) -> Result<String> {
        if !self.binary.is_file() {
            bail!("codebase-memory-mcp yok: {}", self.binary.display());
        }

        let mut command = Command::new(&self.binary);
        command
            .arg("cli")
            .args(tool_args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let output = tokio::time::timeout(CLI_TIMEOUT, command.output())
            .await
            .context("codebase-memory-mcp zaman aşımı")?
            .with_context(|| format!("subprocess başarısız: {}", self.binary.display()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            bail!(
                "codebase-memory-mcp {} ile çıktı. stderr: {}",
                output.status,
                stderr.trim()
            );
        }

        if stdout.trim().is_empty() {
            bail!("codebase-memory-mcp stdout boş. stderr: {}", stderr.trim());
        }

        Ok(stdout)
    }
}

fn resolve_binary(repo_root: &Path) -> Result<PathBuf> {
    if let Ok(from_env) = std::env::var("LOUNGE_MEMORY_BIN") {
        return Ok(PathBuf::from(from_env));
    }

    let mut candidates = vec![
        repo_root.join("bridge/codebase-memory-mcp"),
        repo_root.join("bridge/codebase-memory-mcp.exe"),
    ];
    if let Some(on_path) = find_executable("codebase-memory-mcp") {
        candidates.push(on_path);
    }

    first_existing(candidates)
        .ok_or_else(|| anyhow::anyhow!("codebase-memory-mcp bulunamadı (bridge/ veya PATH)"))
}

pub fn parse_index_stdout(stdout: &str) -> Result<IndexSnapshot> {
    let payload = parse_cli_json(stdout)?;
    index_snapshot_from_value(payload)
}

pub fn parse_cli_json(stdout: &str) -> Result<Value> {
    let json_slice = extract_json_slice(stdout).ok_or_else(|| {
        anyhow::anyhow!("stdout içinde JSON nesnesi yok: {}", truncate(stdout, 240))
    })?;
    let value: Value = serde_json::from_str(json_slice)
        .with_context(|| format!("JSON parse edilemedi: {}", truncate(json_slice, 240)))?;
    Ok(unwrap_cli_envelope(value))
}

fn unwrap_cli_envelope(value: Value) -> Value {
    if let Some(result) = value.get("result") {
        return unwrap_cli_envelope(result.clone());
    }

    if let Some(text) = value
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("text"))
        .and_then(Value::as_str)
    {
        if let Ok(inner) = serde_json::from_str::<Value>(text) {
            return unwrap_cli_envelope(inner);
        }
    }

    value
}

fn index_snapshot_from_value(value: Value) -> Result<IndexSnapshot> {
    let project = value
        .get("project")
        .or_else(|| value.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    Ok(IndexSnapshot {
        project,
        status: value
            .get("status")
            .and_then(Value::as_str)
            .map(str::to_string),
        nodes: as_u64(&value, "nodes"),
        edges: as_u64(&value, "edges"),
        files: value.get("files").and_then(Value::as_u64),
    })
}

fn as_u64(value: &Value, key: &str) -> u64 {
    value
        .get(key)
        .and_then(|item| item.as_u64().or_else(|| item.as_f64().map(|n| n as u64)))
        .unwrap_or(0)
}

fn extract_json_slice(stdout: &str) -> Option<&str> {
    let start = stdout.find('{')?;
    let slice = stdout.get(start..)?;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;
    for (idx, ch) in slice.char_indices() {
        match ch {
            '\\' if in_string => escape = !escape,
            '"' if !escape => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(&slice[..=idx]);
                }
            }
            _ => escape = false,
        }
        if ch != '\\' {
            escape = false;
        }
    }
    None
}

fn truncate(input: &str, max: usize) -> String {
    let trimmed = input.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    format!("{}…", trimmed.chars().take(max).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_direct_index_payload() {
        let stdout =
            r#"{"project":"agent-lounge-os","status":"indexed","nodes":42,"edges":7,"files":12}"#;
        let snap = parse_index_stdout(stdout).unwrap();
        assert_eq!(snap.project, "agent-lounge-os");
        assert_eq!(snap.nodes, 42);
        assert_eq!(snap.edges, 7);
        assert_eq!(snap.files, Some(12));
        assert_eq!(snap.status.as_deref(), Some("indexed"));
    }

    #[test]
    fn unwraps_mcp_content_envelope() {
        let stdout = r#"{"content":[{"type":"text","text":"{\"project\":\"crm\",\"nodes\":9540,\"edges\":27046}"}]}"#;
        let snap = parse_index_stdout(stdout).unwrap();
        assert_eq!(snap.project, "crm");
        assert_eq!(snap.nodes, 9540);
        assert_eq!(snap.edges, 27046);
    }

    #[test]
    fn unwraps_jsonrpc_result_envelope() {
        let stdout = r#"{"jsonrpc":"2.0","result":{"content":[{"type":"text","text":"{\"name\":\"hotspot\",\"nodes\":1,\"edges\":2}"}]}}"#;
        let snap = parse_index_stdout(stdout).unwrap();
        assert_eq!(snap.project, "hotspot");
        assert_eq!(snap.nodes, 1);
    }

    #[test]
    fn ignores_stderr_style_prefix_before_json() {
        let stdout = "level=info msg=mem.init\n{\"project\":\"x\",\"nodes\":3,\"edges\":1}\n";
        let snap = parse_index_stdout(stdout).unwrap();
        assert_eq!(snap.nodes, 3);
    }

    #[test]
    fn rejects_empty_stdout() {
        assert!(parse_index_stdout("   ").is_err());
    }

    #[tokio::test]
    async fn lists_projects_when_binary_present() {
        let Ok(bridge) = MemoryBridge::discover() else {
            return;
        };
        if !bridge.binary_path().is_file() {
            return;
        }
        let projects = bridge.list_projects().await.expect("list_projects parse");
        assert!(projects.iter().all(|project| !project.name.is_empty()));
    }
}
