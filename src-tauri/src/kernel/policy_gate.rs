//! PolicyGate — destructive operations always require confirmation.
//! "Never Ask" / auto-approve cannot bypass this class (K6 / AP-06/07).
//! Matching is argv-token based (flag clusters, aliases), not substring order.
//! F17: recursively unwrap shell/sudo/env wrappers before classifying.

use serde::{Deserialize, Serialize};

const UNWRAP_DEPTH_CAP: usize = 8;

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
    /// PowerShell `-EncodedCommand` — payload cannot be inspected.
    OpaqueEncodedCommand,
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

/// Quote-aware shell word split (handles `"…"` / `'…'`).
pub fn shell_words(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = input.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    while let Some(c) = chars.next() {
        if in_single {
            if c == '\'' {
                in_single = false;
            } else {
                cur.push(c);
            }
            continue;
        }
        if in_double {
            if c == '"' {
                in_double = false;
            } else if c == '\\' {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '\'' => in_single = true,
            '"' => in_double = true,
            c if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn program_base(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('"').trim_matches('\'');
    let base = trimmed
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(trimmed)
        .to_ascii_lowercase();
    base.strip_suffix(".exe").unwrap_or(&base).to_string()
}

/// Tokenize a command line into program + argv, expanding short-flag clusters (`-rfvid` → r,f,v,i,d)
/// and Windows joined flags (`/s/q` → s,q).
pub fn tokenize_command(command: &str) -> (String, Vec<String>, Vec<char>) {
    let raw = shell_words(command);
    if raw.is_empty() {
        return (String::new(), Vec::new(), Vec::new());
    }
    let program_base = program_base(&raw[0]);
    let (args, short_flags) = expand_flags(&raw[1..]);
    (program_base, args, short_flags)
}

fn expand_flags(tokens: &[String]) -> (Vec<String>, Vec<char>) {
    let mut args = Vec::new();
    let mut short_flags = Vec::new();
    for tok in tokens {
        let lower = tok.to_ascii_lowercase();
        if lower.starts_with("--") {
            args.push(lower.trim_start_matches('-').to_string());
        } else if tok.starts_with('-') && tok.len() > 1 && !tok[1..].starts_with('-') {
            let orig_body = &tok[1..];
            let body = orig_body.to_ascii_lowercase();
            if orig_body.len() == 1 {
                short_flags.push(orig_body.chars().next().unwrap());
            } else if orig_body.chars().all(|c| c.is_ascii_lowercase()) {
                // Short cluster of any length: -xdf / -vrf / -rfvid
                for c in orig_body.chars() {
                    short_flags.push(c);
                }
            } else if orig_body.chars().all(|c| c.is_ascii_alphabetic())
                && orig_body.chars().any(|c| c.is_ascii_uppercase())
                && orig_body.chars().any(|c| c.is_ascii_lowercase())
            {
                // PowerShell-style long flag: -Recurse / -Force / -EncodedCommand
                args.push(body);
            } else if orig_body.chars().all(|c| c.is_ascii_alphabetic()) {
                // Mixed short cluster e.g. -Rf / -Rv
                for c in orig_body.chars() {
                    short_flags.push(c);
                }
            } else {
                for c in orig_body.chars() {
                    if c.is_ascii_alphabetic() {
                        short_flags.push(c);
                    }
                }
            }
        } else if lower.starts_with('/') && lower.len() > 1 {
            // Windows style /s /q /f or joined /s/q
            let body = &lower[1..];
            if body.chars().all(|c| c == '/' || c.is_ascii_alphabetic()) {
                for c in body.chars() {
                    if c.is_ascii_alphabetic() {
                        short_flags.push(c);
                    }
                }
            } else {
                args.push(lower);
            }
        } else {
            args.push(lower);
        }
    }
    (args, short_flags)
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

fn is_posix_shell(program: &str) -> bool {
    matches!(
        program,
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" | "ash"
    )
}

fn is_powershell(program: &str) -> bool {
    matches!(program, "powershell" | "pwsh")
}

fn is_prefix_wrapper(program: &str) -> bool {
    matches!(
        program,
        "sudo" | "doas" | "nice" | "nohup" | "xargs" | "time" | "command" | "exec"
    )
}

/// Extract payload after `-c` / combined `-lc` for POSIX shells.
fn extract_shell_c_payload(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let lower = a.to_ascii_lowercase();
        if lower == "-c" || lower == "--command" {
            return args.get(i + 1).cloned();
        }
        if a.starts_with('-') && !a.starts_with("--") {
            let body = &a[1..];
            if body.chars().all(|c| c.is_ascii_alphabetic())
                && body.to_ascii_lowercase().contains('c')
            {
                return args.get(i + 1).cloned();
            }
        }
        i += 1;
    }
    None
}

fn extract_cmd_payload(args: &[String]) -> Option<String> {
    let mut i = 0;
    while i < args.len() {
        let lower = args[i].to_ascii_lowercase();
        if lower == "/c" || lower == "/k" {
            let rest = &args[i + 1..];
            if rest.is_empty() {
                return None;
            }
            // Re-join; shell_words already stripped outer quotes from each token.
            return Some(rest.join(" "));
        }
        i += 1;
    }
    None
}

fn ps_flag_kind(lower: &str) -> Option<(&'static str, Option<String>)> {
    let rest = lower.strip_prefix('-')?;
    let (bare, inline) = match rest.split_once(':') {
        Some((b, v)) => (b, Some(v.to_string())),
        None => (rest, None),
    };
    if bare == "encodedcommand"
        || bare == "enc"
        || bare == "ec"
        || (bare.len() >= 4 && "encodedcommand".starts_with(bare))
    {
        return Some(("encodedcommand", inline));
    }
    if bare == "command" || bare == "c" || (bare.len() >= 4 && "command".starts_with(bare)) {
        return Some(("command", inline));
    }
    None
}

fn extract_powershell_payload(args: &[String]) -> Result<Option<String>, DestructiveClass> {
    let mut i = 0;
    while i < args.len() {
        let lower = args[i].to_ascii_lowercase();
        let Some((kind, inline)) = ps_flag_kind(&lower) else {
            i += 1;
            continue;
        };
        if kind == "encodedcommand" {
            return Err(DestructiveClass::OpaqueEncodedCommand);
        }
        if let Some(v) = inline {
            return Ok(Some(v));
        }
        return Ok(args.get(i + 1).cloned());
    }
    Ok(None)
}

fn skip_env_assignments(args: &[String]) -> &[String] {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            return &args[i + 1..];
        }
        if a == "-u" || a == "-C" || a == "-S" {
            i = (i + 2).min(args.len());
            continue;
        }
        if a.starts_with('-') {
            i += 1;
            continue;
        }
        if a.contains('=') {
            i += 1;
            continue;
        }
        break;
    }
    &args[i..]
}

fn skip_prefix_wrapper_args<'a>(program: &str, args: &'a [String]) -> &'a [String] {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            return &args[i + 1..];
        }
        if !a.starts_with('-') {
            break;
        }
        // Options that take a separate value.
        let takes_value = match program {
            "sudo" | "doas" => matches!(
                a.as_str(),
                "-u" | "-g"
                    | "-C"
                    | "-i"
                    | "--user"
                    | "--group"
                    | "--chdir"
                    | "--prompt"
                    | "-p"
                    | "-r"
                    | "-t"
                    | "-T"
            ),
            "nice" => matches!(a.as_str(), "-n" | "--adjustment"),
            "xargs" => matches!(
                a.as_str(),
                "-n" | "-P"
                    | "-s"
                    | "-I"
                    | "-J"
                    | "-L"
                    | "-R"
                    | "-E"
                    | "--max-args"
                    | "--max-procs"
                    | "--replace"
                    | "--delimiter"
            ),
            "time" => matches!(a.as_str(), "-f" | "-o" | "--format" | "--output"),
            "command" => false,
            _ => false,
        };
        if takes_value
            || (program == "nice" && a.starts_with("-n") && a.len() > 2)
            || (a.starts_with("--") && a.contains('='))
        {
            if takes_value && !a.contains('=') {
                i = (i + 2).min(args.len());
            } else {
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    &args[i..]
}

/// Skip git global options before the subcommand (F17).
pub fn strip_git_globals(args: &[String]) -> Vec<String> {
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let lower = a.to_ascii_lowercase();
        if a == "-C" || a == "-c" {
            i = (i + 2).min(args.len());
            continue;
        }
        if lower == "--no-pager" || a == "-p" || lower == "--paginate" {
            i += 1;
            continue;
        }
        if lower.starts_with("--git-dir=") || lower.starts_with("--work-tree=") {
            i += 1;
            continue;
        }
        if lower == "--git-dir" || lower == "--work-tree" {
            i = (i + 2).min(args.len());
            continue;
        }
        return args[i..].to_vec();
    }
    Vec::new()
}

pub fn classify_destructive(command: &str) -> Option<DestructiveClass> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    classify_words(&shell_words(trimmed), 0)
}

fn classify_words(words: &[String], depth: usize) -> Option<DestructiveClass> {
    if words.is_empty() || depth > UNWRAP_DEPTH_CAP {
        return None;
    }
    let program = program_base(&words[0]);
    let args = &words[1..];

    // PowerShell / pwsh
    if is_powershell(&program) {
        match extract_powershell_payload(args) {
            Err(class) => return Some(class),
            Ok(Some(payload)) => return classify_words(&shell_words(&payload), depth + 1),
            Ok(None) => {}
        }
    }

    // POSIX shells with -c / -lc
    if is_posix_shell(&program) {
        if let Some(payload) = extract_shell_c_payload(args) {
            return classify_words(&shell_words(&payload), depth + 1);
        }
    }

    // cmd /c /k
    if program == "cmd" {
        if let Some(payload) = extract_cmd_payload(args) {
            return classify_words(&shell_words(&payload), depth + 1);
        }
    }

    // env KEY=VAL …
    if program == "env" {
        let rest = skip_env_assignments(args);
        if !rest.is_empty() {
            return classify_words(rest, depth + 1);
        }
    }

    // sudo / doas / nice / nohup / xargs / time / command / exec
    if is_prefix_wrapper(&program) {
        let rest = skip_prefix_wrapper_args(&program, args);
        if !rest.is_empty() {
            return classify_words(rest, depth + 1);
        }
    }

    classify_program(&program, args)
}

fn classify_program(program: &str, raw_args: &[String]) -> Option<DestructiveClass> {
    let lower_line = std::iter::once(program)
        .chain(raw_args.iter().map(|s| s.as_str()))
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();

    // --- Git (strip globals before subcommand) ---
    if program == "git" {
        let stripped = strip_git_globals(raw_args);
        let (args, short) = expand_flags(&stripped);
        // expand_flags lowercases positional args; restore subcommand from stripped for case.
        // Subcommand is first non-flag token in stripped.
        if let Some(class) = classify_git(&args, &short) {
            return Some(class);
        }
    }

    let (args, short) = expand_flags(raw_args);

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
    let ps_recurse = has_long(&args, "recurse")
        || has_short(&short, 'r')
        || args.iter().any(|a| a.starts_with("rec"));
    if program != "rm"
        && (program == "remove-item" || (is_remove_item_alias(program) && ps_recurse))
    {
        return Some(DestructiveClass::WindowsRemoveItem);
    }

    // --- format ---
    if program == "format" {
        return Some(DestructiveClass::WindowsFormat);
    }

    // --- SQL / DB ---
    if let Some(class) = classify_sql(&lower_line, program, &args) {
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
                || args.iter().any(|a| a.starts_with('+'));
            if force {
                Some(DestructiveClass::GitPushForce)
            } else {
                None
            }
        }
        "clean" => {
            let has_f = has_short(short, 'f') || args.iter().any(|a| a == "force");
            let has_d =
                has_short(short, 'd') || args.iter().any(|a| a == "d" || a == "directories");
            let has_x = has_short(short, 'x');
            if has_f || has_d || has_x {
                return Some(DestructiveClass::GitClean);
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
    fn f17_wrapper_forms_are_destructive() {
        let patterns = [
            r#"sh -c "rm -rf x""#,
            "bash -lc 'git clean -fdx'",
            "sudo rm -rf /x",
            "env A=1 rm -rf x",
            "nohup rm -rf x",
            "xargs rm -rf",
            "cmd /c del /s /q x",
            "rd /s/q x",
            r#"powershell -Command "Remove-Item -Recurse x""#,
            "git -C dir clean -fdx",
            "git -c k=v reset --hard",
            "rm -rfvid x",
            // extras that belong to the same unwrap surface
            "doas rm -rf /x",
            "nice rm -rf x",
            "time rm -rf x",
            "command rm -rf x",
            "exec rm -rf x",
            "zsh -c 'rm -rf x'",
            "pwsh -c \"Remove-Item -Recurse x\"",
            "cmd /k rd /s /q x",
            "powershell -EncodedCommand WW91",
        ];
        for p in patterns {
            for src in [ActionSource::Agent, ActionSource::User] {
                assert_req(p, src);
            }
        }
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
            "sh -c \"echo hi\"",
            "sudo ls /tmp",
            "env A=1 git status",
            "git -C dir status",
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

        let (_, _, long_cluster) = tokenize_command("rm -rfvid x");
        for c in ['r', 'f', 'v', 'i', 'd'] {
            assert!(long_cluster.contains(&c), "missing {c} in {long_cluster:?}");
        }

        let (_, _, win) = tokenize_command("rd /s/q x");
        assert!(win.contains(&'s'));
        assert!(win.contains(&'q'));
    }

    #[test]
    fn shell_words_respects_quotes() {
        let words = shell_words(r#"sh -c "rm -rf x""#);
        assert_eq!(words, vec!["sh", "-c", "rm -rf x"]);
        let words = shell_words("bash -lc 'git clean -fdx'");
        assert_eq!(words, vec!["bash", "-lc", "git clean -fdx"]);
    }

    #[test]
    fn strip_git_globals_skips_c_and_config() {
        let args = shell_words("-C dir clean -fdx");
        assert_eq!(
            strip_git_globals(&args)
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            vec!["clean", "-fdx"]
        );
        let args = shell_words("-c k=v reset --hard");
        assert_eq!(
            strip_git_globals(&args)
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            vec!["reset", "--hard"]
        );
    }
}
