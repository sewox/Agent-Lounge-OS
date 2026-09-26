//! PolicyGate — destructive operations always require confirmation.
//! "Never Ask" / auto-approve cannot bypass this class (K6 / AP-06/07).
//! Matching is argv-token based (flag clusters, aliases), not substring order.

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
    GitBranchForceDelete,
    GitCheckoutOverwrite,
    GitRestoreOverwrite,
    GitStashClear,
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

/// Tokenize a command line into program + argv, expanding short-flag clusters (`-xdf` → x,d,f).
pub fn tokenize_command(command: &str) -> (String, Vec<String>, Vec<char>) {
    let raw: Vec<String> = command.split_whitespace().map(|t| t.to_string()).collect();
    if raw.is_empty() {
        return (String::new(), Vec::new(), Vec::new());
    }
    let program = raw[0].to_ascii_lowercase();
    // Basename for paths like /usr/bin/rm
    let program_base = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&program)
        .to_string();
    let mut args = Vec::new();
    let mut short_flags = Vec::new();
    for tok in raw.iter().skip(1) {
        let lower = tok.to_ascii_lowercase();
        if lower.starts_with("--") {
            args.push(lower.trim_start_matches('-').to_string());
        } else if tok.starts_with('-') && tok.len() > 1 && !tok[1..].starts_with('-') {
            // Keep original case for single-letter flags (git branch -D vs -d).
            let orig_body = &tok[1..];
            let body = orig_body.to_ascii_lowercase();
            if orig_body.len() == 1 {
                short_flags.push(orig_body.chars().next().unwrap());
            } else if body.len() <= 4 && orig_body.chars().all(|c| c.is_ascii_alphabetic()) {
                // Short cluster: -xdf / -vrf / -R (already handled) / -fr
                for c in orig_body.chars() {
                    short_flags.push(c);
                }
            } else if body.chars().all(|c| c.is_ascii_alphabetic()) {
                // Long PowerShell-style: -Recurse / -Force
                args.push(body);
            } else {
                for c in orig_body.chars() {
                    if c.is_ascii_alphabetic() {
                        short_flags.push(c);
                    }
                }
            }
        } else if lower.starts_with('/') && lower.len() == 2 {
            // Windows style /s /q /f
            short_flags.push(lower.chars().nth(1).unwrap());
        } else {
            args.push(lower);
        }
    }
    (program_base, args, short_flags)
}

fn has_short(flags: &[char], c: char) -> bool {
    let needle = c.to_ascii_lowercase();
    flags.iter().any(|f| f.to_ascii_lowercase() == needle)
}

fn has_short_exact(flags: &[char], c: char) -> bool {
    flags.contains(&c)
}

fn has_long(args: &[String], name: &str) -> bool {
    let needle = name.to_ascii_lowercase();
    args.iter().any(|a| {
        a == &needle
            || a.starts_with(&format!("{needle}="))
            // PowerShell abbreviations: -Recurse → rec, -r
            || (needle.len() >= 3 && a.starts_with(&needle[..needle.len().min(3)]))
            || (needle == "recurse" && (a == "r" || a.starts_with("rec")))
            || (needle == "force" && (a == "f" || a.starts_with("for")))
    })
}

fn is_remove_item_alias(program: &str) -> bool {
    matches!(
        program,
        "remove-item" | "ri" | "rm" | "del" | "erase" | "rd" | "rmdir"
    )
}

pub fn classify_destructive(command: &str) -> Option<DestructiveClass> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (program, args, short) = tokenize_command(trimmed);
    let lower_line = trimmed.to_ascii_lowercase();

    // --- Git ---
    if program == "git" || (args.first().map(|a| a.as_str()) == Some("git")) {
        // handle `git ...` when program is git
    }
    if program == "git" {
        if let Some(class) = classify_git(&args, &short) {
            return Some(class);
        }
    }

    // --- POSIX rm ---
    if program == "rm" {
        let recursive = has_short(&short, 'r')
            || has_short_exact(&short, 'R')
            || has_long(&args, "recursive")
            || has_long(&args, "recurse");
        if recursive {
            return Some(DestructiveClass::PosixRm);
        }
    }

    // --- dd / unlink / mkfs ---
    if program == "dd" {
        return Some(DestructiveClass::PosixDd);
    }
    if program == "unlink" {
        return Some(DestructiveClass::PosixUnlink);
    }
    if program.starts_with("mkfs") {
        return Some(DestructiveClass::PosixMkfs);
    }

    // --- Windows cmd del / rd / rmdir ---
    if program == "del" || program == "erase" {
        // PowerShell Remove-Item alias vs cmd del /s
        if has_short(&short, 's') {
            return Some(DestructiveClass::WindowsDel);
        }
        if has_long(&args, "recurse") || has_short(&short, 'r') {
            return Some(DestructiveClass::WindowsRemoveItem);
        }
    }
    if program == "rd" || program == "rmdir" {
        if has_short(&short, 's') {
            return Some(DestructiveClass::WindowsRd);
        }
        if has_long(&args, "recurse") || has_short(&short, 'r') {
            return Some(DestructiveClass::WindowsRemoveItem);
        }
    }

    // --- PowerShell Remove-Item (+ aliases when -Recurse) ---
    // POSIX `rm -r` already handled above.
    let ps_recurse = has_long(&args, "recurse")
        || has_short(&short, 'r')
        || args.iter().any(|a| a.starts_with("rec"));
    if program != "rm"
        && (program == "remove-item" || (is_remove_item_alias(&program) && ps_recurse))
    {
        return Some(DestructiveClass::WindowsRemoveItem);
    }

    // --- format ---
    if program == "format" {
        return Some(DestructiveClass::WindowsFormat);
    }

    // --- SQL / DB (statement-oriented, not free text) ---
    if let Some(class) = classify_sql(&lower_line, &program, &args) {
        return Some(class);
    }

    None
}

fn classify_git(args: &[String], short: &[char]) -> Option<DestructiveClass> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    match sub {
        "reset" if args.iter().any(|a| a == "hard") || has_long(args, "hard") => {
            Some(DestructiveClass::GitResetHard)
        }
        "push" => {
            let force = has_long(args, "force")
                || has_long(args, "force-with-lease")
                || has_short(short, 'f')
                || args.iter().any(|a| a == "f")
                // force refspec: +main / +refs/heads/main
                || args.iter().any(|a| a.starts_with('+'));
            if force {
                Some(DestructiveClass::GitPushForce)
            } else {
                None
            }
        }
        "clean" => {
            // -fd, -xdf, -ffd, -df, -f -d, -f alone, etc.
            let has_f = has_short(short, 'f') || args.iter().any(|a| a == "force");
            let has_d =
                has_short(short, 'd') || args.iter().any(|a| a == "d" || a == "directories");
            let has_x = has_short(short, 'x');
            if has_f || has_d || has_x || !short.is_empty() {
                // any clean with destructive-ish flags; plain `git clean` interactive is milder
                // but plan lists `git clean -f` as destructive
                if has_f || has_d || has_x {
                    return Some(DestructiveClass::GitClean);
                }
            }
            None
        }
        "branch" if has_short_exact(short, 'D') || args.iter().any(|a| a == "D") => {
            Some(DestructiveClass::GitBranchForceDelete)
        }
        "checkout" if args.iter().any(|a| a == ".") => Some(DestructiveClass::GitCheckoutOverwrite),
        "restore" if args.iter().any(|a| a == ".") => Some(DestructiveClass::GitRestoreOverwrite),
        "stash" if args.get(1).map(|s| s.as_str()) == Some("clear") => {
            Some(DestructiveClass::GitStashClear)
        }
        _ => None,
    }
}

fn classify_sql(lower_line: &str, program: &str, _args: &[String]) -> Option<DestructiveClass> {
    // Free-text "truncate the string" / echo wrappers must NOT match.
    if matches!(program, "echo" | "printf" | "cat" | "true" | "false") {
        return None;
    }
    let tokens: Vec<&str> = lower_line.split_whitespace().collect();
    if tokens.is_empty() {
        return None;
    }

    if let Some(i) = tokens.iter().position(|t| *t == "drop") {
        if let Some(next) = tokens.get(i + 1) {
            if matches!(*next, "table" | "database" | "schema" | "index") {
                return Some(DestructiveClass::DbDrop);
            }
        }
    }

    // TRUNCATE TABLE … or the truncate(1) binary — never prose.
    if tokens.len() >= 2 && tokens[0] == "truncate" && tokens[1] == "table" {
        return Some(DestructiveClass::DbTruncate);
    }
    if program == "truncate" {
        return Some(DestructiveClass::DbTruncate);
    }

    if let Some(i) = tokens.iter().position(|t| *t == "delete") {
        if tokens.get(i + 1) == Some(&"from") {
            let after = &tokens[i + 2..];
            if !after.contains(&"where") {
                return Some(DestructiveClass::DbDelete);
            }
        }
    }

    if lower_line.contains("migrate down") || lower_line.contains("migration down") {
        return Some(DestructiveClass::DbMigrateDown);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_req(cmd: &str, source: ActionSource) {
        let d = PolicyGate::evaluate(cmd, source).unwrap();
        match d {
            PolicyDecision::RequireConfirmation {
                never_ask_bypasses, ..
            } => assert!(!never_ask_bypasses, "{cmd}"),
            other => panic!("expected RequireConfirmation for {cmd:?}, got {other:?}"),
        }
    }

    fn assert_allow(cmd: &str) {
        let d = PolicyGate::evaluate(cmd, ActionSource::Agent).unwrap();
        assert!(
            matches!(d, PolicyDecision::Allow),
            "expected Allow for {cmd:?}, got {d:?}"
        );
    }

    #[test]
    fn table_pattern_x_source_x_never_ask() {
        let patterns = [
            // baseline
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
            // F4 bypasses — Windows flag order
            "del /q /s x",
            "del /f /s /q x",
            "rd /q /s build",
            "rmdir /q /s build",
            // PowerShell
            "Remove-Item -r x",
            "Remove-Item x -Rec",
            "ri -Recurse x",
            "rm -Recurse x",
            "del -Recurse x",
            // POSIX flag clusters / long flags
            "rm -rv x",
            "rm -vrf x",
            "rm --recursive x",
            "rm -R -v x",
            // git
            "git clean -xdf",
            "git clean -f -d",
            "git clean -ffd",
            "git clean -df",
            "git clean -f",
            "git push origin +main",
            // K6 additions
            "git branch -D stale",
            "git checkout -- .",
            "git restore .",
            "git stash clear",
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
        for cmd in [
            "ls -la",
            "ls -r",
            "cargo test",
            "cargo fmt",
            "git status",
            "git push origin main",
            "echo \"truncate the string\"",
            "echo truncate the string",
        ] {
            assert_allow(cmd);
        }
    }

    #[test]
    fn tokenize_expands_short_flag_clusters() {
        let (prog, _args, flags) = tokenize_command("rm -vrf /tmp/x");
        assert_eq!(prog, "rm");
        assert!(flags.contains(&'v'));
        assert!(flags.contains(&'r'));
        assert!(flags.contains(&'f'));
    }
}
