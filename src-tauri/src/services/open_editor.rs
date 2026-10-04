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

/// Denied program stems (case-insensitive; `.exe`/`.com` stripped before match).
/// Python family (`python`, `python3.12`, `pythonw`, …) handled separately.
const CUSTOM_EDITOR_DENIED_STEMS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "csh",
    "tcsh",
    "cmd",
    "powershell",
    "pwsh",
    "node",
    "nodejs",
    "deno",
    "bun",
    "perl",
    "ruby",
    "php",
    "osascript",
    "wscript",
    "cscript",
    "mshta",
    "curl",
    "wget",
    "busybox",
    "env",
    "wsl",
    "ssh",
];

/// Windows editor launcher scripts that are safe as custom-editor programs.
/// Random `.cmd` / `.bat` / `.ps1` remain denied; only this explicit allow-list passes.
/// Basenames are compared case-insensitively after stripping trailing `.` / spaces.
const CUSTOM_EDITOR_ALLOWED_SCRIPT_BASENAMES: &[&str] = &["code.cmd", "cursor.cmd"];

fn home_dir_for_editor() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

fn expand_home_prefix(program: &str) -> String {
    let trimmed = program.trim();
    if trimmed == "~" {
        return home_dir_for_editor()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|| trimmed.to_string());
    }
    if let Some(rest) = trimmed
        .strip_prefix("~/")
        .or_else(|| trimmed.strip_prefix("~\\"))
    {
        if let Some(home) = home_dir_for_editor() {
            return home.join(rest).to_string_lossy().into_owned();
        }
    }
    trimmed.to_string()
}

/// Basename using both `/` and `\` (cross-platform string logic).
fn custom_editor_program_basename(program: &str) -> String {
    let trimmed = program.trim();
    let base = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed).trim();
    base.to_ascii_lowercase()
}

/// Windows allows trailing `.` / spaces on executable names (`cmd.exe.`).
fn strip_trailing_windows_junk(name: &str) -> &str {
    name.trim_end_matches([' ', '.'])
}

fn editor_program_stem(basename_lower: &str) -> &str {
    let cleaned = strip_trailing_windows_junk(basename_lower);
    for ext in [".exe", ".com"] {
        if let Some(stem) = cleaned.strip_suffix(ext) {
            return stem;
        }
    }
    cleaned
}

fn is_allowed_editor_script_basename(basename_lower: &str) -> bool {
    let cleaned = strip_trailing_windows_junk(basename_lower);
    CUSTOM_EDITOR_ALLOWED_SCRIPT_BASENAMES.contains(&cleaned)
}

fn is_denied_script_extension(basename_lower: &str) -> bool {
    let cleaned = strip_trailing_windows_junk(basename_lower);
    if is_allowed_editor_script_basename(cleaned) {
        return false;
    }
    cleaned.ends_with(".bat") || cleaned.ends_with(".cmd") || cleaned.ends_with(".ps1")
}

fn is_python_family_stem(stem: &str) -> bool {
    if stem == "python" || stem == "py" || stem == "pythonw" {
        return true;
    }
    if let Some(rest) = stem.strip_prefix("pythonw") {
        // pythonw3, pythonw3.12 — windowless launcher still runs Python
        return rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit() || c == '.');
    }
    if let Some(rest) = stem.strip_prefix("python") {
        // python3, python3.12, python310 — not "pythonium"
        return !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '.');
    }
    false
}

fn basename_is_denied_editor(basename_lower: &str) -> bool {
    if is_allowed_editor_script_basename(basename_lower) {
        return false;
    }
    if is_denied_script_extension(basename_lower) {
        return true;
    }
    let stem = editor_program_stem(basename_lower);
    if stem.is_empty() {
        return false;
    }
    is_python_family_stem(stem) || CUSTOM_EDITOR_DENIED_STEMS.contains(&stem)
}

/// Candidates: raw path, `~` expanded, and canonicalize/symlink target when present.
fn editor_denylist_path_candidates(program: &str) -> Vec<String> {
    let mut out = Vec::new();
    let trimmed = program.trim().to_string();
    if !trimmed.is_empty() {
        out.push(trimmed.clone());
    }
    let expanded = expand_home_prefix(&trimmed);
    if expanded != trimmed {
        out.push(expanded.clone());
    }
    for candidate in [trimmed.as_str(), expanded.as_str()] {
        let path = std::path::Path::new(candidate);
        if let Ok(canon) = std::fs::canonicalize(path) {
            let s = canon.to_string_lossy().into_owned();
            if !out.iter().any(|x| x == &s) {
                out.push(s);
            }
        }
    }
    out
}

/// True when program (or its resolved symlink target) is a denied shell/interpreter.
pub fn custom_editor_program_is_denied(program: &str) -> bool {
    for candidate in editor_denylist_path_candidates(program) {
        let base = custom_editor_program_basename(&candidate);
        if basename_is_denied_editor(&base) {
            return true;
        }
    }
    false
}

/// Validate custom editor template — no shell metacharacters; deny interpreters.
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
    if custom_editor_program_is_denied(prog) {
        let base = custom_editor_program_basename(prog);
        bail!("editor program is not allowed (shell/interpreter denylist): {base}");
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
            // Presets use fixed argv builders but still pass the denylist so
            // Windows launchers like `code.cmd` are allow-listed explicitly —
            // never an unchecked bypass of script-extension denial.
            if custom_editor_program_is_denied(&program) {
                let base = custom_editor_program_basename(&program);
                bail!("editor program is not allowed (shell/interpreter denylist): {base}");
            }
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
    fn validate_custom_editor_denies_interpreters_by_basename() {
        for prog in [
            "python3",
            "python3.12",
            "PYTHON3.12",
            "/usr/bin/python3",
            "pythonw",
            "pythonw3",
            "pythonw.exe",
            "node",
            "deno",
            "bun",
            "ssh",
            "ssh.exe",
            r"C:\Windows\System32\cmd.exe",
            "PowerShell",
            "curl",
            "env",
            "wsl",
            "wsl.exe",
            "bash.exe",
            "sh.exe",
            "zsh.exe",
            "evil.bat",
            "run.cmd",
            "hack.ps1",
            "cmd.exe.",
            "cmd.exe ",
        ] {
            let err = validate_custom_editor(prog, "{path}").expect_err(prog);
            assert!(
                err.to_string().contains("denylist") || err.to_string().contains("shell"),
                "{prog}: {err}"
            );
        }
        assert!(validate_custom_editor("code", "-g {path}").is_ok());
        assert!(validate_custom_editor("code.cmd", "--goto {path}").is_ok());
        assert!(validate_custom_editor("cursor.cmd", "--goto {path}").is_ok());
        assert!(validate_custom_editor(
            r"C:\Program Files\Microsoft VS Code\bin\code.cmd",
            "--goto {path}"
        )
        .is_ok());
        assert!(validate_custom_editor("/usr/local/bin/nvim", "{path}").is_ok());
        assert!(validate_custom_editor("subl", "{path}").is_ok());
        // Not a python* version family.
        assert!(validate_custom_editor("pythonium", "{path}").is_ok());
    }

    #[test]
    fn editor_denylist_string_logic_is_case_and_suffix_insensitive() {
        assert!(custom_editor_program_is_denied("BaSh.EXE"));
        assert!(custom_editor_program_is_denied("python3.11"));
        assert!(custom_editor_program_is_denied("Py.exe"));
        assert!(custom_editor_program_is_denied("helper.cmd"));
        assert!(custom_editor_program_is_denied("CODE.BAT"));
        assert!(!custom_editor_program_is_denied("code.cmd")); // allow-listed Windows launcher
        assert!(!custom_editor_program_is_denied("CURSOR.CMD"));
        assert!(!custom_editor_program_is_denied("nvim"));
    }

    #[test]
    fn windows_editor_script_allowlist_keeps_random_cmd_denied() {
        assert!(!custom_editor_program_is_denied("code.cmd"));
        assert!(!custom_editor_program_is_denied("cursor.cmd"));
        assert!(custom_editor_program_is_denied("notepad.cmd"));
        assert!(custom_editor_program_is_denied("code.bat")); // only .cmd launchers allow-listed
        assert!(custom_editor_program_is_denied("cursor.ps1"));
    }

    #[test]
    fn preset_programs_pass_denylist_on_all_oses() {
        for os in [OsFamily::Linux, OsFamily::Macos, OsFamily::Windows] {
            for preset in [
                EditorPreset::Default,
                EditorPreset::VsCode,
                EditorPreset::Cursor,
            ] {
                let (prog, _) = build_preset_argv(os, preset, "/tmp/x.rs", Some(1));
                assert!(
                    !custom_editor_program_is_denied(&prog),
                    "preset {preset:?} on {os:?} program {prog:?} must pass denylist (allow-list Windows .cmd launchers)"
                );
            }
        }
    }

    #[cfg(windows)]
    #[test]
    fn editor_denylist_windows_paths() {
        assert!(custom_editor_program_is_denied(
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"
        ));
        assert!(custom_editor_program_is_denied(
            r"C:\Windows\System32\cmd.exe."
        ));
        assert!(custom_editor_program_is_denied(r".\wsl.exe"));
    }

    #[cfg(unix)]
    #[test]
    fn editor_denylist_follows_symlink_to_shell() {
        let dir = std::env::temp_dir().join(format!("lounge-editor-deny-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let link = dir.join("innocent-editor");
        let target = if std::path::Path::new("/bin/sh").exists() {
            "/bin/sh"
        } else {
            "/usr/bin/sh"
        };
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(target, &link).expect("symlink");
        let err = validate_custom_editor(link.to_str().unwrap(), "{path}").expect_err("symlink");
        assert!(
            err.to_string().contains("denylist"),
            "symlink to sh must deny: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_expand_template_includes_line() {
        let args = expand_custom_template("-g {path}", "/tmp/x.rs", Some(42));
        assert_eq!(args, vec!["-g", "/tmp/x.rs:42"]);
    }

    #[test]
    fn default_opener_never_appends_line_on_any_os() {
        for os in [OsFamily::Linux, OsFamily::Macos, OsFamily::Windows] {
            let (prog, args) = build_preset_argv(os, EditorPreset::Default, "/tmp/x.rs", Some(42));
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
            assert!(
                args.contains("{path}")
                    || prog == "open"
                    || prog == "xdg-open"
                    || prog == "explorer.exe"
            );
        }
    }
}
