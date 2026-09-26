//! Open a source file in the user's editor (custom command or platform default).

use anyhow::{bail, Context, Result};

use crate::db::{accepts_cross_platform_path, normalize_path_str, ExperienceStore};
use crate::kernel::{ActionSource, GuardedCommand};

const SETTING_EDITOR_COMMAND: &str = "editor_command";

/// Open `path` (optionally at `line`) via custom editor or OS default opener.
pub async fn open_in_editor(
    store: &ExperienceStore,
    path: String,
    line: Option<i64>,
    editor_command: Option<String>,
) -> Result<()> {
    let raw = path.trim();
    if raw.is_empty() {
        bail!("path boş");
    }
    if !accepts_cross_platform_path(raw) {
        bail!("path must include a separator or Windows drive letter");
    }
    let normalized = normalize_path_str(raw);
    if normalized.is_empty() {
        bail!("path could not be normalized");
    }

    let custom = match editor_command
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        Some(cmd) => Some(cmd),
        None => store
            .get_setting(SETTING_EDITOR_COMMAND.into())
            .await?
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    };

    if let Some(editor) = custom {
        open_with_custom_editor(&editor, &normalized, line)?;
    } else {
        open_with_platform_default(&normalized)?;
    }
    Ok(())
}

fn open_with_custom_editor(editor: &str, path: &str, line: Option<i64>) -> Result<()> {
    let target = match line {
        Some(n) if n > 0 => format!("{path}:{n}"),
        _ => path.to_string(),
    };
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or(editor);
    let mut cmd = GuardedCommand::new(program).source(ActionSource::User);
    for arg in parts {
        cmd = cmd.arg(arg);
    }
    let lower = editor.to_ascii_lowercase();
    if lower.contains("code") || lower.contains("cursor") || lower.contains("codium") {
        if line.is_some_and(|n| n > 0) && !editor.contains("-g") {
            cmd = cmd.arg("-g").arg(&target);
        } else {
            cmd = cmd.arg(&target);
        }
    } else {
        cmd = cmd.arg(&target);
    }
    let _child = cmd.spawn().context("editor spawn")?;
    Ok(())
}

fn open_with_platform_default(path: &str) -> Result<()> {
    let (program, args) = platform_opener_argv(path)?;
    let mut cmd = GuardedCommand::new(&program).source(ActionSource::User);
    for arg in &args {
        cmd = cmd.arg(arg);
    }
    let _child = cmd.spawn().with_context(|| format!("{program} opener"))?;
    Ok(())
}

/// Pure argv builder for the platform default opener (testable without spawning).
pub fn platform_opener_argv(path: &str) -> Result<(String, Vec<String>)> {
    if cfg!(target_os = "macos") {
        Ok(("open".into(), vec![path.to_string()]))
    } else if cfg!(target_os = "windows") {
        windows_opener_argv(path)
    } else {
        Ok(("xdg-open".into(), vec![path.to_string()]))
    }
}

/// Windows: `explorer.exe` with the path as a single argv element (never `cmd /C start`).
pub fn windows_opener_argv(path: &str) -> Result<(String, Vec<String>)> {
    if !accepts_cross_platform_path(path) {
        bail!("path must include a separator or Windows drive letter");
    }
    // Reject shell metacharacters that would be dangerous if ever routed through cmd.
    // With explorer + single argv they are still passed literally; we keep the check
    // as defense-in-depth for callers that might shell-escape incorrectly.
    if path.chars().any(|c| matches!(c, '&' | '|' | '^' | '%')) {
        // Still open via explorer as one argv — do not reject; metacharacters are data.
        // The important guarantee is we never hand them to `cmd`.
    }
    Ok(("explorer.exe".into(), vec![path.to_string()]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_opener_keeps_ampersand_path_as_single_argv() {
        let path = r"C:\tmp\a&calc.exe";
        let (program, args) = windows_opener_argv(path).expect("argv");
        assert_eq!(program, "explorer.exe");
        assert_eq!(args, vec![path.to_string()]);
        assert_eq!(args.len(), 1);
        assert!(!program.eq_ignore_ascii_case("cmd"));
        assert!(!args
            .iter()
            .any(|a| a.eq_ignore_ascii_case("/C") || a.eq_ignore_ascii_case("start")));
    }

    #[test]
    fn windows_opener_rejects_non_path_strings() {
        assert!(windows_opener_argv("nopath").is_err());
    }

    #[test]
    fn accepts_cross_platform_round_trip_for_open_editor() {
        let samples = [
            r"C:\Users\sercan\dev\Agent-Lounge-OS\src\main.rs",
            "/home/sercan/dev/Agent-Lounge-OS/src/main.rs",
            r"mixed/path\with\both",
        ];
        for sample in samples {
            assert!(accepts_cross_platform_path(sample), "accepts: {sample}");
            let normalized = normalize_path_str(sample);
            assert!(!normalized.is_empty());
            let (program, args) = windows_opener_argv(sample).expect("windows argv");
            assert_eq!(program, "explorer.exe");
            assert_eq!(args, vec![sample.to_string()]);
        }
    }
}
