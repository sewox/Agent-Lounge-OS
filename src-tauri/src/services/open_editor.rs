//! Open a source file in the user's editor (custom command or platform default).

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::db::{accepts_cross_platform_path, normalize_path_str, ExperienceStore};
use crate::kernel::{ActionSource, GuardedCommand};

const SETTING_EDITOR_COMMAND: &str = "editor_command";
const SETTING_EDITOR_PRESET: &str = "editor_preset";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditorPreset {
    Default,
    VsCode,
    Cursor,
    Custom,
}

impl EditorPreset {
    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "vscode" | "vs_code" | "code" => Self::VsCode,
            "cursor" => Self::Cursor,
            "custom" => Self::Custom,
            _ => Self::Default,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::VsCode => "vscode",
            Self::Cursor => "cursor",
            Self::Custom => "custom",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorSettings {
    pub preset: EditorPreset,
    /// Custom editor: program + argument template with `{path}` placeholder.
    /// Validated in Rust — no shell, pipes, or env expansion.
    pub custom_program: String,
    pub custom_args_template: String,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            preset: EditorPreset::Default,
            custom_program: String::new(),
            custom_args_template: "{path}".into(),
        }
    }
}

/// Resolve preset → argv template for the current OS (no shell).
pub fn preset_argv_template(preset: EditorPreset) -> (String, String) {
    match preset {
        EditorPreset::Default => platform_default_editor_argv(),
        EditorPreset::VsCode => platform_vscode_argv(),
        EditorPreset::Cursor => platform_cursor_argv(),
        EditorPreset::Custom => (String::new(), "{path}".into()),
    }
}

fn platform_default_editor_argv() -> (String, String) {
    if cfg!(target_os = "macos") {
        ("open".into(), "{path}".into())
    } else if cfg!(target_os = "windows") {
        ("explorer.exe".into(), "{path}".into())
    } else {
        ("xdg-open".into(), "{path}".into())
    }
}

fn platform_vscode_argv() -> (String, String) {
    if cfg!(target_os = "windows") {
        ("code.cmd".into(), "-g {path}".into())
    } else if cfg!(target_os = "macos") {
        (
            "open".into(),
            "-a Visual Studio Code --args -g {path}".into(),
        )
    } else {
        ("code".into(), "-g {path}".into())
    }
}

fn platform_cursor_argv() -> (String, String) {
    if cfg!(target_os = "windows") {
        ("Cursor.exe".into(), "-g {path}".into())
    } else if cfg!(target_os = "macos") {
        ("open".into(), "-a Cursor --args -g {path}".into())
    } else {
        ("cursor".into(), "-g {path}".into())
    }
}

/// Validate custom editor template — no shell metacharacters.
pub fn validate_custom_editor(program: &str, args_template: &str) -> Result<()> {
    let prog = program.trim();
    if prog.is_empty() {
        bail!("editor program is required");
    }
    if prog.contains('|')
        || prog.contains('&')
        || prog.contains(';')
        || prog.contains('`')
        || prog.contains('$')
        || prog.contains('\n')
    {
        bail!("editor program must not contain shell metacharacters");
    }
    let args = args_template.trim();
    if args.is_empty() {
        bail!("editor argument template is required");
    }
    if !args.contains("{path}") {
        bail!("editor argument template must include {{path}} placeholder");
    }
    for bad in ['|', '&', ';', '`', '$', '\n'] {
        if args.contains(bad) {
            bail!("editor argument template must not contain shell metacharacters");
        }
    }
    if args
        .split_whitespace()
        .any(|token| token == "-c" || token == "--command")
    {
        bail!("editor argument template must not invoke a shell (-c)");
    }
    Ok(())
}

fn expand_template(template: &str, path: &str, line: Option<i64>) -> Vec<String> {
    let target = match line {
        Some(n) if n > 0 => format!("{path}:{n}"),
        _ => path.to_string(),
    };
    template
        .split_whitespace()
        .map(|part| part.replace("{path}", &target))
        .collect()
}

fn spawn_editor_argv(program: &str, args: &[String]) -> Result<()> {
    let mut cmd = GuardedCommand::new(program).source(ActionSource::User);
    for arg in args {
        cmd = cmd.arg(arg);
    }
    let _child = cmd.spawn().with_context(|| format!("spawn {program}"))?;
    Ok(())
}

fn spawn_from_template(
    program: &str,
    args_template: &str,
    path: &str,
    line: Option<i64>,
) -> Result<()> {
    validate_custom_editor(program, args_template)?;
    let args = expand_template(args_template, path, line);
    spawn_editor_argv(program, &args)
}

pub async fn load_editor_settings(store: &ExperienceStore) -> Result<EditorSettings> {
    let preset_raw = store
        .get_setting(SETTING_EDITOR_PRESET.into())
        .await?
        .unwrap_or_else(|| EditorPreset::Default.as_str().into());
    let preset = EditorPreset::parse(&preset_raw);
    let custom_program = store
        .get_setting(format!("{SETTING_EDITOR_COMMAND}_program"))
        .await?
        .unwrap_or_default();
    let custom_args_template = store
        .get_setting(format!("{SETTING_EDITOR_COMMAND}_args"))
        .await?
        .unwrap_or_else(|| "{path}".into());
    Ok(EditorSettings {
        preset,
        custom_program,
        custom_args_template,
    })
}

pub async fn save_editor_settings(
    store: &ExperienceStore,
    settings: &EditorSettings,
) -> Result<EditorSettings> {
    if settings.preset == EditorPreset::Custom {
        validate_custom_editor(&settings.custom_program, &settings.custom_args_template)?;
    }
    store
        .set_setting(
            SETTING_EDITOR_PRESET.into(),
            settings.preset.as_str().into(),
        )
        .await?;
    store
        .set_setting(
            format!("{SETTING_EDITOR_COMMAND}_program"),
            settings.custom_program.trim().into(),
        )
        .await?;
    store
        .set_setting(
            format!("{SETTING_EDITOR_COMMAND}_args"),
            settings.custom_args_template.trim().into(),
        )
        .await?;
    // Legacy single-line key for older call sites.
    let legacy = match settings.preset {
        EditorPreset::Custom => {
            format!(
                "{} {}",
                settings.custom_program.trim(),
                settings.custom_args_template.trim()
            )
        }
        other => {
            let (prog, args) = preset_argv_template(other);
            format!("{prog} {args}")
        }
    };
    store
        .set_setting(SETTING_EDITOR_COMMAND.into(), legacy)
        .await?;
    Ok(settings.clone())
}

pub async fn test_editor_open(
    _store: &ExperienceStore,
    settings: &EditorSettings,
    path: &str,
) -> Result<()> {
    open_with_settings(settings, path, None)
}

fn open_with_settings(
    settings: &EditorSettings,
    path: &str,
    line: Option<i64>,
) -> Result<()> {
    match settings.preset {
        EditorPreset::Custom => spawn_from_template(
            &settings.custom_program,
            &settings.custom_args_template,
            path,
            line,
        ),
        preset => {
            let (program, args_template) = preset_argv_template(preset);
            spawn_from_template(&program, &args_template, path, line)
        }
    }
}

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

    if let Some(cmd) = editor_command
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        // Legacy single-line override from call site.
        let mut parts = cmd.split_whitespace();
        let program = parts.next().unwrap_or(&cmd);
        let rest: Vec<String> = parts.map(str::to_string).collect();
        let template = if rest.is_empty() {
            "{path}".into()
        } else {
            rest.join(" ")
        };
        spawn_from_template(program, &template, &normalized, line)?;
        return Ok(());
    }

    let settings = load_editor_settings(store).await?;
    open_with_settings(&settings, &normalized, line)
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

    #[test]
    fn validate_custom_editor_rejects_shell() {
        assert!(validate_custom_editor("sh", "-c {path}").is_err());
        assert!(validate_custom_editor("code", "-g {path}").is_ok());
        assert!(validate_custom_editor("code", "-g").is_err());
    }

    #[test]
    fn expand_template_includes_line() {
        let args = expand_template("-g {path}", "/tmp/x.rs", Some(42));
        assert_eq!(args, vec!["-g", "/tmp/x.rs:42"]);
    }

    #[test]
    fn preset_argv_templates_are_non_empty() {
        for preset in [
            EditorPreset::Default,
            EditorPreset::VsCode,
            EditorPreset::Cursor,
        ] {
            let (prog, args) = preset_argv_template(preset);
            assert!(!prog.is_empty());
            assert!(args.contains("{path}"));
        }
    }
}
