//! Detect upstream codebase-memory-mcp v0.11.0+ shared coordination daemon.
//!
//! ## Chosen detection signal
//! Invoke the sidecar binary with `daemon status` (official CLI in v0.11.0) and
//! parse its stable stdout:
//! - `daemon: active (permanent|session-managed)` → shared daemon mode
//! - `ui: configured at http://127.0.0.1:N` → shared UI port (do not rewrite)
//! - `ui: disabled ...` → daemon up, UI off
//! - `daemon: not running` / probe failure / unrecognized output → per-session mode
//!
//! Alternatives considered and rejected for Lounge's gate:
//! - HTTP `/api/ui-config` alone — does not prove daemon ownership vs per-session listener
//! - Process / socket inspection — OS-specific and races with unrelated listeners
//! - Env flags — no public env means "daemon is serving UI"
//! - Separate `CBM_CACHE_DIR` for Lounge's child — out of scope (OPEN); also rejected
//!   upstream for concurrent sessions sharing one canonical cache root

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::kernel::GuardedCommand;

const DAEMON_STATUS_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CbmDaemonStatus {
    /// Shared coordination daemon is active (permanent or session-managed).
    pub active: bool,
    pub permanent: bool,
    pub daemon_pid: Option<u32>,
    /// Port from `ui: configured at http://127.0.0.1:N`, when present.
    pub ui_port: Option<u16>,
    /// Explicit `ui: disabled` line from `daemon status`.
    pub ui_disabled: bool,
    /// Raw stdout (truncated) for diagnostics — never contains secrets.
    pub raw_summary: String,
}

impl CbmDaemonStatus {
    pub fn inactive() -> Self {
        Self {
            active: false,
            permanent: false,
            daemon_pid: None,
            ui_port: None,
            ui_disabled: false,
            raw_summary: "daemon: not running".into(),
        }
    }

    pub fn is_daemon_mode(&self) -> bool {
        self.active
    }
}

/// Parse `codebase-memory-mcp daemon status` stdout (v0.11.0 contract).
pub fn parse_daemon_status_output(stdout: &str) -> CbmDaemonStatus {
    let mut status = CbmDaemonStatus::inactive();
    status.raw_summary = stdout.lines().take(8).collect::<Vec<_>>().join("\n");

    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("daemon: active") {
            status.active = true;
            let lower = rest.to_ascii_lowercase();
            status.permanent = lower.contains("permanent");
            continue;
        }
        if trimmed.starts_with("daemon: not running")
            || trimmed.starts_with("daemon: not responding")
        {
            status.active = false;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("pid:") {
            if let Ok(pid) = rest.trim().parse::<u32>() {
                status.daemon_pid = Some(pid);
            }
            continue;
        }
        if trimmed.starts_with("ui: disabled") {
            status.ui_disabled = true;
            status.ui_port = None;
            continue;
        }
        if let Some(idx) = trimmed.find("http://127.0.0.1:") {
            let after = &trimmed[idx + "http://127.0.0.1:".len()..];
            let port_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
            if let Ok(port) = port_str.parse::<u16>() {
                if port > 0 {
                    status.ui_port = Some(port);
                    status.ui_disabled = false;
                }
            }
        }
    }
    status
}

/// Run `binary daemon status` with a short timeout. Never mutates config.
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
        return Ok(CbmDaemonStatus::inactive());
    }

    let output = GuardedCommand::new(binary)
        .arg("daemon")
        .arg("status")
        .internal_daemon()
        .into_std_command()
        .with_context(|| format!("daemon status gate: {}", binary.display()))?;

    let mut command = output;
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
            return Ok(CbmDaemonStatus::inactive());
        }
    };

    let wait = wait_child_with_timeout(child, DAEMON_STATUS_TIMEOUT);
    match wait {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let parsed = parse_daemon_status_output(&stdout);
            // Exit code is non-zero when not running; still parse stdout.
            Ok(parsed)
        }
        Err(err) => {
            log::debug!("cbm daemon status probe failed: {err}");
            Ok(CbmDaemonStatus::inactive())
        }
    }
}

fn wait_child_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<std::process::Output> {
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .context("daemon status wait_with_output");
            }
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("daemon status timed out after {timeout:?}");
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(err) => {
                let _ = child.kill();
                return Err(err).context("daemon status try_wait");
            }
        }
    }
}

/// Resolve config path under an optional cache dir (tests inject; prod uses env/HOME).
pub fn cbm_config_path_in(cache_dir: &Path) -> PathBuf {
    cache_dir.join("config.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_not_running() {
        let s = parse_daemon_status_output(
            "daemon: not running\nhint: `codebase-memory-mcp daemon start` keeps a daemon warm\n",
        );
        assert!(!s.active);
        assert!(!s.is_daemon_mode());
        assert!(s.ui_port.is_none());
    }

    #[test]
    fn parse_active_permanent_with_ui_port() {
        let raw = "\
daemon: active (permanent)
  pid: 3525
  build: 0.11.0 (ce11c141431a...)
  committed clients: 0
  ui: configured at http://127.0.0.1:9749 (readiness not checked; `daemon start --open` verifies it)
";
        let s = parse_daemon_status_output(raw);
        assert!(s.active);
        assert!(s.permanent);
        assert_eq!(s.daemon_pid, Some(3525));
        assert_eq!(s.ui_port, Some(9749));
        assert!(!s.ui_disabled);
    }

    #[test]
    fn parse_active_session_managed_ui_disabled() {
        let raw = "\
daemon: active (session-managed)
  pid: 99
  build: 0.11.0 (abcd...)
  committed clients: 1
  ui: disabled (enable with `daemon start` in a UI build)
";
        let s = parse_daemon_status_output(raw);
        assert!(s.active);
        assert!(!s.permanent);
        assert_eq!(s.daemon_pid, Some(99));
        assert!(s.ui_disabled);
        assert!(s.ui_port.is_none());
    }

    #[test]
    fn parse_not_responding_is_inactive_for_lounge() {
        let raw = "daemon: not responding (endpoint held by pid 12)\n";
        let s = parse_daemon_status_output(raw);
        assert!(!s.active);
    }

    #[test]
    fn override_short_circuits_probe() {
        let overridden = CbmDaemonStatus {
            active: true,
            permanent: true,
            daemon_pid: Some(1),
            ui_port: Some(9749),
            ui_disabled: false,
            raw_summary: "override".into(),
        };
        let got = probe_daemon_status_with_override(
            Path::new("/nonexistent/codebase-memory-mcp"),
            Some(&overridden),
        )
        .unwrap();
        assert!(got.active);
        assert_eq!(got.ui_port, Some(9749));
        let inactive = probe_daemon_status(Path::new("/nonexistent/codebase-memory-mcp")).unwrap();
        assert!(!inactive.active);
    }
}
