//! Single entry point for process spawning — all agent/user-initiated commands go through PolicyGate.

use std::ffi::{OsStr, OsString};
use std::process::{Command, Output, Stdio};

use anyhow::{bail, Result};

use super::policy_gate::{
    classify_destructive, ActionSource, DestructiveClass, PolicyDecision, PolicyGate,
};

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
    /// Call sites must have `#[allow(clippy::disallowed_methods)]` removed — use this instead.
    pub fn internal_daemon(mut self) -> Self {
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

    fn evaluate(&self) -> Result<PolicyDecision> {
        if self.bypass_gate {
            return Ok(PolicyDecision::Allow);
        }
        let line = self.command_line();
        PolicyGate::evaluate(&line, self.source)
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
