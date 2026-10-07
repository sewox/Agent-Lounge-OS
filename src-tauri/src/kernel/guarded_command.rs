//! Single entry point for process spawning — all agent/user-initiated commands go through PolicyGate.

use std::ffi::{OsStr, OsString};
use std::process::{Command, Output, Stdio};

use anyhow::{bail, Result};

use super::destructive_confirm::{command_hash, register_pending, take_confirmed_allowance};
use super::policy_gate::{
    classify_argv, classify_destructive, ActionSource, DestructiveClass, PolicyDecision, PolicyGate,
};

/// Programs allowed to use [`GuardedCommand::internal_daemon`] (F9).
const INTERNAL_DAEMON_ALLOWLIST: &[&str] = &[
    "nats-server",
    "codebase-memory-mcp",
    "ollama",
    "lmr",
    "lsof",
    "ss",
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
        let base = raw
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&raw)
            .to_ascii_lowercase();
        base.strip_suffix(".exe")
            .or_else(|| base.strip_suffix(".cmd"))
            .or_else(|| base.strip_suffix(".bat"))
            .or_else(|| base.strip_suffix(".com"))
            .map(str::to_string)
            .unwrap_or(base)
    }

    fn argv_strings(&self) -> Vec<String> {
        self.args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    fn ensure_daemon_allowlist(&self) -> Result<()> {
        if !self.bypass_gate {
            return Ok(());
        }
        let base = self.program_base();
        // Exact match after stripping Windows executable suffixes (.exe/.cmd/.bat/.com);
        // target-triple suffix (`name-triple`) still ok.
        // Must NOT prefix-match bare names (`kill` ↛ `killall`) — F18.
        let allowed = INTERNAL_DAEMON_ALLOWLIST
            .iter()
            .any(|p| base == *p || base.starts_with(&format!("{p}-")));
        if !allowed {
            bail!("internal_daemon not allowlisted for program: {base}");
        }
        // kill/taskkill: pid-only argv
        if base == "kill" || base == "taskkill" {
            let args = self.argv_strings();
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
        let prog = self.program.to_string_lossy().into_owned();
        let args = self.argv_strings();
        // F22: classify the real argv — never re-join then re-split (loses -c quoting).
        let decision = PolicyGate::evaluate_argv(&prog, &args, self.source)?;
        if let PolicyDecision::RequireConfirmation { class, .. } = &decision {
            let hash = command_hash(&prog, &args);
            if take_confirmed_allowance(&hash) {
                return Ok(PolicyDecision::Allow);
            }
            let event = register_pending(&prog, &args, *class, self.source);
            bail!(
                "destructive operation requires user confirmation ({:?}): {} confirm_id={}",
                class,
                self.command_line(),
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

/// Classify structured argv without executing (F22).
pub fn would_require_confirmation_argv(
    program: &str,
    args: &[String],
    source: ActionSource,
) -> Option<DestructiveClass> {
    match classify_argv(program, args) {
        Some(class) => {
            let _ = source;
            Some(class)
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::destructive_confirm::{
        confirm_destructive, reset_for_tests, take_emitted_for_tests, test_lock,
    };

    fn expect_confirm_id(result: Result<Command>) -> String {
        let err = result.expect_err("expected confirmation");
        let msg = format!("{err}");
        assert!(
            msg.contains("confirm_id="),
            "expected confirm_id= in error, got: {msg}"
        );
        msg.split("confirm_id=").nth(1).unwrap().trim().to_string()
    }

    #[test]
    fn internal_daemon_rejects_rm() {
        let _guard = test_lock();
        let err = GuardedCommand::new("rm")
            .args(["-rf", "/tmp/x"])
            .internal_daemon()
            .into_std_command();
        assert!(err.is_err());
        assert!(format!("{}", err.unwrap_err()).contains("allowlisted"));
    }

    #[test]
    fn internal_daemon_rejects_killall_prefix() {
        let _guard = test_lock();
        let err = GuardedCommand::new("killall")
            .args(["nats-server"])
            .internal_daemon()
            .into_std_command();
        assert!(err.is_err(), "kill must not admit killall");
        assert!(format!("{}", err.unwrap_err()).contains("allowlisted"));

        let err = GuardedCommand::new("kill.exe")
            .args(["1"])
            .internal_daemon()
            .into_std_command();
        assert!(err.is_ok(), "kill.exe should strip .exe and exact-match");
    }

    /// F22: Windows launcher suffixes must strip for allowlist AND must not
    /// let a non-allowlisted destructive program sneak past as an "internal_daemon".
    #[test]
    fn program_base_strips_windows_launcher_suffixes_case_insensitive() {
        let _guard = test_lock();
        let allow_cases = [
            "ollama.exe",
            "OLLAMA.EXE",
            "ollama.cmd",
            "Ollama.CMD",
            "ollama.bat",
            "OLLAMA.BAT",
            "ollama.com",
            "Ollama.Com",
            r"C:\Program Files\Lounge\nats-server.CMD",
            "/usr/local/bin/nats-server.bat",
        ];
        for prog in allow_cases {
            let result = GuardedCommand::new(prog)
                .internal_daemon()
                .into_std_command();
            assert!(
                result.is_ok(),
                "allowlisted stem must pass after suffix strip: {prog} → {}",
                result.err().map(|e| e.to_string()).unwrap_or_default()
            );
        }

        // Non-allowlisted programs with the same suffixes stay blocked from bypass.
        let deny_cases = [
            "rm.cmd",
            "RM.BAT",
            "killall.com",
            "KillAll.EXE",
            r"C:\Windows\System32\cmd.exe",
        ];
        for prog in deny_cases {
            let result = GuardedCommand::new(prog)
                .args(["-rf", "/tmp/x"])
                .internal_daemon()
                .into_std_command();
            assert!(
                result.is_err(),
                "non-allowlisted {prog} must not bypass via launcher suffix"
            );
            assert!(
                format!("{}", result.unwrap_err()).contains("allowlisted"),
                "{prog}"
            );
        }
    }

    #[test]
    fn windows_suffix_does_not_bypass_destructive_confirmation() {
        let _guard = test_lock();
        reset_for_tests();
        // Without internal_daemon, destructive argv still needs confirmation even
        // when the program path uses .cmd/.bat/.com (suffix strip is for allowlist only).
        for prog in ["rm.cmd", "RM.BAT", "rm.COM", "rm.exe"] {
            reset_for_tests();
            let result = GuardedCommand::new(prog)
                .args(["-rf", "/tmp/suffix-bypass"])
                .source(ActionSource::Agent)
                .into_std_command();
            let _ = expect_confirm_id(result);
        }
    }

    #[test]
    fn confirm_destructive_allows_once() {
        let _guard = test_lock();
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

    #[test]
    fn confirm_destructive_allows_arg_with_space() {
        let _guard = test_lock();
        reset_for_tests();
        let err = GuardedCommand::new("rm")
            .args(["-rf", "/tmp/has space"])
            .source(ActionSource::User)
            .into_std_command();
        let msg = format!("{}", err.unwrap_err());
        let id = msg.split("confirm_id=").nth(1).unwrap().trim().to_string();
        confirm_destructive(&id).unwrap();
        let cmd = GuardedCommand::new("rm")
            .args(["-rf", "/tmp/has space"])
            .source(ActionSource::User)
            .into_std_command();
        assert!(
            cmd.is_ok(),
            "register_pending and evaluate must hash the same argv"
        );
    }

    /// F22: argv-preserving classification through GuardedCommand.
    #[test]
    fn f22_shell_c_payload_requires_confirmation() {
        let _guard = test_lock();
        reset_for_tests();
        let cases: Vec<(&str, &[&str])> = vec![
            ("sh", &["-c", "rm -rf x"]),
            ("bash", &["-lc", "git clean -fdx"]),
            ("sudo", &["sh", "-c", "rm -rf x"]),
            ("cmd", &["/c", "del /s /q x"]),
            ("pwsh", &["-Command", "Remove-Item -Recurse x"]),
        ];
        for (prog, args) in cases {
            reset_for_tests();
            let result = GuardedCommand::new(prog)
                .args(args.iter().copied())
                .source(ActionSource::Agent)
                .into_std_command();
            let id = expect_confirm_id(result);
            assert!(!id.is_empty(), "{prog} {:?}", args);
        }
    }

    #[test]
    fn f22_shell_c_echo_allowed() {
        let _guard = test_lock();
        reset_for_tests();
        let result = GuardedCommand::new("sh")
            .args(["-c", "echo hi"])
            .source(ActionSource::User)
            .into_std_command();
        assert!(
            result.is_ok(),
            "non-destructive -c payload must be Allowed: {}",
            result.err().map(|e| e.to_string()).unwrap_or_default()
        );
    }

    #[test]
    fn f22_k6_git_patterns_via_argv() {
        let _guard = test_lock();
        let cases: Vec<(&str, &[&str])> = vec![
            ("git", &["reset", "--hard"]),
            ("git", &["push", "--force"]),
            ("git", &["push", "-f"]),
            ("git", &["push", "--force-with-lease"]),
            ("git", &["clean", "-fd"]),
            ("git", &["clean", "-fdx"]),
            ("git", &["clean", "-df"]),
            ("git", &["-C", "/tmp/repo", "reset", "--hard"]),
            ("git", &["-c", "user.name=x", "push", "--force"]),
            ("git", &["-C/tmp/repo", "clean", "-fd"]),
            ("git", &["-cuser.name=x", "push", "-f"]),
        ];
        for (prog, args) in cases {
            reset_for_tests();
            let result = GuardedCommand::new(prog)
                .args(args.iter().copied())
                .source(ActionSource::User)
                .into_std_command();
            let _ = expect_confirm_id(result);
        }
    }

    #[test]
    fn f22_windows_style_argv_on_all_oses() {
        let _guard = test_lock();
        let cases: Vec<(&str, &[&str])> = vec![
            ("cmd", &["/c", "del /s /q C:\\tmp\\x"]),
            ("cmd", &["/c", "rd /s /q build"]),
            (
                "powershell",
                &["-Command", "Remove-Item -Recurse -Force .\\db"],
            ),
            ("pwsh", &["-Command", "Remove-Item -Recurse x"]),
            ("powershell.exe", &["-Command", "ri -Recurse x"]),
        ];
        for (prog, args) in cases {
            reset_for_tests();
            let result = GuardedCommand::new(prog)
                .args(args.iter().copied())
                .source(ActionSource::Agent)
                .into_std_command();
            let _ = expect_confirm_id(result);
        }
    }
}
