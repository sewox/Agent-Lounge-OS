//! PR-Connect-1 (F1): plan → apply → verify agent MCP config writes.
//!
//! Local UI / CLI only — never exposed as an MCP tool or deep link.
//! Allowed while Halted (CC-5). Touches only `agent-lounge-os` key + rule files.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::models::AgentMcpStatus;
use crate::services::agent_health::{check_client_entries, ExpectedLoungeMcp};
use crate::services::agent_mcp_paths::{
    client_supports_rule_file, config_targets_for_scope, jsonc_write_would_lose_comments,
    parse_jsonc, primary_write_target, rule_file_targets, AgentHealthRoots, ClientConfigTarget,
    ConnectScope, LOUNGE_MCP_SERVER_KEY, SUPPORTED_CLIENTS,
};
use crate::services::health_filters::mask_home_path_with;

/// Embedded English rule text (same bytes as docs/agent-rules/agent-lounge.md).
pub const AGENT_RULE_MARKDOWN: &str = include_str!("../../../docs/agent-rules/agent-lounge.md");

const DEFAULT_LOUNGE_MCP_URL: &str = "http://127.0.0.1:18791";

/// Cursor `.mdc` front matter + body.
fn cursor_rule_file_body() -> String {
    format!(
        "---\ndescription: Agent Lounge OS MCP usage\nalwaysApply: true\n---\n\n{AGENT_RULE_MARKDOWN}"
    )
}

fn claude_skill_body() -> String {
    format!(
        "---\nname: agent-lounge\ndescription: Agent Lounge OS MCP — when to use Lounge tools\n---\n\n{AGENT_RULE_MARKDOWN}"
    )
}

fn antigravity_rule_body() -> String {
    AGENT_RULE_MARKDOWN.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConnectAction {
    Connect,
    Disconnect,
    Print,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectFilePlan {
    /// Masked path only (`~/…`). Never serialize absolute home paths to UI/IPC.
    pub path_masked: String,
    pub kind: String,
    pub exists: bool,
    pub would_create: bool,
    pub jsonc_comment_loss: bool,
    /// Redacted `agent-lounge-os` entry only (env values `***`, command home-masked).
    pub entry_before: Option<Value>,
    /// Redacted `agent-lounge-os` entry only after the planned change.
    pub entry_after: Option<Value>,
    /// Structural diff of our entry (not a line-position file diff).
    pub diff: String,
    pub backup_path_masked: Option<String>,
    /// Absolute path for local apply only — never sent over IPC.
    #[serde(skip)]
    pub(crate) path: PathBuf,
    /// Full file bytes to write on apply — never sent over IPC.
    #[serde(skip)]
    pub(crate) write_contents: Option<String>,
    #[serde(skip)]
    pub(crate) backup_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectPlan {
    pub client: String,
    pub scope: String,
    pub action: ConnectAction,
    pub dry_run: bool,
    pub command_path_masked: Option<String>,
    pub files: Vec<ConnectFilePlan>,
    pub warnings: Vec<String>,
    /// True when any target would lose JSONC comments without consent / --force-lossy-jsonc.
    pub requires_jsonc_consent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectResult {
    pub plan: ConnectPlan,
    pub applied: bool,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub client: String,
    pub scope: ConnectScope,
    pub dry_run: bool,
    /// Explicit consent to rewrite JSONC and drop comments (CLI flag / UI "Yine de Yaz").
    pub force_lossy_jsonc: bool,
    /// Override binary path (tests). When None, use ExpectedLoungeMcp::current().
    pub command_path: Option<PathBuf>,
    pub expected: Option<ExpectedLoungeMcp>,
}

fn validate_client(client: &str) -> Result<(), String> {
    if SUPPORTED_CLIENTS.contains(&client) {
        Ok(())
    } else {
        Err(format!(
            "unsupported client {client:?}; expected one of {}",
            SUPPORTED_CLIENTS.join(", ")
        ))
    }
}

fn resolve_command_path(opts: &ConnectOptions) -> Result<PathBuf, String> {
    if let Some(p) = &opts.command_path {
        if p.is_file() {
            return Ok(p.clone());
        }
        return Err(format!("lounge-mcp binary not found at {}", p.display()));
    }
    let expected = opts
        .expected
        .clone()
        .unwrap_or_else(ExpectedLoungeMcp::current);
    expected.path.ok_or_else(|| {
        "canonical lounge-mcp binary not found beside this process or on PATH".into()
    })
}

fn stamp() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

fn backup_sibling(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("config");
    path.with_file_name(format!("{name}.lounge-bak-{}", stamp()))
}

fn mask_path(path: &Path, roots: &AgentHealthRoots) -> String {
    mask_home_path_with(&path.to_string_lossy(), &roots.mask_homes())
}

/// Extract the `agent-lounge-os` server object from a parsed config root.
fn extract_lounge_entry(root: &Value, servers_key: &str) -> Option<Value> {
    root.get(servers_key)
        .and_then(Value::as_object)
        .and_then(|m| m.get(LOUNGE_MCP_SERVER_KEY))
        .cloned()
}

/// Redact env values to `***` and mask home prefixes in command/string fields.
fn redact_lounge_entry(entry: &Value, roots: &AgentHealthRoots) -> Value {
    let mut out = entry.clone();
    let homes = roots.mask_homes();
    if let Some(obj) = out.as_object_mut() {
        if let Some(cmd) = obj.get("command").and_then(Value::as_str) {
            obj.insert(
                "command".into(),
                Value::String(mask_home_path_with(cmd, &homes)),
            );
        }
        if let Some(env) = obj.get_mut("env").and_then(Value::as_object_mut) {
            for (_k, v) in env.iter_mut() {
                *v = Value::String("***".into());
            }
        }
        // Redact any other string that looks path-like under home.
        for (key, v) in obj.clone().iter() {
            if key == "command" || key == "env" || key == "args" {
                continue;
            }
            if let Some(s) = v.as_str() {
                let masked = mask_home_path_with(s, &homes);
                if masked != s {
                    obj.insert(key.clone(), Value::String(masked));
                }
            }
        }
    }
    out
}

/// Structural diff of our redacted entry (not a full-file line diff).
fn structural_entry_diff(
    path_label: &str,
    before: Option<&Value>,
    after: Option<&Value>,
) -> String {
    let before_s = before
        .map(|v| serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into()))
        .unwrap_or_else(|| "null".into());
    let after_s = after
        .map(|v| serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into()))
        .unwrap_or_else(|| "null".into());
    if before_s == after_s {
        return format!("--- {path_label} / {LOUNGE_MCP_SERVER_KEY} (unchanged)\n");
    }
    let mut out = String::new();
    out.push_str(&format!(
        "--- a/{path_label}#{LOUNGE_MCP_SERVER_KEY}\n+++ b/{path_label}#{LOUNGE_MCP_SERVER_KEY}\n"
    ));
    let before_lines: Vec<&str> = before_s.lines().collect();
    let after_lines: Vec<&str> = after_s.lines().collect();
    let max = before_lines.len().max(after_lines.len());
    for i in 0..max {
        match (before_lines.get(i), after_lines.get(i)) {
            (Some(b), Some(a)) if b == a => out.push_str(&format!(" {b}\n")),
            (Some(b), Some(a)) => out.push_str(&format!("-{b}\n+{a}\n")),
            (Some(b), None) => out.push_str(&format!("-{b}\n")),
            (None, Some(a)) => out.push_str(&format!("+{a}\n")),
            (None, None) => {}
        }
    }
    out
}

fn lounge_server_entry(command: &Path) -> Value {
    // serde_json escapes Windows backslashes correctly (CC-10 / CN10).
    json!({
        "command": command.to_string_lossy(),
        "args": [],
        "env": {
            "LOUNGE_MCP_URL": DEFAULT_LOUNGE_MCP_URL
        }
    })
}

fn parse_config_raw(raw: &str, allow_jsonc: bool) -> Result<Value, String> {
    if allow_jsonc {
        if let Ok(v) = serde_json::from_str::<Value>(raw) {
            return Ok(v);
        }
        return parse_jsonc(raw).map_err(|e| format!("jsonc parse failed: {e}"));
    }
    serde_json::from_str(raw).map_err(|e| format!("json parse failed: {e}"))
}

fn ensure_servers_map<'a>(
    root: &'a mut Value,
    servers_key: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    if !root.is_object() {
        *root = json!({});
    }
    let obj = root
        .as_object_mut()
        .ok_or_else(|| "config root must be a JSON object".to_string())?;
    if !obj.contains_key(servers_key) {
        obj.insert(servers_key.to_string(), json!({}));
    }
    let servers = obj
        .get_mut(servers_key)
        .ok_or_else(|| format!("missing {servers_key}"))?;
    if !servers.is_object() {
        *servers = json!({});
    }
    servers
        .as_object_mut()
        .ok_or_else(|| format!("{servers_key} must be an object"))
}

fn upsert_lounge_key(root: &mut Value, servers_key: &str, command: &Path) -> Result<(), String> {
    let servers = ensure_servers_map(root, servers_key)?;
    servers.insert(
        LOUNGE_MCP_SERVER_KEY.to_string(),
        lounge_server_entry(command),
    );
    Ok(())
}

fn remove_lounge_key(root: &mut Value, servers_key: &str) -> bool {
    let Some(obj) = root.as_object_mut() else {
        return false;
    };
    let Some(servers) = obj.get_mut(servers_key).and_then(Value::as_object_mut) else {
        return false;
    };
    servers.remove(LOUNGE_MCP_SERVER_KEY).is_some()
}

fn pretty_json(value: &Value) -> Result<String, String> {
    let mut s = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    if !s.ends_with('\n') {
        s.push('\n');
    }
    Ok(s)
}

fn rule_body_for_client(client: &str) -> String {
    match client {
        "cursor" => cursor_rule_file_body(),
        "claude_code" => claude_skill_body(),
        "antigravity" => antigravity_rule_body(),
        _ => AGENT_RULE_MARKDOWN.to_string(),
    }
}

fn plan_mcp_file(
    target: &ClientConfigTarget,
    roots: &AgentHealthRoots,
    command: &Path,
    action: ConnectAction,
    force_lossy_jsonc: bool,
    scope: ConnectScope,
) -> Result<ConnectFilePlan, String> {
    let exists = target.path.is_file();
    let would_create = !exists && action == ConnectAction::Connect;
    if would_create && scope != ConnectScope::Global {
        return Err(format!(
            "config file missing at {} — create only allowed with --global (directory must exist)",
            mask_path(&target.path, roots)
        ));
    }
    if would_create {
        if let Some(parent) = target.path.parent() {
            if !parent.exists() {
                return Err(format!(
                    "config directory missing: {} — create the directory first",
                    mask_path(parent, roots)
                ));
            }
        }
    }

    let raw_before = if exists {
        Some(fs::read_to_string(&target.path).map_err(|e| format!("read failed: {e}"))?)
    } else {
        None
    };

    let jsonc_comment_loss = raw_before
        .as_deref()
        .map(|raw| target.allow_jsonc && jsonc_write_would_lose_comments(raw))
        .unwrap_or(false);
    let _ = force_lossy_jsonc;

    let parsed_before = if let Some(raw) = &raw_before {
        Some(parse_config_raw(raw, target.allow_jsonc)?)
    } else {
        None
    };

    let entry_before_raw = parsed_before
        .as_ref()
        .and_then(|v| extract_lounge_entry(v, target.servers_key));
    let entry_before = entry_before_raw
        .as_ref()
        .map(|e| redact_lounge_entry(e, roots));

    let (write_contents, entry_after_raw) = match action {
        ConnectAction::Print => (None, entry_before_raw.clone()),
        ConnectAction::Connect => {
            let mut value = parsed_before.clone().unwrap_or_else(|| json!({}));
            upsert_lounge_key(&mut value, target.servers_key, command)?;
            let entry = extract_lounge_entry(&value, target.servers_key);
            (Some(pretty_json(&value)?), entry)
        }
        ConnectAction::Disconnect => {
            if let Some(mut value) = parsed_before.clone() {
                remove_lounge_key(&mut value, target.servers_key);
                (Some(pretty_json(&value)?), None)
            } else {
                (None, None)
            }
        }
    };

    let entry_after = entry_after_raw
        .as_ref()
        .map(|e| redact_lounge_entry(e, roots));
    let label = mask_path(&target.path, roots);
    let diff = structural_entry_diff(&label, entry_before.as_ref(), entry_after.as_ref());

    let backup = if exists && action != ConnectAction::Print {
        Some(backup_sibling(&target.path))
    } else {
        None
    };

    Ok(ConnectFilePlan {
        path_masked: label,
        kind: "mcp_config".into(),
        exists,
        would_create,
        jsonc_comment_loss,
        entry_before,
        entry_after,
        diff,
        backup_path_masked: backup.as_ref().map(|p| mask_path(p, roots)),
        path: target.path.clone(),
        write_contents,
        backup_path: backup,
    })
}

fn plan_rule_file(
    path: PathBuf,
    roots: &AgentHealthRoots,
    client: &str,
    action: ConnectAction,
) -> ConnectFilePlan {
    let exists = path.is_file();
    let homes = roots.mask_homes();
    let before_raw = if exists {
        fs::read_to_string(&path).ok()
    } else {
        None
    };
    let after_raw = match action {
        ConnectAction::Connect => Some(rule_body_for_client(client)),
        ConnectAction::Disconnect => None,
        ConnectAction::Print => before_raw.clone(),
    };
    // Rule files are our text only — still mask any home substrings; no env secrets.
    let entry_before = before_raw
        .as_ref()
        .map(|s| Value::String(mask_home_in_text_local(s, &homes)));
    let entry_after = after_raw
        .as_ref()
        .map(|s| Value::String(mask_home_in_text_local(s, &homes)));
    let label = mask_path(&path, roots);
    let backup = if exists && action != ConnectAction::Print {
        Some(backup_sibling(&path))
    } else {
        None
    };
    ConnectFilePlan {
        path_masked: label.clone(),
        kind: "rule_file".into(),
        exists,
        would_create: !exists && action == ConnectAction::Connect,
        jsonc_comment_loss: false,
        entry_before: entry_before.clone(),
        entry_after: entry_after.clone(),
        diff: structural_entry_diff(&label, entry_before.as_ref(), entry_after.as_ref()),
        backup_path_masked: backup.as_ref().map(|p| mask_path(p, roots)),
        path,
        write_contents: after_raw,
        backup_path: backup,
    }
}

fn mask_home_in_text_local(text: &str, homes: &[String]) -> String {
    crate::services::health_filters::mask_home_in_text(text, homes)
}

/// Build a connect/disconnect/print plan without writing.
pub fn plan_agent_connect(
    roots: &AgentHealthRoots,
    opts: &ConnectOptions,
    action: ConnectAction,
) -> Result<ConnectPlan, String> {
    validate_client(&opts.client)?;
    let command = match action {
        ConnectAction::Connect | ConnectAction::Print => Some(resolve_command_path(opts)?),
        ConnectAction::Disconnect => opts.command_path.clone().or_else(|| {
            opts.expected
                .clone()
                .unwrap_or_else(ExpectedLoungeMcp::current)
                .path
        }),
    };

    let mut files = Vec::new();
    let mut warnings = Vec::new();

    // Primary MCP target for this scope.
    match primary_write_target(&opts.client, roots, opts.scope) {
        Ok(target) => {
            let cmd = command
                .clone()
                .unwrap_or_else(|| PathBuf::from("lounge-mcp"));
            let plan = plan_mcp_file(
                &target,
                roots,
                &cmd,
                action.clone(),
                opts.force_lossy_jsonc,
                opts.scope,
            )?;
            if plan.jsonc_comment_loss
                && !opts.force_lossy_jsonc
                && action == ConnectAction::Connect
            {
                warnings.push(format!(
                    "Yorumlar kaybolacak: {} — use --force-lossy-jsonc or print",
                    plan.path_masked
                ));
            }
            files.push(plan);
        }
        Err(err) => {
            if action == ConnectAction::Disconnect {
                warnings.push(err);
            } else {
                return Err(err);
            }
        }
    }

    if client_supports_rule_file(&opts.client) {
        for path in rule_file_targets(&opts.client, roots, opts.scope) {
            // For global Antigravity, prefer first existing rule path; else first.
            if opts.client == "antigravity"
                && opts.scope == ConnectScope::Global
                && !path.is_file()
                && files.iter().any(|f| f.kind == "rule_file")
            {
                continue;
            }
            files.push(plan_rule_file(path, roots, &opts.client, action.clone()));
            break;
        }
    } else {
        warnings.push(
            "this client has no rule-file support; MCP server instructions cover usage (equal connected status)"
                .into(),
        );
    }

    let requires_jsonc_consent = files.iter().any(|f| f.jsonc_comment_loss)
        && !opts.force_lossy_jsonc
        && action == ConnectAction::Connect;

    let cmd_masked = command.as_ref().map(|p| mask_path(p, roots));

    Ok(ConnectPlan {
        client: opts.client.clone(),
        scope: opts.scope.as_str().into(),
        action,
        dry_run: opts.dry_run,
        command_path_masked: cmd_masked,
        files,
        warnings,
        requires_jsonc_consent,
    })
}

/// Atomic write: temp + fsync + rename; Windows uses ReplaceFileW (no retries).
pub fn write_atomic_with_backup(
    path: &Path,
    contents: &[u8],
    backup: Option<&Path>,
) -> Result<(), String> {
    if let Some(bak) = backup {
        if path.is_file() {
            fs::copy(path, bak).map_err(|e| format!("backup failed: {e}"))?;
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }

    let parent = path
        .parent()
        .ok_or_else(|| "path has no parent".to_string())?;
    let file_name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("config");
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = parent.join(format!(
        "{file_name}.lounge-tmp-{}-{nanos}",
        std::process::id()
    ));

    {
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp)
            .map_err(|e| format!("create temp: {e}"))?;
        file.write_all(contents)
            .map_err(|e| format!("write temp: {e}"))?;
        file.sync_all().map_err(|e| format!("fsync temp: {e}"))?;
    }

    atomic_replace(&tmp, path).inspect_err(|_e| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(())
}

#[cfg(unix)]
fn fsync_parent_dir(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    let dir = fs::File::open(parent).map_err(|e| format!("open parent dir: {e}"))?;
    dir.sync_all()
        .map_err(|e| format!("fsync parent dir: {e}"))?;
    Ok(())
}

fn atomic_replace(tmp: &Path, dest: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::GetLastError;
        use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

        if !dest.exists() {
            fs::rename(tmp, dest).map_err(|e| format!("rename: {e}"))?;
            return Ok(());
        }
        let wide = |p: &Path| -> Vec<u16> {
            p.as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        };
        let dest_w = wide(dest);
        let tmp_w = wide(tmp);
        // REPLACEFILE_WRITE_THROUGH = 0x00000001
        let ok = unsafe {
            ReplaceFileW(
                dest_w.as_ptr(),
                tmp_w.as_ptr(),
                std::ptr::null(),
                0x0000_0001,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            return Err(format!("ReplaceFileW failed (GetLastError={err})"));
        }
        // ReplaceFileW consumes the replacement file.
        Ok(())
    }
    #[cfg(not(windows))]
    {
        fs::rename(tmp, dest).map_err(|e| format!("rename: {e}"))?;
        fsync_parent_dir(dest)?;
        Ok(())
    }
}

fn apply_file_plan(file: &ConnectFilePlan, action: &ConnectAction) -> Result<(), String> {
    let path = &file.path;
    match action {
        ConnectAction::Print => Ok(()),
        ConnectAction::Connect => {
            let after = file
                .write_contents
                .as_deref()
                .ok_or_else(|| "missing write contents".to_string())?;
            write_atomic_with_backup(path, after.as_bytes(), file.backup_path.as_deref())
        }
        ConnectAction::Disconnect => {
            if file.kind == "rule_file" {
                if path.is_file() {
                    if let Some(bak) = file.backup_path.as_deref() {
                        fs::copy(path, bak).map_err(|e| format!("rule backup: {e}"))?;
                    }
                    fs::remove_file(path).map_err(|e| format!("remove rule: {e}"))?;
                }
                return Ok(());
            }
            if let Some(after) = &file.write_contents {
                write_atomic_with_backup(path, after.as_bytes(), file.backup_path.as_deref())?;
            }
            Ok(())
        }
    }
}

/// Apply a previously built plan (or rebuild + apply).
pub fn apply_agent_connect(
    roots: &AgentHealthRoots,
    opts: &ConnectOptions,
    action: ConnectAction,
) -> Result<ConnectResult, String> {
    let mut plan = plan_agent_connect(roots, opts, action.clone())?;
    if opts.dry_run {
        plan.dry_run = true;
        return Ok(ConnectResult {
            message: "dry-run: no files written".into(),
            plan,
            applied: false,
        });
    }
    if plan.requires_jsonc_consent {
        return Err(
            "JSONC comments would be lost. Pass --force-lossy-jsonc (CLI) or confirm in UI (Yine de Yaz)."
                .into(),
        );
    }
    for file in &plan.files {
        apply_file_plan(file, &action)?;
        log::info!(
            "agent_connect {:?} {} kind={}",
            action,
            file.path_masked,
            file.kind
        );
    }
    Ok(ConnectResult {
        message: match action {
            ConnectAction::Connect => "connected".into(),
            ConnectAction::Disconnect => "disconnected".into(),
            ConnectAction::Print => "print".into(),
        },
        plan,
        applied: action != ConnectAction::Print,
    })
}

/// Print fragment for UI/IPC: masked paths only, env values redacted. No writes.
pub fn print_agent_connect(
    roots: &AgentHealthRoots,
    opts: &ConnectOptions,
) -> Result<Value, String> {
    print_agent_connect_inner(roots, opts, /*include_unmasked_path*/ false)
}

/// CLI print: same as UI plus `path_unmasked_for_cli` for copy-paste installs.
pub fn print_agent_connect_for_cli(
    roots: &AgentHealthRoots,
    opts: &ConnectOptions,
) -> Result<Value, String> {
    print_agent_connect_inner(roots, opts, /*include_unmasked_path*/ true)
}

fn print_agent_connect_inner(
    roots: &AgentHealthRoots,
    opts: &ConnectOptions,
    include_unmasked_path: bool,
) -> Result<Value, String> {
    let plan = plan_agent_connect(roots, opts, ConnectAction::Print)?;
    let command = resolve_command_path(opts)?;
    let target = primary_write_target(&opts.client, roots, opts.scope)?;
    let mut fragment = {
        let mut root = json!({});
        upsert_lounge_key(&mut root, target.servers_key, &command)?;
        root
    };
    // Always redact env values; mask command for UI, keep real command for CLI copy-paste.
    if let Some(entry) = fragment
        .get_mut(target.servers_key)
        .and_then(Value::as_object_mut)
        .and_then(|m| m.get_mut(LOUNGE_MCP_SERVER_KEY))
    {
        let redacted = redact_lounge_entry(entry, roots);
        if include_unmasked_path {
            // Restore real command path for CLI; keep env as ***.
            if let Some(obj) = redacted.as_object() {
                let mut cli_entry = obj.clone();
                cli_entry.insert(
                    "command".into(),
                    Value::String(command.to_string_lossy().into_owned()),
                );
                *entry = Value::Object(cli_entry);
            } else {
                *entry = redacted;
            }
        } else {
            *entry = redacted;
        }
    }
    let mut out = json!({
        "client": opts.client,
        "scope": opts.scope.as_str(),
        "path": mask_path(&target.path, roots),
        "servers_key": target.servers_key,
        "fragment": fragment,
        "rule_files": plan.files.iter().filter(|f| f.kind == "rule_file").map(|f| &f.path_masked).collect::<Vec<_>>(),
        "warnings": plan.warnings,
    });
    if include_unmasked_path {
        out.as_object_mut().unwrap().insert(
            "path_unmasked_for_cli".into(),
            Value::String(target.path.to_string_lossy().into_owned()),
        );
    }
    Ok(out)
}

/// After connect, re-check health for the client (loop test helper).
pub fn verify_connect_health(
    roots: &AgentHealthRoots,
    client: &str,
    expected: &ExpectedLoungeMcp,
) -> Result<AgentMcpStatus, String> {
    let targets = config_targets_for_scope(client, roots, ConnectScope::Global)
        .or_else(|| config_targets_for_scope(client, roots, ConnectScope::Project))
        .ok_or_else(|| format!("no paths for {client}"))?;
    let paths: Vec<PathBuf> = targets.into_iter().map(|t| t.path).collect();
    let allow_jsonc = crate::services::agent_mcp_paths::client_allows_jsonc(client);
    let entries = check_client_entries(client, true, paths, allow_jsonc, expected, roots);
    let status = entries
        .iter()
        .find(|e| e.status == AgentMcpStatus::Ok)
        .map(|e| e.status)
        .or_else(|| entries.first().map(|e| e.status))
        .unwrap_or(AgentMcpStatus::Missing);
    Ok(status)
}

/// CLI entry used by `lounge-mcp connect|disconnect|print`.
pub fn run_connect_cli(args: &[String]) -> Result<(), String> {
    let mut iter = args.iter();
    let action_s = iter.next().ok_or("missing action")?;
    let action = match action_s.as_str() {
        "connect" => ConnectAction::Connect,
        "disconnect" => ConnectAction::Disconnect,
        "print" => ConnectAction::Print,
        other => return Err(format!("unknown action {other}")),
    };
    let client = iter
        .next()
        .ok_or("missing <agent> client id")?
        .to_ascii_lowercase();

    let mut dry_run = false;
    let mut scope = ConnectScope::Global;
    let mut force_lossy_jsonc = false;
    let mut workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    for arg in iter {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            "--global" => scope = ConnectScope::Global,
            "--project" => scope = ConnectScope::Project,
            "--force-lossy-jsonc" => force_lossy_jsonc = true,
            "--help" | "-h" => {
                print_cli_help();
                return Ok(());
            }
            s if s.starts_with("--workspace=") => {
                workspace = PathBuf::from(s.trim_start_matches("--workspace="));
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }

    let roots = AgentHealthRoots::from_env(workspace);
    let opts = ConnectOptions {
        client: client.clone(),
        scope,
        dry_run,
        force_lossy_jsonc,
        command_path: None,
        expected: None,
    };

    match action {
        ConnectAction::Print => {
            let value = print_agent_connect_for_cli(&roots, &opts)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?
            );
            Ok(())
        }
        other => {
            let result = apply_agent_connect(&roots, &opts, other)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?
            );
            Ok(())
        }
    }
}

fn print_cli_help() {
    eprintln!(
        "Usage: lounge-mcp <connect|disconnect|print> <agent> [--dry-run] [--global|--project] [--force-lossy-jsonc]\n\
         Agents: {}\n\
         Default scope: --global",
        SUPPORTED_CLIENTS.join(", ")
    );
}

/// Ensure path does not target `.gitignore` (Connect never touches it).
pub fn path_is_gitignore(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|n| n.eq_ignore_ascii_case(".gitignore"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::agent_health::{
        command_is_exact_lounge_mcp, LOUNGE_MCP_VERSION_MARKER_PREFIX,
    };
    use sha2::{Digest, Sha256};

    fn temp_roots(label: &str) -> (AgentHealthRoots, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("lounge-connect-{}-{}", label, uuid::Uuid::new_v4()));
        let home = base.join("home");
        let appdata = base.join("appdata");
        let workspace = base.join("workspace");
        let xdg = base.join("xdg");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&appdata).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&xdg).unwrap();
        // Parent dirs for global creates — cover Linux (XDG), macOS (Application Support),
        // and Windows (APPDATA) so injectable roots work on every CI OS.
        fs::create_dir_all(home.join(".cursor")).unwrap();
        fs::create_dir_all(home.join(".gemini/config")).unwrap();
        fs::create_dir_all(home.join(".gemini/antigravity")).unwrap();
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::create_dir_all(home.join(".grok")).unwrap();
        fs::create_dir_all(home.join(".codeium/windsurf")).unwrap();
        fs::create_dir_all(home.join("Library/Application Support/Claude")).unwrap();
        fs::create_dir_all(home.join("Library/Application Support/Grok")).unwrap();
        fs::create_dir_all(home.join("Library/Application Support/Grok Bot")).unwrap();
        fs::create_dir_all(home.join("Library/Application Support/Code/User")).unwrap();
        fs::create_dir_all(home.join("Library/Application Support/Windsurf")).unwrap();
        fs::create_dir_all(home.join("Library/Application Support/Antigravity/User")).unwrap();
        fs::create_dir_all(xdg.join("Claude")).unwrap();
        fs::create_dir_all(xdg.join("Code/User")).unwrap();
        fs::create_dir_all(xdg.join("Windsurf")).unwrap();
        fs::create_dir_all(xdg.join("Grok")).unwrap();
        fs::create_dir_all(xdg.join("Grok Bot")).unwrap();
        fs::create_dir_all(xdg.join("Antigravity/User")).unwrap();
        fs::create_dir_all(appdata.join("Claude")).unwrap();
        fs::create_dir_all(appdata.join("Grok")).unwrap();
        fs::create_dir_all(appdata.join("Grok Bot")).unwrap();
        fs::create_dir_all(appdata.join("Code/User")).unwrap();
        fs::create_dir_all(appdata.join("Windsurf")).unwrap();
        fs::create_dir_all(appdata.join("Antigravity/User")).unwrap();
        let roots = AgentHealthRoots {
            home,
            appdata: Some(appdata),
            xdg_config: Some(xdg),
            workspace,
            path_dirs: Vec::new(),
        };
        (roots, base)
    }

    fn fake_binary(path: &Path, version: &str) -> String {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(path).unwrap();
        writeln!(
            f,
            "#!/bin/sh\necho fake\n{LOUNGE_MCP_VERSION_MARKER_PREFIX}{version}\n"
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(path, perms).unwrap();
        }
        let bytes = fs::read(path).unwrap();
        let digest = Sha256::digest(&bytes);
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn opts(client: &str, _roots: &AgentHealthRoots, bin: &Path, sha: &str) -> ConnectOptions {
        ConnectOptions {
            client: client.into(),
            scope: ConnectScope::Global,
            dry_run: false,
            force_lossy_jsonc: false,
            command_path: Some(bin.to_path_buf()),
            expected: Some(ExpectedLoungeMcp {
                version: "0.1.0".into(),
                sha256: Some(sha.into()),
                path: Some(bin.to_path_buf()),
            }),
        }
    }

    #[test]
    fn each_client_has_injectable_paths_without_env_mutation() {
        let (roots, _base) = temp_roots("paths");
        for client in SUPPORTED_CLIENTS {
            let targets =
                crate::services::agent_mcp_paths::all_config_targets(client, &roots).expect(client);
            assert!(!targets.is_empty(), "{client} must have at least one path");
            for t in &targets {
                assert!(
                    t.path.starts_with(&roots.home)
                        || t.path.starts_with(&roots.workspace)
                        || roots
                            .xdg_config
                            .as_ref()
                            .is_some_and(|x| t.path.starts_with(x))
                        || roots
                            .appdata
                            .as_ref()
                            .is_some_and(|a| t.path.starts_with(a)),
                    "{client} path escaped injected roots: {}",
                    t.path.display()
                );
            }
        }
    }

    #[test]
    fn connect_preserves_other_servers_and_is_idempotent() {
        let (roots, base) = temp_roots("idem");
        let bin = base.join("bin/lounge-mcp");
        let sha = fake_binary(&bin, "0.1.0");
        let cfg = roots.home.join(".cursor/mcp.json");
        fs::write(
            &cfg,
            r#"{
  "mcpServers": {
    "other": { "command": "npx", "args": ["-y", "x"] }
  }
}
"#,
        )
        .unwrap();

        let o = opts("cursor", &roots, &bin, &sha);
        apply_agent_connect(&roots, &o, ConnectAction::Connect).unwrap();
        let once = fs::read_to_string(&cfg).unwrap();
        assert!(once.contains("other"));
        assert!(once.contains(LOUNGE_MCP_SERVER_KEY));
        assert!(once.contains("LOUNGE_MCP_URL"));

        apply_agent_connect(&roots, &o, ConnectAction::Connect).unwrap();
        let twice = fs::read_to_string(&cfg).unwrap();
        assert_eq!(once, twice, "second connect must be idempotent");

        let status = verify_connect_health(&roots, "cursor", o.expected.as_ref().unwrap()).unwrap();
        assert_eq!(status, AgentMcpStatus::Ok);
    }

    #[test]
    fn disconnect_removes_only_our_key() {
        let (roots, base) = temp_roots("disc");
        let bin = base.join("bin/lounge-mcp");
        let sha = fake_binary(&bin, "0.1.0");
        let cfg = roots.home.join(".cursor/mcp.json");
        let o = opts("cursor", &roots, &bin, &sha);
        apply_agent_connect(&roots, &o, ConnectAction::Connect).unwrap();
        // inject sibling
        let mut v: Value = serde_json::from_str(&fs::read_to_string(&cfg).unwrap()).unwrap();
        v["mcpServers"]["keep-me"] = json!({"command": "echo"});
        fs::write(&cfg, pretty_json(&v).unwrap()).unwrap();

        apply_agent_connect(&roots, &o, ConnectAction::Disconnect).unwrap();
        let after = fs::read_to_string(&cfg).unwrap();
        assert!(!after.contains(LOUNGE_MCP_SERVER_KEY));
        assert!(after.contains("keep-me"));
    }

    #[test]
    fn jsonc_refuses_without_force_and_print_is_valid_json() {
        let (roots, base) = temp_roots("jsonc");
        let bin = base.join("bin/lounge-mcp");
        let sha = fake_binary(&bin, "0.1.0");
        let cfg = roots.home.join(".cursor/mcp.json");
        fs::write(
            &cfg,
            r#"{
  // keep me
  "mcpServers": {
    "other": { "command": "npx" },
  },
}
"#,
        )
        .unwrap();
        let mut o = opts("cursor", &roots, &bin, &sha);
        let err = apply_agent_connect(&roots, &o, ConnectAction::Connect).unwrap_err();
        assert!(
            err.contains("JSONC") || err.contains("force-lossy"),
            "unexpected: {err}"
        );
        assert!(fs::read_to_string(&cfg).unwrap().contains("// keep me"));

        let printed = print_agent_connect(&roots, &o).unwrap();
        let frag = &printed["fragment"];
        assert!(frag.is_object());
        serde_json::to_string(frag).expect("print fragment serializes");

        o.force_lossy_jsonc = true;
        apply_agent_connect(&roots, &o, ConnectAction::Connect).unwrap();
        let after = fs::read_to_string(&cfg).unwrap();
        assert!(after.contains(LOUNGE_MCP_SERVER_KEY));
        assert!(after.contains("other"));
        assert!(!after.contains("// keep me"));
        // backup created
        let parent = cfg.parent().unwrap();
        let bak = fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().contains("lounge-bak-"));
        assert!(bak, "timestamped backup required");
    }

    #[test]
    fn dry_run_writes_nothing() {
        let (roots, base) = temp_roots("dry");
        let bin = base.join("bin/lounge-mcp");
        let sha = fake_binary(&bin, "0.1.0");
        let cfg = roots.home.join(".cursor/mcp.json");
        let before = br#"{"mcpServers":{}}"#;
        fs::write(&cfg, before).unwrap();
        let mut o = opts("cursor", &roots, &bin, &sha);
        o.dry_run = true;
        let result = apply_agent_connect(&roots, &o, ConnectAction::Connect).unwrap();
        assert!(!result.applied);
        assert_eq!(fs::read(&cfg).unwrap(), before);
        assert!(!result.plan.files.is_empty());
        assert!(!result.plan.files[0].diff.is_empty());
        assert!(result.plan.files[0].entry_after.is_some());
    }

    #[test]
    fn plan_ipc_redacts_sibling_secrets_and_masks_home() {
        let (roots, base) = temp_roots("secret");
        let bin = base.join("bin/lounge-mcp");
        let sha = fake_binary(&bin, "0.1.0");
        let cfg = roots.home.join(".cursor/mcp.json");
        let secret = "sk-live-FAKESECRET-sibling-env-value";
        let home_s = roots.home.to_string_lossy().replace('\\', "/");
        let cfg_json = json!({
            "mcpServers": {
                "sibling": {
                    "command": "npx",
                    "env": { "API_KEY": secret, "TOKEN": secret }
                },
                "agent-lounge-os": {
                    "command": format!("{home_s}/bin/old-lounge-mcp"),
                    "args": [],
                    "env": { "LOUNGE_MCP_URL": "http://127.0.0.1:18791", "LOUNGE_DB_PATH": format!("{home_s}/secret.db") }
                }
            }
        });
        fs::write(&cfg, pretty_json(&cfg_json).unwrap()).unwrap();

        let o = opts("cursor", &roots, &bin, &sha);
        let plan = plan_agent_connect(&roots, &o, ConnectAction::Connect).unwrap();
        let ipc = serde_json::to_string_pretty(&plan).expect("plan serializes for IPC");
        assert!(
            !ipc.contains(secret),
            "sibling secret must never appear in IPC plan: {ipc}"
        );
        assert!(
            !ipc.contains(home_s.as_str()),
            "unmasked home must never appear in IPC plan: {ipc}"
        );
        assert!(
            !ipc.contains("path_unmasked"),
            "no unmasked path key in plan IPC"
        );
        // Full file body fields must be gone.
        assert!(
            !ipc.contains("\"before\""),
            "full before must not be serialized"
        );
        assert!(
            !ipc.contains("\"after\""),
            "full after must not be serialized"
        );
        assert!(!ipc.contains("write_contents"));

        let file = &plan.files[0];
        assert_eq!(file.kind, "mcp_config");
        let entry_before = file.entry_before.as_ref().expect("entry_before");
        let entry_after = file.entry_after.as_ref().expect("entry_after");
        // Only our entry — sibling must not be present as a peer object in entry_* .
        let before_s = serde_json::to_string(entry_before).unwrap();
        let after_s = serde_json::to_string(entry_after).unwrap();
        assert!(!before_s.contains("sibling"));
        assert!(!after_s.contains("sibling"));
        assert!(!before_s.contains(secret));
        assert!(!after_s.contains(secret));
        assert!(before_s.contains("***"));
        assert!(after_s.contains("***"));
        assert!(file.path_masked.starts_with("~/") || file.path_masked.contains('~'));
        assert!(file.diff.contains(LOUNGE_MCP_SERVER_KEY) || file.diff.contains("command"));

        let ui_print = print_agent_connect(&roots, &o).unwrap();
        let ui_s = serde_json::to_string(&ui_print).unwrap();
        assert!(!ui_s.contains(secret));
        assert!(!ui_s.contains(home_s.as_str()));
        assert!(ui_print.get("path_unmasked_for_cli").is_none());
        assert!(
            ui_print["path"].as_str().unwrap().starts_with("~/")
                || ui_print["path"].as_str().unwrap().contains('~')
        );

        let cli_print = print_agent_connect_for_cli(&roots, &o).unwrap();
        assert!(cli_print.get("path_unmasked_for_cli").is_some());
        let cli_s = serde_json::to_string(&cli_print).unwrap();
        assert!(
            !cli_s.contains(secret),
            "CLI print must still redact env secrets"
        );
    }

    #[test]
    fn atomic_write_no_half_file_and_never_gitignore() {
        let (roots, base) = temp_roots("atomic");
        let dest = roots.home.join(".cursor/mcp.json");
        let payload = br#"{"mcpServers":{"agent-lounge-os":{"command":"x"}}}"#;
        write_atomic_with_backup(&dest, payload, None).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), payload);
        let leftovers: Vec<_> = fs::read_dir(dest.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("lounge-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp leftovers: {leftovers:?}");
        assert!(!path_is_gitignore(&dest));
        assert!(path_is_gitignore(Path::new("/tmp/.gitignore")));
        let _ = roots;
        let _ = base;
    }

    #[test]
    fn windows_path_escaped_via_serde() {
        let cmd = PathBuf::from(r"C:\Users\Demo\bin\lounge-mcp.exe");
        let entry = lounge_server_entry(&cmd);
        let s = serde_json::to_string(&entry).unwrap();
        // serde escapes backslashes; raw single-backslash must not appear unescaped.
        assert!(s.contains(r"C:\\Users\\Demo\\bin\\lounge-mcp.exe") || s.contains("C:/Users"));
        let v: Value = serde_json::from_str(&s).unwrap();
        let round = v["command"].as_str().unwrap();
        assert!(command_is_exact_lounge_mcp(round) || round.ends_with("lounge-mcp.exe"));
    }

    #[test]
    fn create_global_when_missing_project_errors_without_dir() {
        let (roots, base) = temp_roots("create");
        let bin = base.join("bin/lounge-mcp");
        let sha = fake_binary(&bin, "0.1.0");
        let o = opts("cursor", &roots, &bin, &sha);
        apply_agent_connect(&roots, &o, ConnectAction::Connect).unwrap();
        assert!(roots.home.join(".cursor/mcp.json").is_file());

        let mut proj = o.clone();
        proj.scope = ConnectScope::Project;
        // workspace .cursor may not exist
        let err = apply_agent_connect(&roots, &proj, ConnectAction::Connect).unwrap_err();
        assert!(
            err.contains("missing") || err.contains("directory") || err.contains("create"),
            "{err}"
        );
    }

    #[test]
    fn claude_desktop_and_grok_equal_connected_without_rules() {
        let (roots, base) = temp_roots("norule");
        let bin = base.join("bin/lounge-mcp");
        let sha = fake_binary(&bin, "0.1.0");
        for client in ["claude_desktop", "grok_bot"] {
            let o = opts(client, &roots, &bin, &sha);
            let plan = plan_agent_connect(&roots, &o, ConnectAction::Connect).unwrap();
            assert!(
                plan.files.iter().all(|f| f.kind != "rule_file"),
                "{client} must not write rule files"
            );
            assert!(plan.warnings.iter().any(|w| w.contains("no rule-file")));
        }
    }
}
