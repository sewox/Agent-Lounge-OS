//! Detect upstream codebase-memory-mcp v0.11.0+ shared coordination daemon.
//!
//! ## Chosen detection signal
//! Invoke the sidecar binary with `daemon status` (official CLI in v0.11.0) and
//! classify into three **fail-closed** states:
//!
//! | Presence | When |
//! |---|---|
//! | **Active** | Clear `daemon: active …` line (any case) |
//! | **Inactive** | Clear `daemon: not running`, **or** binary missing, **or** pre-0.11 unknown-subcommand |
//! | **Unknown** | Timeout, wait/spawn error, empty/garbage output, `not responding`, non-zero exit without a clear Inactive line |
//!
//! Only **Inactive** may take the per-session `--ui=true --port=N` spawn path.
//! **Active** and **Unknown** never spawn and never mutate shared config/port.
//!
//! ### Pre-0.11 `daemon status` behaviour
//! Builds before the coordination daemon do not implement `daemon status`. Lounge
//! treats that as **Inactive** (safe to use the legacy per-session path) **only**
//! when **all** of the following hold:
//!
//! 1. the process exits **non-zero**,
//! 2. the error text **explicitly** says the `daemon` subcommand is
//!    unknown/unrecognized (same line), and
//! 3. **no** output line starts with `daemon` (any case).
//!
//! Loose matches on bare `usage:`, or on the words `daemon` + `codebase-memory-mcp`
//! alone, are **not** enough — those classify as **Unknown**. A hung or opaque
//! failure is also **Unknown** (fail-closed — do not spawn).
//!
//! Alternatives considered and rejected for Lounge's gate:
//! - HTTP `/api/ui-config` alone — does not prove daemon ownership vs per-session listener
//! - Process / socket inspection — OS-specific and races with unrelated listeners
//! - Env flags — no public env means "daemon is serving UI"
//! - Separate `CBM_CACHE_DIR` for Lounge's child — out of scope (OPEN); also rejected
//!   upstream for concurrent sessions sharing one canonical cache root

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::kernel::GuardedCommand;

const DAEMON_STATUS_TIMEOUT: Duration = Duration::from_secs(3);
/// Cap stdout/stderr so a noisy child cannot hang the reader indefinitely.
const DAEMON_STATUS_MAX_CAPTURE: usize = 64 * 1024;
/// After kill/timeout, do not wait forever for reader threads (a grandchild may
/// still hold the inherited pipes).
const DAEMON_STATUS_READER_JOIN: Duration = Duration::from_millis(500);

/// Fail-closed presence of the shared coordination daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CbmDaemonPresence {
    /// Clear `daemon: active …`.
    Active,
    /// Clear `daemon: not running`, missing binary, or pre-0.11 unknown subcommand.
    #[default]
    Inactive,
    /// Probe inconclusive — must never spawn.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CbmDaemonStatus {
    pub presence: CbmDaemonPresence,
    pub permanent: bool,
    pub daemon_pid: Option<u32>,
    /// Port from `ui: configured at http://127.0.0.1|localhost|[::1]:N`.
    pub ui_port: Option<u16>,
    /// Explicit `ui: disabled` line from `daemon status`.
    pub ui_disabled: bool,
    /// Raw stdout (truncated) for diagnostics — never contains secrets.
    pub raw_summary: String,
    /// Why presence is Unknown / Inactive (stable short token for tests/logs).
    #[serde(default)]
    pub reason: Option<String>,
}

impl Default for CbmDaemonStatus {
    fn default() -> Self {
        Self::inactive()
    }
}

impl CbmDaemonStatus {
    pub fn inactive() -> Self {
        Self {
            presence: CbmDaemonPresence::Inactive,
            permanent: false,
            daemon_pid: None,
            ui_port: None,
            ui_disabled: false,
            raw_summary: "daemon: not running".into(),
            reason: Some("not_running".into()),
        }
    }

    pub fn inactive_missing_binary() -> Self {
        Self {
            presence: CbmDaemonPresence::Inactive,
            permanent: false,
            daemon_pid: None,
            ui_port: None,
            ui_disabled: false,
            raw_summary: String::new(),
            reason: Some("missing_binary".into()),
        }
    }

    pub fn inactive_pre_0_11() -> Self {
        Self {
            presence: CbmDaemonPresence::Inactive,
            permanent: false,
            daemon_pid: None,
            ui_port: None,
            ui_disabled: false,
            raw_summary: String::new(),
            reason: Some("pre_0_11_unknown_subcommand".into()),
        }
    }

    pub fn unknown(reason: impl Into<String>, raw: impl Into<String>) -> Self {
        Self {
            presence: CbmDaemonPresence::Unknown,
            permanent: false,
            daemon_pid: None,
            ui_port: None,
            ui_disabled: false,
            raw_summary: raw.into(),
            reason: Some(reason.into()),
        }
    }

    #[cfg(any(test, feature = "test-helpers"))]
    pub fn active_for_tests(ui_port: Option<u16>, permanent: bool) -> Self {
        Self {
            presence: CbmDaemonPresence::Active,
            permanent,
            daemon_pid: Some(1),
            ui_port,
            ui_disabled: ui_port.is_none(),
            raw_summary: "daemon: active (test)".into(),
            reason: None,
        }
    }

    /// True when Lounge must not spawn / mutate shared UI config.
    pub fn blocks_spawn(&self) -> bool {
        matches!(
            self.presence,
            CbmDaemonPresence::Active | CbmDaemonPresence::Unknown
        )
    }

    pub fn is_active(&self) -> bool {
        self.presence == CbmDaemonPresence::Active
    }

    pub fn is_inactive(&self) -> bool {
        self.presence == CbmDaemonPresence::Inactive
    }

    pub fn is_unknown(&self) -> bool {
        self.presence == CbmDaemonPresence::Unknown
    }

    /// Legacy alias — Active only (not Unknown). Prefer [`Self::blocks_spawn`].
    pub fn is_daemon_mode(&self) -> bool {
        self.is_active()
    }
}

/// Strip a UTF-8 BOM if present.
pub fn strip_utf8_bom(s: &str) -> &str {
    s.strip_prefix('\u{FEFF}').unwrap_or(s)
}

fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

fn extract_loopback_ui_port(line_lower: &str) -> Option<u16> {
    const MARKERS: &[&str] = &[
        "http://127.0.0.1:",
        "http://localhost:",
        "http://[::1]:",
        "https://127.0.0.1:",
        "https://localhost:",
        "https://[::1]:",
    ];
    for marker in MARKERS {
        if let Some(idx) = line_lower.find(marker) {
            let after = &line_lower[idx + marker.len()..];
            let port_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(port) = port_str.parse::<u16>() {
                if port > 0 {
                    return Some(port);
                }
            }
        }
    }
    None
}

/// Pre-0.11 / old-binary: Inactive only when exit ≠ 0, an error line explicitly
/// names `daemon` as an unknown/unrecognized command/subcommand, and no output
/// line starts with `daemon`.
fn looks_like_pre_0_11_unknown_subcommand(
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> bool {
    if !matches!(exit_code, Some(code) if code != 0) {
        return false;
    }
    let combined = format!("{stdout}\n{stderr}");
    for line in combined.lines() {
        if line.trim().to_ascii_lowercase().starts_with("daemon") {
            return false;
        }
    }
    explicitly_unknown_daemon_subcommand(&combined.to_ascii_lowercase())
}

fn explicitly_unknown_daemon_subcommand(combined_lower: &str) -> bool {
    for line in combined_lower.lines() {
        let line = line.trim();
        if !line.contains("daemon") {
            continue;
        }
        let names_unknown = line.contains("unknown command")
            || line.contains("unrecognized command")
            || line.contains("unknown subcommand")
            || line.contains("unrecognized subcommand")
            || line.contains("invalid command")
            || line.contains("unexpected argument");
        if names_unknown {
            return true;
        }
    }
    false
}

/// Parse `codebase-memory-mcp daemon status` stdout/stderr (v0.11.0 contract).
///
/// Case-insensitive. Accepts CRLF and a leading UTF-8 BOM. Host forms for the UI
/// URL: `127.0.0.1`, `localhost`, `[::1]`.
pub fn parse_daemon_status_output(stdout: &str) -> CbmDaemonStatus {
    classify_daemon_probe_output(stdout, "", Some(0))
}

/// Classify raw probe bytes + exit status into Active / Inactive / Unknown.
pub fn classify_daemon_probe_output(
    stdout: &str,
    stderr: &str,
    exit_code: Option<i32>,
) -> CbmDaemonStatus {
    let stdout = normalize_newlines(strip_utf8_bom(stdout));
    let stderr = normalize_newlines(strip_utf8_bom(stderr));
    let combined = format!("{stdout}\n{stderr}");
    let summary: String = stdout.lines().take(8).collect::<Vec<_>>().join("\n");

    let mut permanent = false;
    let mut daemon_pid = None;
    let mut ui_port = None;
    let mut ui_disabled = false;
    let mut saw_active = false;
    let mut saw_not_running = false;
    let mut saw_not_responding = false;

    for line in combined.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();

        if lower.starts_with("daemon: active") {
            saw_active = true;
            permanent = lower.contains("permanent");
            continue;
        }
        if lower.starts_with("daemon: not running") {
            saw_not_running = true;
            continue;
        }
        if lower.starts_with("daemon: not responding") {
            saw_not_responding = true;
            continue;
        }
        if let Some(rest) = lower.strip_prefix("pid:") {
            if let Ok(pid) = rest.trim().parse::<u32>() {
                daemon_pid = Some(pid);
            }
            continue;
        }
        // Also accept "  pid: N" after trim already handled.
        if lower.starts_with("ui: disabled") {
            ui_disabled = true;
            ui_port = None;
            continue;
        }
        if let Some(port) = extract_loopback_ui_port(&lower) {
            ui_port = Some(port);
            ui_disabled = false;
        }
    }

    if saw_not_responding {
        return CbmDaemonStatus::unknown("not_responding", summary);
    }
    if saw_active {
        return CbmDaemonStatus {
            presence: CbmDaemonPresence::Active,
            permanent,
            daemon_pid,
            ui_port,
            ui_disabled,
            raw_summary: summary,
            reason: None,
        };
    }
    if saw_not_running {
        return CbmDaemonStatus {
            presence: CbmDaemonPresence::Inactive,
            permanent: false,
            daemon_pid: None,
            ui_port: None,
            ui_disabled: false,
            raw_summary: summary,
            reason: Some("not_running".into()),
        };
    }
    if looks_like_pre_0_11_unknown_subcommand(&stdout, &stderr, exit_code) {
        let mut s = CbmDaemonStatus::inactive_pre_0_11();
        s.raw_summary = summary;
        return s;
    }

    // Non-zero exit without a clear Inactive line → Unknown (fail-closed).
    // Zero exit with empty/garbage → Unknown.
    let reason = match exit_code {
        Some(code) if code != 0 => "nonzero_exit",
        Some(0) if stdout.trim().is_empty() && stderr.trim().is_empty() => "empty_output",
        Some(0) => "unrecognised_output",
        None => "missing_exit",
        _ => "unrecognised_output",
    };
    CbmDaemonStatus::unknown(reason, summary)
}

/// Run `binary daemon status` with a short timeout. Never mutates config.
/// Safe to call from `spawn_blocking` — does not touch the async runtime.
pub fn probe_daemon_status(binary: &Path) -> Result<CbmDaemonStatus> {
    probe_daemon_status_with_override(binary, None)
}

/// Same as [`probe_daemon_status`], with an optional in-process override (tests).
pub fn probe_daemon_status_with_override(
    binary: &Path,
    override_status: Option<&CbmDaemonStatus>,
) -> Result<CbmDaemonStatus> {
    if let Some(overridden) = override_status {
        return Ok(overridden.clone());
    }

    if !binary.is_file() {
        return Ok(CbmDaemonStatus::inactive_missing_binary());
    }

    let gated = match GuardedCommand::new(binary)
        .arg("daemon")
        .arg("status")
        .internal_daemon()
        .into_std_command()
    {
        Ok(cmd) => cmd,
        Err(err) => {
            log::debug!("cbm daemon status gate failed: {err}");
            return Ok(CbmDaemonStatus::unknown(
                "gate_error",
                format!("gate: {err}"),
            ));
        }
    };

    let mut command: Command = gated;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }

    let child = match command.spawn() {
        Ok(c) => c,
        Err(err) => {
            log::debug!("cbm daemon status spawn failed: {err}");
            // Binary path exists but failed to start → Unknown (not "missing").
            return Ok(CbmDaemonStatus::unknown(
                "spawn_failed",
                format!("spawn: {err}"),
            ));
        }
    };

    match wait_child_bounded(child, DAEMON_STATUS_TIMEOUT, DAEMON_STATUS_MAX_CAPTURE) {
        Ok(out) => Ok(classify_daemon_probe_output(
            &out.stdout,
            &out.stderr,
            out.exit_code,
        )),
        Err(err) => {
            let msg = err.to_string();
            log::debug!("cbm daemon status probe failed: {msg}");
            if msg.contains("timed out") {
                Ok(CbmDaemonStatus::unknown("timeout", msg))
            } else {
                Ok(CbmDaemonStatus::unknown("wait_error", msg))
            }
        }
    }
}

struct BoundedOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
}

fn read_bounded(mut reader: impl Read, max: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if buf.len() >= max {
            break;
        }
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                let room = max.saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..n.min(room)]);
            }
            Err(_) => break,
        }
    }
    // Drain leftover so the child is not stuck on a full pipe, but do not retain it.
    let mut sink = [0u8; 8192];
    loop {
        match reader.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
    buf
}

/// Join a reader thread with a hard deadline. On timeout the join handle is
/// abandoned (thread may outlive the probe) so a grandchild holding pipes cannot
/// pin the single-flight lock forever.
fn join_bounded<T: Send + 'static>(
    handle: std::thread::JoinHandle<T>,
    limit: Duration,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(handle.join());
    });
    match rx.recv_timeout(limit) {
        Ok(Ok(value)) => Some(value),
        _ => None,
    }
}

fn wait_child_bounded(
    mut child: std::process::Child,
    timeout: Duration,
    max_capture: usize,
) -> Result<BoundedOutput> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_handle = std::thread::spawn(move || {
        stdout
            .map(|r| read_bounded(r, max_capture))
            .unwrap_or_default()
    });
    let stderr_handle = std::thread::spawn(move || {
        stderr
            .map(|r| read_bounded(r, max_capture))
            .unwrap_or_default()
    });

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                // Bounded joins: grandchild holding inherited pipes must not hang us.
                let _ = join_bounded(stdout_handle, DAEMON_STATUS_READER_JOIN);
                let _ = join_bounded(stderr_handle, DAEMON_STATUS_READER_JOIN);
                anyhow::bail!("daemon status timed out after {timeout:?}");
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(err) => {
                let _ = child.kill();
                let _ = join_bounded(stdout_handle, DAEMON_STATUS_READER_JOIN);
                let _ = join_bounded(stderr_handle, DAEMON_STATUS_READER_JOIN);
                return Err(err).context("daemon status try_wait");
            }
        }
    };

    // Child exited: pipes should EOF quickly. Still bound the join so a leaked
    // grandchild cannot pin the probe (fail-closed → empty/partial → Unknown).
    let join_limit = DAEMON_STATUS_READER_JOIN
        .checked_mul(4)
        .unwrap_or(DAEMON_STATUS_READER_JOIN);
    let stdout_bytes = join_bounded(stdout_handle, join_limit).unwrap_or_default();
    let stderr_bytes = join_bounded(stderr_handle, join_limit).unwrap_or_default();
    Ok(BoundedOutput {
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        exit_code: status.code(),
    })
}

/// Resolve config path under an optional cache dir (tests inject; prod uses env/HOME).
pub fn cbm_config_path_in(cache_dir: &Path) -> PathBuf {
    cache_dir.join("config.json")
}

/// Sibling cache the fake helper pollutes on `--ui` spawn (no process-wide env).
#[cfg(any(test, feature = "test-helpers"))]
pub fn fake_helper_pollution_config(binary: &Path) -> Option<PathBuf> {
    Some(
        binary
            .parent()?
            .join("lounge-fake-cbm-cache")
            .join("config.json"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_not_running_case_insensitive_crlf_bom() {
        let raw = "\u{FEFF}Daemon: Not Running\r\nhint: something\r\n";
        let s = parse_daemon_status_output(raw);
        assert!(s.is_inactive());
        assert!(!s.blocks_spawn());
        assert_eq!(s.reason.as_deref(), Some("not_running"));
    }

    #[test]
    fn parse_active_permanent_with_localhost_port() {
        let raw = "\
DAEMON: ACTIVE (permanent)
  PID: 3525
  ui: configured at http://localhost:9749 (readiness not checked)
";
        let s = parse_daemon_status_output(raw);
        assert!(s.is_active());
        assert!(s.permanent);
        assert_eq!(s.daemon_pid, Some(3525));
        assert_eq!(s.ui_port, Some(9749));
        assert!(s.blocks_spawn());
    }

    #[test]
    fn parse_active_ipv6_loopback_port() {
        let raw = "daemon: active (session-managed)\n  ui: configured at http://[::1]:18749\n";
        let s = parse_daemon_status_output(raw);
        assert!(s.is_active());
        assert_eq!(s.ui_port, Some(18749));
        assert!(!s.permanent);
    }

    #[test]
    fn parse_active_session_managed_ui_disabled() {
        let raw = "\
daemon: active (session-managed)
  pid: 99
  ui: disabled (enable with `daemon start` in a UI build)
";
        let s = parse_daemon_status_output(raw);
        assert!(s.is_active());
        assert!(s.ui_disabled);
        assert!(s.ui_port.is_none());
    }

    #[test]
    fn parse_not_responding_is_unknown() {
        let raw = "daemon: not responding (endpoint held by pid 12)\n";
        let s = parse_daemon_status_output(raw);
        assert!(s.is_unknown());
        assert!(s.blocks_spawn());
        assert_eq!(s.reason.as_deref(), Some("not_responding"));
    }

    #[test]
    fn classify_garbage_nonzero_is_unknown() {
        let s = classify_daemon_probe_output("totally unrelated\n", "", Some(2));
        assert!(s.is_unknown());
        assert_eq!(s.reason.as_deref(), Some("nonzero_exit"));
    }

    #[test]
    fn classify_empty_zero_is_unknown() {
        let s = classify_daemon_probe_output("", "", Some(0));
        assert!(s.is_unknown());
        assert_eq!(s.reason.as_deref(), Some("empty_output"));
    }

    #[test]
    fn classify_genuine_old_binary_unknown_subcommand_is_inactive() {
        let help = "\
codebase-memory-mcp 0.10.8

Usage:
  codebase-memory-mcp              Run MCP server on stdio
  codebase-memory-mcp --ui=true    Enable HTTP graph visualization
";
        let s = classify_daemon_probe_output(help, "unknown command: daemon\n", Some(1));
        assert!(s.is_inactive());
        assert_eq!(s.reason.as_deref(), Some("pre_0_11_unknown_subcommand"));
        assert!(!s.blocks_spawn());

        let clap = classify_daemon_probe_output(
            "",
            "error: unrecognized subcommand 'daemon'\n\nUsage: codebase-memory-mcp [OPTIONS]\n",
            Some(2),
        );
        assert!(clap.is_inactive());
        assert_eq!(clap.reason.as_deref(), Some("pre_0_11_unknown_subcommand"));
    }

    #[test]
    fn classify_reworded_active_with_daemon_start_hint_is_unknown() {
        // Must NOT match the old loose "daemon" + "codebase-memory-mcp" heuristic.
        let stdout = "\
shared coordination service is up (permanent)
  pid: 12
hint: `codebase-memory-mcp daemon start` keeps a daemon warm so CLI commands skip startup.
";
        let s = classify_daemon_probe_output(stdout, "", Some(0));
        assert!(s.is_unknown(), "reworded active must be Unknown, got {s:?}");
        assert!(s.blocks_spawn());
    }

    #[test]
    fn classify_stray_unrecognized_on_stderr_exit_zero_is_unknown() {
        let s = classify_daemon_probe_output(
            "ready\n",
            "warning: unrecognized locale setting\n",
            Some(0),
        );
        assert!(s.is_unknown());
        assert_ne!(s.reason.as_deref(), Some("pre_0_11_unknown_subcommand"));
    }

    #[test]
    fn classify_bare_usage_without_daemon_unknown_is_unknown() {
        let s =
            classify_daemon_probe_output("Usage:\n  codebase-memory-mcp --ui=true\n", "", Some(1));
        assert!(s.is_unknown());
        assert_eq!(s.reason.as_deref(), Some("nonzero_exit"));
    }

    #[test]
    fn classify_not_running_nonzero_exit_is_inactive() {
        // Upstream returns exit 1 with "daemon: not running" plus a daemon-start hint.
        let stdout = "\
daemon: not running
hint: `codebase-memory-mcp daemon start` keeps a daemon warm so CLI commands and hooks skip the per-command startup cost.
";
        let s = classify_daemon_probe_output(stdout, "", Some(1));
        assert!(s.is_inactive());
        assert_eq!(s.reason.as_deref(), Some("not_running"));
    }

    #[test]
    fn missing_binary_is_inactive() {
        let got = probe_daemon_status(Path::new("/nonexistent/codebase-memory-mcp")).unwrap();
        assert!(got.is_inactive());
        assert_eq!(got.reason.as_deref(), Some("missing_binary"));
    }

    #[test]
    fn override_short_circuits_probe() {
        let overridden = CbmDaemonStatus::active_for_tests(Some(9749), true);
        let got = probe_daemon_status_with_override(
            Path::new("/nonexistent/codebase-memory-mcp"),
            Some(&overridden),
        )
        .unwrap();
        assert!(got.is_active());
        assert_eq!(got.ui_port, Some(9749));
    }

    #[test]
    fn strip_bom_helper() {
        assert_eq!(strip_utf8_bom("\u{FEFF}hi"), "hi");
        assert_eq!(strip_utf8_bom("hi"), "hi");
    }
}
