//! Open a source file in the user's editor (custom command or platform default).

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::db::{accepts_cross_platform_path, normalize_path_str, ExperienceStore};
use crate::kernel::{ActionSource, GuardedCommand};

const SETTING_EDITOR_COMMAND: &str = "editor_command";
const SETTING_EDITOR_PRESET: &str = "editor_preset";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsFamily {
    Macos,
    Windows,
    Linux,
}

impl OsFamily {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

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

/// Resolve preset → display template for the current OS (settings UI / legacy key).
pub fn preset_argv_template(preset: EditorPreset) -> (String, String) {
    let (prog, args) = build_preset_argv(OsFamily::current(), preset, "{path}", None);
    (prog, args.join(" "))
}

/// Structured argv for a preset. Spaced macOS app names stay a single argv element.
/// Default opener never appends `:line` to the path.
pub fn build_preset_argv(
    os: OsFamily,
    preset: EditorPreset,
    path: &str,
    line: Option<i64>,
) -> (String, Vec<String>) {
    match preset {
        EditorPreset::Default => build_default_argv(os, path),
        EditorPreset::VsCode => build_vscode_argv(os, path, line),
        EditorPreset::Cursor => build_cursor_argv(os, path, line),
        EditorPreset::Custom => (String::new(), vec!["{path}".into()]),
    }
}

fn goto_target(path: &str, line: Option<i64>) -> String {
    match line {
        Some(n) if n > 0 => format!("{path}:{n}:1"),
        _ => path.to_string(),
    }
}

fn build_default_argv(os: OsFamily, path: &str) -> (String, Vec<String>) {
    // Default OS opener: path only — never `path:line`.
    match os {
        OsFamily::Macos => ("open".into(), vec![path.to_string()]),
        OsFamily::Windows => ("explorer.exe".into(), vec![path.to_string()]),
        OsFamily::Linux => ("xdg-open".into(), vec![path.to_string()]),
    }
}

fn build_vscode_argv(os: OsFamily, path: &str, line: Option<i64>) -> (String, Vec<String>) {
    let target = goto_target(path, line);
    match os {
        OsFamily::Windows => ("code.cmd".into(), vec!["--goto".into(), target]),
        OsFamily::Macos => (
            "open".into(),
            vec![
                "-a".into(),
                "Visual Studio Code".into(),
                "--args".into(),
                "--goto".into(),
                target,
            ],
        ),
        OsFamily::Linux => ("code".into(), vec!["--goto".into(), target]),
    }
}

fn build_cursor_argv(os: OsFamily, path: &str, line: Option<i64>) -> (String, Vec<String>) {
    let target = goto_target(path, line);
    match os {
        OsFamily::Windows => ("Cursor.exe".into(), vec!["--goto".into(), target]),
        OsFamily::Macos => (
            "open".into(),
            vec![
                "-a".into(),
                "Cursor".into(),
                "--args".into(),
                "--goto".into(),
                target,
            ],
        ),
        OsFamily::Linux => ("cursor".into(), vec!["--goto".into(), target]),
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

/// Expand a custom argument template. Line is attached as `path:line` only for custom editors.
fn expand_custom_template(template: &str, path: &str, line: Option<i64>) -> Vec<String> {
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

fn spawn_from_custom_template(
    program: &str,
    args_template: &str,
    path: &str,
    line: Option<i64>,
) -> Result<()> {
    validate_custom_editor(program, args_template)?;
    let args = expand_custom_template(args_template, path, line);
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
            let (prog, args) = build_preset_argv(OsFamily::current(), other, "{path}", None);
            format!("{prog} {}", args.join(" "))
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

fn open_with_settings(settings: &EditorSettings, path: &str, line: Option<i64>) -> Result<()> {
    match settings.preset {
        EditorPreset::Custom => spawn_from_custom_template(
            &settings.custom_program,
            &settings.custom_args_template,
            path,
            line,
        ),
        preset => {
            let (program, args) = build_preset_argv(OsFamily::current(), preset, path, line);
            spawn_editor_argv(&program, &args)
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
        spawn_from_custom_template(program, &template, &normalized, line)?;
        return Ok(());
    }

    let settings = load_editor_settings(store).await?;
    open_with_settings(&settings, &normalized, line)
}

/// Pure argv builder for the platform default opener (testable without spawning).
pub fn platform_opener_argv(path: &str) -> Result<(String, Vec<String>)> {
    Ok(build_default_argv(OsFamily::current(), path))
}

/// Windows: `explorer.exe` with the path as a single argv element (never `cmd /C start`).
pub fn windows_opener_argv(path: &str) -> Result<(String, Vec<String>)> {
    if !accepts_cross_platform_path(path) {
        bail!("path must include a separator or Windows drive letter");
    }
    Ok(build_default_argv(OsFamily::Windows, path))
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
    fn custom_expand_template_includes_line() {
        let args = expand_custom_template("-g {path}", "/tmp/x.rs", Some(42));
        assert_eq!(args, vec!["-g", "/tmp/x.rs:42"]);
    }

    #[test]
    fn default_opener_never_appends_line_on_any_os() {
        for os in [OsFamily::Linux, OsFamily::Macos, OsFamily::Windows] {
            let (prog, args) =
                build_preset_argv(os, EditorPreset::Default, "/tmp/x.rs", Some(42));
            assert!(!prog.is_empty());
            assert_eq!(args, vec!["/tmp/x.rs".to_string()], "os={os:?}");
            assert!(
                !args.iter().any(|a| a.contains(":42")),
                "default must not use path:line (os={os:?})"
            );
        }
    }

    #[test]
    fn vscode_uses_goto_path_line_col_on_all_oses() {
        for os in [OsFamily::Linux, OsFamily::Macos, OsFamily::Windows] {
            let (_, args) = build_preset_argv(os, EditorPreset::VsCode, "/tmp/x.rs", Some(42));
            assert!(
                args.iter().any(|a| a == "--goto"),
                "missing --goto on {os:?}: {args:?}"
            );
            assert!(
                args.iter().any(|a| a == "/tmp/x.rs:42:1"),
                "missing goto target on {os:?}: {args:?}"
            );
        }
    }

    #[test]
    fn cursor_uses_goto_path_line_col_on_all_oses() {
        for os in [OsFamily::Linux, OsFamily::Macos, OsFamily::Windows] {
            let (_, args) = build_preset_argv(os, EditorPreset::Cursor, "/tmp/x.rs", Some(7));
            assert!(args.iter().any(|a| a == "--goto"), "{os:?}: {args:?}");
            assert!(
                args.iter().any(|a| a == "/tmp/x.rs:7:1"),
                "{os:?}: {args:?}"
            );
        }
    }

    #[test]
    fn macos_vscode_keeps_spaced_app_name_as_single_argv() {
        let (prog, args) =
            build_preset_argv(OsFamily::Macos, EditorPreset::VsCode, "/tmp/x.rs", Some(10));
        assert_eq!(prog, "open");
        assert_eq!(args[0], "-a");
        assert_eq!(args[1], "Visual Studio Code");
        assert!(!args.iter().any(|a| a == "Visual"));
        assert!(!args.iter().any(|a| a == "Studio"));
        assert!(!args.iter().any(|a| a == "Code"));
        assert!(args.iter().any(|a| a == "--args"));
        assert!(args.iter().any(|a| a == "--goto"));
    }

    #[test]
    fn macos_cursor_app_name_is_single_argv() {
        let (prog, args) =
            build_preset_argv(OsFamily::Macos, EditorPreset::Cursor, "/tmp/y.rs", None);
        assert_eq!(prog, "open");
        assert_eq!(args[0], "-a");
        assert_eq!(args[1], "Cursor");
        assert_eq!(
            args.iter().filter(|a| a.as_str() == "Cursor").count(),
            1,
            "Cursor must appear once as app name, not split"
        );
    }

    #[test]
    fn linux_and_windows_vscode_programs() {
        let (linux_prog, _) =
            build_preset_argv(OsFamily::Linux, EditorPreset::VsCode, "/tmp/x.rs", None);
        let (win_prog, _) =
            build_preset_argv(OsFamily::Windows, EditorPreset::VsCode, r"C:\x.rs", None);
        assert_eq!(linux_prog, "code");
        assert_eq!(win_prog, "code.cmd");
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
            assert!(args.contains("{path}") || prog == "open" || prog == "xdg-open" || prog == "explorer.exe");
        }
    }
}
