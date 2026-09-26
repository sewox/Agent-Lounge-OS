//! Single entry point for process spawning — all agent/user-initiated commands go through PolicyGate.

use std::ffi::{OsStr, OsString};
use std::process::{Command, Output, Stdio};

use anyhow::{bail, Result};

use super::destructive_confirm::{command_hash, register_pending, take_confirmed_allowance};
use super::policy_gate::{
    classify_destructive, ActionSource, DestructiveClass, PolicyDecision, PolicyGate,
};

/// Programs allowed to use [`GuardedCommand::internal_daemon`] (F9).
const INTERNAL_DAEMON_ALLOWLIST: &[&str] = &[
    "nats-server",
    "codebase-memory-mcp",
    "ollama",
    "lmr",
    "lsof",
    "netstat",
    "kill",
    "taskkill",
];

pub struct GuardedCommand {
    program: OsString,
    args: Vec<OsString>,
    source: ActionSource,
    /// When true, skip gate (ONLY for justified internal daemon launches).
    bypass_gate: bool,
}

impl GuardedCommand {
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_os_string(),
            args: Vec::new(),
            source: ActionSource::System,
            bypass_gate: false,
        }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for a in args {
            self.args.push(a.as_ref().to_os_string());
        }
        self
    }

    pub fn source(mut self, source: ActionSource) -> Self {
        self.source = source;
        self
    }

    /// Justified bypass for internal daemon lifecycle (nats, memory bridge, ollama).
    /// `pub(crate)` + fixed program allowlist (F9).
    pub(crate) fn internal_daemon(mut self) -> Self {
        self.bypass_gate = true;
        self.source = ActionSource::System;
        self
    }

    pub fn command_line(&self) -> String {
        let mut parts = vec![self.program.to_string_lossy().into_owned()];
        for a in &self.args {
            parts.push(a.to_string_lossy().into_owned());
        }
        parts.join(" ")
    }

    fn program_base(&self) -> String {
        let raw = self.program.to_string_lossy();
        raw.rsplit(['/', '\\'])
            .next()
            .unwrap_or(&raw)
            .to_ascii_lowercase()
    }

    fn ensure_daemon_allowlist(&self) -> Result<()> {
        if !self.bypass_gate {
            return Ok(());
        }
        let base = self.program_base();
        let allowed = INTERNAL_DAEMON_ALLOWLIST
            .iter()
            .any(|p| base == *p || base.starts_with(&format!("{p}-")) || base.starts_with(p));
        if !allowed {
            bail!("internal_daemon not allowlisted for program: {base}");
        }
        // kill/taskkill: pid-only argv
        if base == "kill" || base == "taskkill" {
            let args: Vec<String> = self
                .args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            let ok = args.iter().all(|a| {
                a == "/PID"
                    || a == "/F"
                    || a == "/T"
                    || a == "-9"
                    || a == "-TERM"
                    || a.chars().all(|c| c.is_ascii_digit())
            });
            if !ok {
                bail!("internal_daemon kill/taskkill only allows pid flags");
            }
        }
        Ok(())
    }

    fn evaluate(&self) -> Result<PolicyDecision> {
        self.ensure_daemon_allowlist()?;
        if self.bypass_gate {
            return Ok(PolicyDecision::Allow);
        }
        let line = self.command_line();
        let decision = PolicyGate::evaluate(&line, self.source)?;
        if let PolicyDecision::RequireConfirmation { class, .. } = &decision {
            let hash = {
                let args: Vec<String> = self
                    .args
                    .iter()
                    .map(|a| a.to_string_lossy().into_owned())
                    .collect();
                command_hash(&self.program.to_string_lossy(), &args)
            };
            if take_confirmed_allowance(&hash) {
                return Ok(PolicyDecision::Allow);
            }
            let event = register_pending(&line, *class, self.source);
            bail!(
                "destructive operation requires user confirmation ({:?}): {} confirm_id={}",
                class,
                line,
                event.id
            );
        }
        Ok(decision)
    }

    pub fn output(self) -> Result<Output> {
        match self.evaluate()? {
            PolicyDecision::Allow => Ok(self.build().output()?),
            PolicyDecision::RequireConfirmation { class, .. } => {
                bail!(
                    "destructive operation requires user confirmation ({class:?}): {}",
                    self.command_line()
                )
            }
            PolicyDecision::Deny { reason } => bail!("command denied: {reason}"),
        }
    }

    pub fn spawn(self) -> Result<std::process::Child> {
        match self.evaluate()? {
            PolicyDecision::Allow => Ok(self.build().spawn()?),
            PolicyDecision::RequireConfirmation { class, .. } => {
                bail!(
                    "destructive operation requires user confirmation ({class:?}): {}",
                    self.command_line()
                )
            }
            PolicyDecision::Deny { reason } => bail!("command denied: {reason}"),
        }
    }

    pub fn status(self) -> Result<std::process::ExitStatus> {
        Ok(self.output()?.status)
    }

    /// Build underlying Command after gate passed. Prefer output/spawn.
    pub fn into_std_command(self) -> Result<Command> {
        match self.evaluate()? {
            PolicyDecision::Allow => Ok(self.build()),
            PolicyDecision::RequireConfirmation { class, .. } => {
                bail!(
                    "destructive operation requires user confirmation ({class:?}): {}",
                    self.command_line()
                )
            }
            PolicyDecision::Deny { reason } => bail!("command denied: {reason}"),
        }
    }

    fn build(&self) -> Command {
        // Intentionally uses Command::new — this is the ONLY allowed call site.
        #[allow(clippy::disallowed_methods)]
        let mut cmd = Command::new(&self.program);
        cmd.args(&self.args);
        cmd
    }

    pub fn stdout(self, cfg: Stdio) -> Result<Command> {
        let mut cmd = self.into_std_command()?;
        cmd.stdout(cfg);
        Ok(cmd)
    }
}

/// Classify without executing — for UI approval banners / tests.
pub fn would_require_confirmation(
    command_line: &str,
    source: ActionSource,
) -> Option<DestructiveClass> {
    match classify_destructive(command_line) {
        Some(class) => {
            let _ = source; // both agent and user always require confirm
            Some(class)
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::destructive_confirm::{
        confirm_destructive, reset_for_tests, take_emitted_for_tests,
    };

    #[test]
    fn internal_daemon_rejects_rm() {
        let err = GuardedCommand::new("rm")
            .args(["-rf", "/tmp/x"])
            .internal_daemon()
            .into_std_command();
        assert!(err.is_err());
        assert!(format!("{}", err.unwrap_err()).contains("allowlisted"));
    }

    #[test]
    fn confirm_destructive_allows_once() {
        reset_for_tests();
        let err = GuardedCommand::new("rm")
            .args(["-rf", "/tmp/pr1-confirm-test"])
            .source(ActionSource::User)
            .into_std_command();
        let msg = format!("{}", err.unwrap_err());
        assert!(msg.contains("confirm_id="));
        let id = msg.split("confirm_id=").nth(1).unwrap().trim().to_string();
        let emitted = take_emitted_for_tests();
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].kind, "destructive");

        confirm_destructive(&id).unwrap();
        // Second evaluate with same hash should Allow (consumes token). We only
        // build the Command — do not actually run rm -rf.
        let cmd = GuardedCommand::new("rm")
            .args(["-rf", "/tmp/pr1-confirm-test"])
            .source(ActionSource::User)
            .into_std_command();
        assert!(cmd.is_ok());

        // Replay without new confirm fails again.
        let again = GuardedCommand::new("rm")
            .args(["-rf", "/tmp/pr1-confirm-test"])
            .source(ActionSource::User)
            .into_std_command();
        assert!(again.is_err());
        assert!(confirm_destructive(&id).is_err());
    }
}
