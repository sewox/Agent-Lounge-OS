//! PolicyGate — destructive operations always require confirmation.
//! "Never Ask" / auto-approve cannot bypass this class (K6 / AP-06/07).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionSource {
    Agent,
    User,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DestructiveClass {
    PosixRm,
    PosixDd,
    PosixUnlink,
    PosixMkfs,
    WindowsDel,
    WindowsRd,
    WindowsRemoveItem,
    WindowsFormat,
    DbDrop,
    DbTruncate,
    DbDelete,
    DbMigrateDown,
    GitResetHard,
    GitPushForce,
    GitClean,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    RequireConfirmation {
        class: DestructiveClass,
        never_ask_bypasses: bool, // always false
    },
    Deny {
        reason: String,
    },
}

pub struct PolicyGate;

impl PolicyGate {
    /// Evaluate a shell/command line. Destructive patterns ALWAYS RequireConfirmation
    /// regardless of source (Agent or User) and Never Ask cannot bypass.
    pub fn evaluate(command_line: &str, _source: ActionSource) -> anyhow::Result<PolicyDecision> {
        if let Some(class) = classify_destructive(command_line) {
            return Ok(PolicyDecision::RequireConfirmation {
                class,
                never_ask_bypasses: false,
            });
        }
        Ok(PolicyDecision::Allow)
    }

    /// Whether Never Ask can skip this class — always false for destructive.
    pub fn never_ask_bypasses_destructive() -> bool {
        false
    }
}

pub fn classify_destructive(command: &str) -> Option<DestructiveClass> {
    let lower = command.to_ascii_lowercase();
    let collapsed: String = lower.split_whitespace().collect::<Vec<_>>().join(" ");

    // Git (K6) — check before generic patterns
    if matches_git_reset_hard(&collapsed) {
        return Some(DestructiveClass::GitResetHard);
    }
    if matches_git_push_force(&collapsed) {
        return Some(DestructiveClass::GitPushForce);
    }
    if matches_git_clean(&collapsed) {
        return Some(DestructiveClass::GitClean);
    }

    // POSIX
    if collapsed.contains("rm -rf") || collapsed.contains("rm -fr") || regex_rm_r(&collapsed) {
        return Some(DestructiveClass::PosixRm);
    }
    if collapsed.contains("dd if=") || collapsed.starts_with("dd ") || collapsed.contains(" dd ") {
        // only if looks like dd command
        if collapsed.split_whitespace().any(|t| t == "dd") {
            return Some(DestructiveClass::PosixDd);
        }
    }
    if collapsed.contains("unlink ") || collapsed.starts_with("unlink ") {
        return Some(DestructiveClass::PosixUnlink);
    }
    if collapsed.contains("mkfs") {
        return Some(DestructiveClass::PosixMkfs);
    }

    // Windows
    if collapsed.contains("del /s") || collapsed.contains("del /s /q") {
        return Some(DestructiveClass::WindowsDel);
    }
    if collapsed.contains("rd /s")
        || collapsed.contains("rmdir /s")
        || collapsed.contains("rd /s /q")
        || collapsed.contains("rmdir /s /q")
    {
        return Some(DestructiveClass::WindowsRd);
    }
    if collapsed.contains("remove-item") && collapsed.contains("-recurse") {
        return Some(DestructiveClass::WindowsRemoveItem);
    }
    if collapsed.split_whitespace().any(|t| t == "format") || collapsed.starts_with("format ") {
        // avoid matching "format string" in code — look for drive-like
        if collapsed.contains("format c:")
            || collapsed.contains("format d:")
            || collapsed.contains("format /")
            || collapsed.starts_with("format ")
        {
            return Some(DestructiveClass::WindowsFormat);
        }
    }

    // DB
    if collapsed.contains("drop table")
        || collapsed.contains("drop database")
        || collapsed.contains("drop schema")
    {
        return Some(DestructiveClass::DbDrop);
    }
    if collapsed.contains("truncate ") || collapsed.contains("truncate table") {
        return Some(DestructiveClass::DbTruncate);
    }
    if is_unconditional_delete(&collapsed) {
        return Some(DestructiveClass::DbDelete);
    }
    if collapsed.contains("migrate down") || collapsed.contains("migration down") {
        return Some(DestructiveClass::DbMigrateDown);
    }

    None
}

fn regex_rm_r(s: &str) -> bool {
    // rm -r /path or rm -r path (space after -r)
    s.contains("rm -r ") || s.contains("rm -r\t")
}

fn matches_git_reset_hard(s: &str) -> bool {
    s.contains("git reset --hard")
        || (s.contains("reset") && s.contains("--hard") && s.contains("git"))
}

fn matches_git_push_force(s: &str) -> bool {
    if !s.contains("git") || !s.contains("push") {
        return false;
    }
    s.contains("--force-with-lease")
        || s.contains("--force")
        || s.split_whitespace().any(|t| t == "-f")
}

fn matches_git_clean(s: &str) -> bool {
    s.contains("git clean -fd")
        || s.contains("git clean -fdx")
        || (s.contains("git clean")
            && (s.contains("-fd") || s.contains("-fdx") || s.contains("-dff")))
}

fn is_unconditional_delete(s: &str) -> bool {
    // DELETE FROM t  without WHERE
    let trimmed = s.trim();
    if !trimmed.contains("delete from") && !trimmed.starts_with("delete ") {
        return false;
    }
    if trimmed.contains("delete from") {
        let after = trimmed.split("delete from").nth(1).unwrap_or("");
        // if no where clause in the statement
        return !after.contains(" where ");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_req(cmd: &str, source: ActionSource) {
        let d = PolicyGate::evaluate(cmd, source).unwrap();
        match d {
            PolicyDecision::RequireConfirmation {
                never_ask_bypasses, ..
            } => assert!(!never_ask_bypasses),
            other => panic!("expected RequireConfirmation for {cmd:?}, got {other:?}"),
        }
    }

    #[test]
    fn table_pattern_x_source_x_never_ask() {
        let patterns = [
            "rm -rf ./data",
            "rm -r ./tmp",
            "dd if=/dev/zero of=/dev/sda",
            "unlink /tmp/x",
            "mkfs.ext4 /dev/sdb1",
            "del /s C:\\tmp\\*",
            "rd /s /q build",
            "rmdir /s /q build",
            "Remove-Item -Recurse -Force .\\db",
            "format C:",
            "DROP TABLE experiences",
            "TRUNCATE TABLE experiences",
            "DELETE FROM experiences",
            "migrate down",
            "git reset --hard HEAD",
            "git push --force origin main",
            "git push -f origin main",
            "git push --force-with-lease",
            "git clean -fd",
            "git clean -fdx",
        ];
        for p in patterns {
            for src in [ActionSource::Agent, ActionSource::User] {
                assert_req(p, src);
            }
        }
        assert!(!PolicyGate::never_ask_bypasses_destructive());
    }

    #[test]
    fn safe_commands_allowed() {
        for cmd in ["ls -la", "cargo test", "git status", "git push origin main"] {
            let d = PolicyGate::evaluate(cmd, ActionSource::Agent).unwrap();
            assert!(matches!(d, PolicyDecision::Allow), "{cmd}");
        }
    }
}
