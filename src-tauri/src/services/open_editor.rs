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
    let (program, args): (&str, Vec<&str>) = if cfg!(target_os = "macos") {
        ("open", vec![path])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/C", "start", "", path])
    } else {
        ("xdg-open", vec![path])
    };
    let mut cmd = GuardedCommand::new(program).source(ActionSource::User);
    for arg in args {
        cmd = cmd.arg(arg);
    }
    let _child = cmd.spawn().with_context(|| format!("{program} opener"))?;
    Ok(())
}
